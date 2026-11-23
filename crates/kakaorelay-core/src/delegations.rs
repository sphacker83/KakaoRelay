//! 원본: `BackgroundDelegations.cs`의 `DelegationJob`, `DelegationJobStore`, `BackgroundDelegations`.
//! Grok 위임 작업은 방 대화 게이트를 잡지 않는다. 같은 (프로필, 방, 메시지)의 작업은 재시작 뒤에도 다시 접수하지 않는다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use chrono::{DateTime, FixedOffset, Utc};
use kakaorelay_ai::ai_service::{AiCommand, AiResult, AiService};
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::grok_delegation::{GrokCompletion, GrokTask};
use kakaorelay_ai::{Error, LocalMessage, LocalRoom, Result, json};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use crate::fs::{create_new, write_atomic};
use crate::support::{BoxFuture, error_kind};

/// 이번 실행의 식별자. 이전 실행에서 진행 중이던 작업은 목록에서 `interrupted`로 보인다.
pub static RUN_ID: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().simple().to_string());

const ACTIVE: [&str; 5] = ["accepting", "queued", "running", "finalizing", "sending"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationJob {
    pub id: String,
    pub profile: String,
    pub room_id: String,
    pub room_title: String,
    pub message_id: String,
    pub request: String,
    pub task: GrokTask,
    #[serde(with = "kakaorelay_ai::json::time")]
    pub created_at: DateTime<FixedOffset>,
    #[serde(default = "run_id")]
    pub run_id: String,
    #[serde(default = "accepting")]
    pub status: String,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(with = "kakaorelay_ai::json::time", default = "utc_now")]
    pub updated_at: DateTime<FixedOffset>,
}

fn run_id() -> String {
    RUN_ID.clone()
}
fn accepting() -> String {
    "accepting".into()
}
fn utc_now() -> DateTime<FixedOffset> {
    Utc::now().fixed_offset()
}

pub struct DelegationJobStore {
    folder: PathBuf,
}

impl DelegationJobStore {
    /// 원본 `AppContext.BaseDirectory/data/delegations`.
    pub fn default_root() -> PathBuf {
        kakaorelay_ai::paths::provider_data().join("delegations")
    }

    pub fn new(root: Option<PathBuf>) -> Self {
        Self {
            folder: root.unwrap_or_else(Self::default_root),
        }
    }

    fn path(&self, id: &str) -> PathBuf {
        self.folder.join(format!("{id}.json"))
    }

    /// 이미 있으면 `false`. 재시작 뒤 같은 요청을 다시 접수하지 않는 근거다.
    pub fn try_create(&self, job: &DelegationJob) -> Result<bool> {
        std::fs::create_dir_all(&self.folder)?;
        match create_new(&self.path(&job.id), json::indented(job).as_bytes()) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, job: &mut DelegationJob, status: &str, error: Option<String>) -> Result<()> {
        job.status = status.into();
        job.error = error;
        job.updated_at = utc_now();
        write_atomic(&self.path(&job.id), json::indented(job).as_bytes(), false)
    }

    pub fn list(&self) -> Vec<DelegationJob> {
        let Ok(entries) = std::fs::read_dir(&self.folder) else {
            return vec![];
        };
        let mut result: Vec<DelegationJob> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .filter_map(|p| {
                let text = kakaorelay_ai::text::read_text(&p).ok()?;
                serde_json::from_str::<Option<DelegationJob>>(&text).ok()?
            })
            .map(|mut job| {
                if job.run_id != *RUN_ID && ACTIVE.contains(&job.status.as_str()) {
                    job.status = "interrupted".into();
                }
                job
            })
            .collect();
        result.sort_by_key(|job| std::cmp::Reverse(job.created_at));
        result
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }
}

