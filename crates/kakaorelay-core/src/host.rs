//! 원본 앱 조립부를 화면과 분리한 서비스.
//! - `MainWindow.SendFromApiAsync`: 종료 중 거절, 전역 발송 큐, 수동 작업과의 상호 배제
//! - `App.OnStartup`: 발송 서비스·자동답장·정리감시·제어방·자동 정리·API 조립
//! - `AiPanel.ExecuteCommandAsync`·`RuntimeSnapshot`: 제어방 명령 실행과 런타임 상태
//! - `App.ExitFromTrayAsync`: 종료 순서(새 작업 차단 → 봇·전달·제어방 드레인 → 진행 중 발송 대기 → API 정지)

use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use kakaorelay_ai::ai_service::AiService;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::settings::AiSettings;
use kakaorelay_ai::{ApiFailure, Error, LocalRoom, Result};
use serde_json::{Value, json};

use crate::api_send::{ApiSendCommand, ApiSendService, Scan};
use crate::command_room::{CommandPlan, CommandRoomService, CommandRoomStore, policy};
use crate::periodic::Periodic;
use crate::platform::Platform;
use crate::relay_api::{RelayApi, RelayApiConfig, default_connection_path};
use crate::reply_runtime::{CancellableSend, ReplyRuntime};
use crate::summary_forwarder::SummaryForwarder;
use crate::support::{BoxFuture, StatusEvent};
use crate::test_send::{TestSendReceipt, default_ledger, read_history};
use crate::work_queue::WorkQueue;

const SHUTTING_DOWN: &str = "앱이 종료 중입니다. 같은 요청 ID를 유지하세요.";

fn shutting_down() -> Error {
    Error::Api(ApiFailure::new(503, "shutting_down", SHUTTING_DOWN))
}

/// 조립에 쓰는 경로. 기본값은 원본 앱과 같은 위치다.
#[derive(Debug, Clone)]
pub struct HostPaths {
    pub ledger: PathBuf,
    pub connection: PathBuf,
}

impl Default for HostPaths {
    fn default() -> Self {
        Self {
            ledger: default_ledger(),
            connection: default_connection_path(),
        }
    }
}

/// 발송 조정자. API·자동답장·정리감시·제어방 발송은 모두 이 큐를 지나고, 앱의 수동 작업과 겹치지 않는다.
pub struct SendCoordinator {
    sends: ApiSendService,
    queue: WorkQueue,
    /// 잡고 있는 동안이 원본 `working`이다(네이티브 입력 작업 하나만 진행).
    native: tokio::sync::Mutex<()>,
    queued: AtomicUsize,
    shutting_down: AtomicBool,
    pub sent: StatusEvent,
}

impl SendCoordinator {
    pub fn new(sends: ApiSendService) -> Self {
        Self {
            sends,
            queue: WorkQueue::new(),
            native: tokio::sync::Mutex::new(()),
            queued: AtomicUsize::new(0),
            shutting_down: AtomicBool::new(false),
            sent: StatusEvent::default(),
        }
    }

    pub fn queued(&self) -> usize {
        self.queued.load(Ordering::SeqCst)
    }

    pub fn working(&self) -> bool {
        self.native.try_lock().is_err()
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// 원본 `SendFromApiAsync`. 순서는 호출 시점에 예약한다.
    pub async fn send(
        &self,
        command: ApiSendCommand,
        cancel: &CancellationToken,
    ) -> Result<TestSendReceipt> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if self.is_shutting_down() {
            return Err(shutting_down());
        }
        self.queued.fetch_add(1, Ordering::SeqCst);
        let outcome = self
            .queue
            .enqueue(async {
                // 수동 발송·화면 확인 작업이 끝난 뒤 다음 네이티브 작업을 시작한다.
                let _working = self.native.lock().await;
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                if self.is_shutting_down() {
                    return Err(shutting_down());
                }
                let receipt = self.sends.send(command).await?;
                self.sent.emit(&format!(
                    "AI · {} · {}",
                    receipt.recipient,
                    receipt.display_status()
                ));
                Ok(receipt)
            })
            .await;
        self.queued.fetch_sub(1, Ordering::SeqCst);
        outcome
    }