/// `(방, 원래 메시지, 보낼 결과, 단계, 취소)` → 발송. 원본 `Func<LocalRoom, LocalMessage, AiResult, string, CancellationToken, Task>`.
pub type DelegationSend = Arc<
    dyn Fn(
            LocalRoom,
            LocalMessage,
            AiResult,
            String,
            CancellationToken,
        ) -> BoxFuture<'static, Result<()>>
        + Send
        + Sync,
>;
pub type StatusFn = Arc<dyn Fn(&str) + Send + Sync>;

/// 원본 `BackgroundDelegations`. 한 방의 폴링 세션(또는 제어방)이 소유한다.
pub struct BackgroundDelegations {
    ai: Arc<AiService>,
    send: DelegationSend,
    status: StatusFn,
    stop: CancellationToken,
    store: Arc<DelegationJobStore>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    pending: Arc<AtomicUsize>,
}

impl BackgroundDelegations {
    pub fn new(
        ai: Arc<AiService>,
        send: DelegationSend,
        status: StatusFn,
        cancel: &CancellationToken,
        root: Option<PathBuf>,
    ) -> Self {
        Self {
            ai,
            send,
            status,
            stop: cancel.child_token(),
            store: Arc::new(DelegationJobStore::new(root)),
            workers: Mutex::new(vec![]),
            pending: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    pub async fn start(
        &self,
        room: &LocalRoom,
        message: &LocalMessage,
        plan: &AiResult,
    ) -> Result<()> {
        if self.stop.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let task = plan
            .delegation
            .clone()
            .ok_or_else(|| Error::argument("위임 작업이 없습니다."))?;
        let id =
            json::hash(&format!("{}:{}:{}", room.profile, room.id, message.id))[..40].to_string();
        let mut job = DelegationJob {
            id,
            profile: room.profile.clone(),
            room_id: room.id.clone(),
            room_title: room.title.clone(),
            message_id: message.id.clone(),
            request: message.text.clone(),
            task,
            created_at: utc_now(),
            run_id: run_id(),
            status: accepting(),
            result: None,
            error: None,
            updated_at: utc_now(),
        };
        if !self.store.try_create(&job)? {
            return Ok(()); // 이전에 접수한 작업은 재시작 뒤에도 다시 실행하지 않는다.
        }
        let accepted = async {
            (self.send)(
                room.clone(),
                message.clone(),
                plan.clone(),
                String::new(),
                self.stop.clone(),
            )
            .await?;
            if self.stop.is_cancelled() {
                return Err(Error::Cancelled);
            }
            self.store.save(&mut job, "queued", None)
        }
        .await;
        if let Err(error) = accepted {
            self.store
                .save(&mut job, "uncertain", Some(error_kind(&error)))?;
            return Err(error);
        }
        let mut workers = self.workers.lock().unwrap_or_else(|e| e.into_inner());
        workers.retain(|w| !w.is_finished());
        self.pending.fetch_add(1, Ordering::SeqCst);
        let worker = Worker {
            ai: self.ai.clone(),
            send: self.send.clone(),
            status: self.status.clone(),
            stop: self.stop.clone(),
            store: self.store.clone(),
            pending: self.pending.clone(),
        };
        let (room, message, plan) = (room.clone(), message.clone(), plan.clone());
        workers.push(tokio::spawn(async move {
            worker.run(job, room, message, plan).await
        }));
        Ok(())
    }

    /// 원본 `DisposeAsync`. 남은 Grok 작업을 취소하고 끝날 때까지 기다린다.
    pub async fn dispose(&self) {
        self.stop.cancel();
        let workers = std::mem::take(&mut *self.workers.lock().unwrap_or_else(|e| e.into_inner()));
        for worker in workers {
            let _ = worker.await;
        }
    }
}

struct Worker {
    ai: Arc<AiService>,
    send: DelegationSend,
    status: StatusFn,
    stop: CancellationToken,
    store: Arc<DelegationJobStore>,
    pending: Arc<AtomicUsize>,
}

fn request_label(message: &LocalMessage) -> String {
    let text = message.text.replace(['\r', '\n'], " ");
    let text = if kakaorelay_ai::text::utf16_len(&text) > 80 {
        format!("{}…", kakaorelay_ai::text::utf16_prefix(&text, 80))
    } else {
        text
    };
    format!("“{text}” 요청 결과\n")
}

impl Worker {
    async fn run(
        self,
        mut job: DelegationJob,
        room: LocalRoom,
        message: LocalMessage,
        plan: AiResult,
    ) {
        let mut sending = false;
        let outcome = self
            .work(&mut job, &room, &message, &plan, &mut sending)
            .await;
        match outcome {
            Ok(()) => {}
            Err(Error::Cancelled) if self.stop.is_cancelled() => {
                self.record(
                    &mut job,
                    if sending { "uncertain" } else { "cancelled" },
                    None,
                );
            }
            Err(error) => {
                self.record(
                    &mut job,
                    if sending { "uncertain" } else { "failed" },
                    Some(error_kind(&error)),
                );
                (self.status)(&format!(
                    "Grok 작업 {} · 일반 대화 계속",
                    if sending {
                        "발송 확인 필요"
                    } else {
                        "실패"
                    }
                ));
                if !sending && !self.stop.is_cancelled() {
                    let notice = AiResult {
                        text: format!(
                            "{}검토 작업을 완료하지 못했어요. 다른 대화는 계속할 수 있어요.",
                            request_label(&message)
                        ),
                        delegation: None,
                        attachment: None,
                        image_path: None,
                        image_prompt: None,
                        ..plan
                    };
                    if let Err(failure) = (self.send)(
                        room,
                        message,
                        notice,
                        "grok-error".into(),
                        self.stop.clone(),
                    )
                    .await
                    {
                        let state = if self.stop.is_cancelled() {
                            "cancelled"
                        } else {
                            "uncertain"
                        };
                        self.record(&mut job, state, Some(error_kind(&failure)));
                    }
                }
            }
        }
        self.pending.fetch_sub(1, Ordering::SeqCst);
    }

    async fn work(
        &self,
        job: &mut DelegationJob,
        room: &LocalRoom,
        message: &LocalMessage,
        plan: &AiResult,
        sending: &mut bool,
    ) -> Result<()> {
        self.store.save(job, "running", None)?;
        (self.status)(&format!("Grok 작업 중 · {}", &job.id[..8]));
        let raw = self.ai.run_grok_task(&job.task, &self.stop).await?;
        job.result = Some(raw.clone());
        self.store.save(job, "finalizing", None)?;
        let mut command = AiCommand::new(&room.profile, &room.id, "reply");
        command.instruction = "완료된 백그라운드 작업의 결과를 원래 요청에 맞게 전달하라.".into();
        command.target_message_id = Some(message.id.clone());
        let completion = GrokCompletion {
            job_id: job.id.clone(),
            request: message.text.clone(),
            task: job.task.clone(),
            result: raw,
        };
        let last = self
            .ai
            .generate(&command, &self.stop, None, true, false, Some(&completion))
            .await?;
        if last.persona_id != plan.persona_id {
            return Err(Error::invalid_operation(
                "작업 중 연결된 페르소나가 변경되었습니다.",
            ));
        }
        if self.stop.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.store.save(job, "sending", None)?;
        *sending = true;
        let delivered = AiResult {
            text: format!("{}{}", request_label(message), last.text),
            attachment: None,
            ..last
        };
        (self.send)(
            room.clone(),
            message.clone(),
            delivered,
            "grok-result".into(),
            self.stop.clone(),
        )
        .await?;
        self.store.save(job, "completed", None)?;
        (self.status)(&format!("Grok 결과 전달 완료 · {}", &job.id[..8]));
        Ok(())
    }

    /// 상태 기록 실패(입출력)는 작업 결과를 바꾸지 않는다(원본과 같다).
    fn record(&self, job: &mut DelegationJob, state: &str, error: Option<String>) {
        let _ = self.store.save(job, state, error);
    }
}