    /// 앱의 수동 발송·창 확인처럼 큐 밖에서 하는 네이티브 작업. 진행 중인 발송이 끝난 뒤 시작한다.
    pub async fn exclusive<T>(&self, operation: impl Future<Output = T>) -> T {
        let _working = self.native.lock().await;
        operation.await
    }

    async fn drain(&self) {
        // 진행 중인 발송이 최종 영수증을 저장할 때까지 기다린다(원본은 100ms 폴링).
        drop(self.native.lock().await);
    }
}

pub struct RelayHost {
    ai: Arc<AiService>,
    coordinator: Arc<SendCoordinator>,
    runtime: ReplyRuntime,
    forwarder: Arc<SummaryForwarder>,
    commands: Arc<CommandRoomService>,
    command_store: CommandRoomStore,
    loops: Vec<Periodic>,
    api: tokio::sync::Mutex<Option<RelayApi>>,
    settings: Mutex<AiSettings>,
    settings_busy: AtomicBool,
    known_rooms: Mutex<Vec<LocalRoom>>,
    /// 제어방이 설정을 바꿨을 때 알린다(원본 `RemoteSettingsChanged`).
    pub settings_changed: StatusEvent,
    /// 자동 정리 주기 실행의 실패 문구(원본은 패널 기록에 남긴다). 진행 상황은 `automatic_knowledge.on_status`로 받는다.
    pub knowledge_errors: StatusEvent,
}

impl RelayHost {
    /// 원본 `OnStartup`의 서비스 조립과 API 시작. 주기 작업은 바로 돈다.
    pub async fn start(
        platform: Arc<dyn Platform>,
        ai: Arc<AiService>,
        paths: HostPaths,
    ) -> Result<Arc<Self>> {
        let settings = ai.settings().load()?;
        let coordinator = Arc::new(SendCoordinator::new(ApiSendService::from_platform(
            &paths.ledger,
            platform.clone(),
        )));
        let send: CancellableSend = {
            let coordinator = coordinator.clone();
            Arc::new(move |command, cancel| {
                let coordinator = coordinator.clone();
                Box::pin(async move { coordinator.send(command, &cancel).await })
            })
        };
        let scan: Scan = {
            let platform = platform.clone();
            Arc::new(move || platform.rooms().scan())
        };
        let runtime = ReplyRuntime::new(ai.clone(), send.clone(), scan.clone());
        let forwarder = Arc::new(SummaryForwarder::new(
            ai.clone(),
            send.clone(),
            scan.clone(),
            None,
            None,
        ));
        let command_store = CommandRoomStore::new(None);
        let knowledge_errors = StatusEvent::default();
        let host = Arc::new_cyclic(|me: &Weak<RelayHost>| {
            let (status_host, settings_host, execute_host) = (me.clone(), me.clone(), me.clone());
            let commands = Arc::new(CommandRoomService::new(
                ai.clone(),
                command_store.clone(),
                Arc::new(move || {
                    settings_host
                        .upgrade()
                        .map(|h| h.settings())
                        .unwrap_or_default()
                }),
                Arc::new(move |plan, ct| {
                    let host = execute_host.clone();
                    Box::pin(async move {
                        match host.upgrade() {
                            Some(host) => host.execute(plan, &ct).await,
                            None => Err(Error::Cancelled),
                        }
                    }) as BoxFuture<'static, Result<String>>
                }),
                Arc::new(move || {
                    status_host
                        .upgrade()
                        .map(|h| h.command_runtime_status())
                        .unwrap_or_default()
                }),
                send.clone(),
            ));
            let loops = vec![
                {
                    let forwarder = forwarder.clone();
                    Periodic::start(Duration::from_secs(15), move |ct| {
                        let forwarder = forwarder.clone();
                        async move {
                            // 실패는 상태 문구로 이미 알렸다.
                            let _ = forwarder.tick(&ct).await;
                        }
                    })
                },
                {
                    let commands = commands.clone();
                    Periodic::start(Duration::from_secs(2), move |ct| {
                        let commands = commands.clone();
                        async move {
                            let _ = commands.tick(&ct).await;
                        }
                    })
                },
                {
                    let (ai, errors) = (ai.clone(), knowledge_errors.clone());
                    Periodic::start(Duration::from_secs(60), move |ct| {
                        let (ai, errors) = (ai.clone(), errors.clone());
                        async move {
                            let outcome = match ai.settings().load() {
                                Ok(settings) => {
                                    ai.automatic_knowledge.tick(&settings, false, &ct).await
                                }
                                Err(error) => Err(error),
                            };
                            if let Err(error) = outcome
                                && !matches!(error, Error::Cancelled)
                            {
                                errors.emit(&error.to_string());
                            }
                        }
                    })
                },
            ];
            RelayHost {
                ai: ai.clone(),
                coordinator: coordinator.clone(),
                runtime,
                forwarder,
                commands,
                command_store,
                loops,
                api: tokio::sync::Mutex::new(None),
                settings: Mutex::new(settings),
                settings_busy: AtomicBool::new(false),
                known_rooms: Mutex::new(vec![]),
                settings_changed: StatusEvent::default(),
                knowledge_errors: knowledge_errors.clone(),
            }
        });
        let ledger = paths.ledger.clone();
        let api_send = coordinator.clone();
        let mut config = RelayApiConfig::new(
            &paths.connection,
            scan,
            Arc::new(move || read_history(&ledger)),
            Arc::new(move |command| {
                let coordinator = api_send.clone();
                Box::pin(async move { coordinator.send(command, &CancellationToken::new()).await })
            }),
        );
        config.ai = Some(ai);
        config.image_transport = platform.image_transport_name().into();
        let snapshot = Arc::downgrade(&host);
        config.runtime = Some(Arc::new(move || {
            let value = snapshot
                .upgrade()
                .map(|h| h.runtime_snapshot())
                .unwrap_or(Value::Null);
            Box::pin(async move { value })
        }));
        *host.api.lock().await = Some(RelayApi::start(config).await?);
        Ok(host)
    }

    pub fn ai(&self) -> &Arc<AiService> {
        &self.ai
    }

    pub fn coordinator(&self) -> &Arc<SendCoordinator> {
        &self.coordinator
    }

    pub fn runtime(&self) -> &ReplyRuntime {
        &self.runtime
    }

    pub fn forwarder(&self) -> &Arc<SummaryForwarder> {
        &self.forwarder
    }

    pub fn commands(&self) -> &Arc<CommandRoomService> {
        &self.commands
    }

    pub async fn connection(&self) -> Option<crate::relay_api::ApiConnection> {
        self.api
            .lock()
            .await
            .as_ref()
            .map(|api| api.connection().clone())
    }

    /// 앱이 보유한 현재 설정(원본 `AiPanel.Settings`).
    pub fn settings(&self) -> AiSettings {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 앱에서 설정을 저장한다. 정리감시 규칙이 바뀌면 진행 중인 정리감시 실행을 끊는다.
    pub fn save_settings(&self, mut next: AiSettings) -> Result<()> {
        self.ai.settings().save(&mut next)?;
        let forward_changed = self.settings().summary_forwards != next.summary_forwards;
        *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = next;
        if forward_changed {
            self.loops[0].cancel_current();
        }
        Ok(())
    }

    /// 앱 화면에서 다른 설정 작업이 진행 중이면 제어방 명령 실행을 거절한다.
    pub fn set_settings_busy(&self, busy: bool) {
        self.settings_busy.store(busy, Ordering::SeqCst);
    }

    /// 제어방 `/상태`에 쓰는 로컬 방 목록(원본은 화면에 불러온 방 목록).
    pub fn set_known_rooms(&self, rooms: Vec<LocalRoom>) {
        *self.known_rooms.lock().unwrap_or_else(|e| e.into_inner()) = rooms;
    }

    /// 원본 `ResumeSelectedRepliesAsync`. 저장된 자동답장 방을 다시 시작한다(제어방 제외).
    pub async fn resume_selected_replies(&self, cancel: &CancellationToken) -> Result<()> {
        let rooms = self.ai.chats().rooms().await?;
        self.set_known_rooms(rooms.clone());
        let settings = self.settings();
        let control = self.command_store.load()?;
        let selected: Vec<LocalRoom> = rooms
            .into_iter()
            .filter(|r| {
                settings
                    .selected_bot_rooms
                    .contains(&kakaorelay_ai::settings::room_key(&r.profile, &r.id))
                    && !control.owns(r)
            })
            .collect();
        self.runtime.start(&selected, cancel).await
    }

    fn bot_badge(&self) -> String {
        match self.runtime.count() {
            0 => "○ 자동답장 꺼짐".into(),
            n => format!("● 자동답장 {n}개 방"),
        }
    }

    fn command_runtime_status(&self) -> String {
        let rooms: Vec<String> = self
            .known_rooms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|r| format!("{} · {}", r.title, r.id))
            .collect();
        format!(
            "{}\n{}\n{}",
            self.bot_badge(),
            self.runtime.statuses().join("\n"),
            rooms.join("\n")
        )
    }

    /// 원본 `RuntimeSnapshot`(`GET /v1/ai/runtime`).
    pub fn runtime_snapshot(&self) -> Value {
        let active = self.runtime.count();
        json!({
            "autoReplyRunning": active > 0,
            "activeRooms": active,
            "rooms": self.runtime.statuses(),
            "lastError": self.runtime.last_error(),
            "commandRoom": {
                "enabled": self.command_store.load().is_ok_and(|c| c.enabled),
                "busy": self.commands.busy(),
                "status": self.commands.status(),
            },
        })
    }

    /// 원본 `ExecuteCommandAsync`. 실행 상태 전환과 저장이 성공하기 전에는 성공을 알리지 않는다.
    async fn execute(&self, plan: CommandPlan, ct: &CancellationToken) -> Result<String> {
        if self.settings_busy.load(Ordering::SeqCst) {
            return Err(Error::invalid_operation(
                "앱에서 다른 설정 작업 중입니다. 잠시 후 다시 요청하세요.",
            ));
        }
        if plan.revision != policy::revision(&self.settings()) {
            return Err(Error::invalid_operation(
                "설정이 바뀌었습니다. 다시 요청하세요.",
            ));
        }
        let reply = plan.command.action == "reply";
        if reply {
            if plan.command.value == "켜기" {
                let room = plan
                    .room
                    .clone()
                    .ok_or_else(|| Error::invalid_operation("방이 없습니다."))?;
                self.runtime.start(&[room], ct).await?;
            } else {
                self.runtime.stop(plan.room.as_ref()).await;
            }
        }
        if let Err(error) = self.save_settings(plan.next.clone()) {
            return Err(Error::invalid_operation(format!(
                "{}{error}",
                if reply {
                    "자동답장 실행 상태는 변경됐지만 설정 저장 실패: "
                } else {
                    "설정 저장 실패: "
                }
            )));
        }
        self.settings_changed.emit("");
        if plan.command.action == "knowledge-run" {
            let knowledge = &self.ai.automatic_knowledge;
            if knowledge.busy() {
                return Ok(format!("{} · 이미 진행 중입니다.", plan.description));
            }
            let settings = self.settings();
            knowledge.tick(&settings, true, ct).await?;
            return Ok(knowledge.saved_statuses(&settings).join("\n"));
        }
        Ok(format!("적용: {}", plan.description))
    }

    /// 원본 `ExitFromTrayAsync`. 두 번 불러도 안전하다.
    pub async fn shutdown(&self) {
        // 1. 새 작업 차단과 주기 작업·자동답장 중지(원본 `PrepareShutdown`).
        self.coordinator.shutting_down.store(true, Ordering::SeqCst);
        for periodic in &self.loops {
            periodic.stop();
        }
        self.commands.stop();
        // 2. 봇·전달·제어방 드레인(원본 `WaitForBotsAsync`).
        self.runtime.stop(None).await;
        self.runtime.drain().await;
        for periodic in &self.loops {
            periodic.drain().await;
        }
        self.commands.drain().await;
        // 3. 진행 중 발송이 최종 영수증을 저장할 때까지 기다린다.
        self.coordinator.drain().await;
        // 4. API 정지(최대 2초).
        if let Some(api) = self.api.lock().await.take() {
            api.stop().await;
        }
    }
}
