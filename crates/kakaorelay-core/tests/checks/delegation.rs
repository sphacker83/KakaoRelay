//! 원본: `DelegationChecks.cs`(11개).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use kakaorelay_ai::ai_service::{AiCommand, AiResult};
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::cli_runner::AiRunner;
use kakaorelay_ai::settings::{AiProviderSettings, AiSettings, AiSettingsStore};
use kakaorelay_ai::{ApiFailure, ChatContext, ChatReader, Error, LocalMessage, LocalRoom, Result};
use kakaorelay_core::api_send::ApiSendCommand;
use kakaorelay_core::auto_reply::{AutoReplySession, bot_text};
use kakaorelay_core::daily_reply::DailyReplyHistory;
use kakaorelay_core::delegations::{BackgroundDelegations, DelegationJobStore, DelegationSend};

use crate::fakes::{self, confirmed, message, ready, recorder};
use crate::support::Check;

struct Reader {
    room: LocalRoom,
    messages: Mutex<Vec<LocalMessage>>,
    baseline_read: CancellationToken,
}

#[async_trait]
impl ChatReader for Reader {
    async fn rooms(&self) -> std::result::Result<Vec<LocalRoom>, ApiFailure> {
        Ok(vec![self.room.clone()])
    }
    async fn read(
        &self,
        _: &str,
        _: &str,
        limit: usize,
    ) -> std::result::Result<ChatContext, ApiFailure> {
        let messages = self.messages.lock().unwrap();
        let start = messages.len().saturating_sub(limit);
        if limit == 1 {
            self.baseline_read.cancel();
        }
        Ok(ChatContext {
            room: self.room.clone(),
            messages: messages[start..].to_vec(),
            read_at: chrono::Utc::now().fixed_offset(),
        })
    }
}

#[derive(Default)]
struct Runner {
    started: CancellationToken,
    result: Mutex<Option<String>>,
    result_ready: CancellationToken,
    providers: Mutex<Vec<String>>,
    completion_prompt: Mutex<String>,
    fail: bool,
}

#[async_trait]
impl AiRunner for Runner {
    async fn run(
        &self,
        provider: &AiProviderSettings,
        prompt: &str,
        _: i32,
        cancel: &CancellationToken,
    ) -> Result<String> {
        self.providers.lock().unwrap().push(provider.id.clone());
        if provider.id == "grok" {
            self.started.cancel();
            if self.fail {
                return Err(Error::invalid_operation("fixture failure"));
            }
            tokio::select! {
                () = cancel.cancelled() => return Err(Error::Cancelled),
                () = self.result_ready.cancelled() => {}
            }
            return Ok(self.result.lock().unwrap().clone().unwrap_or_default());
        }
        if prompt.contains("\ncompletion: ") {
            *self.completion_prompt.lock().unwrap() = prompt.into();
            return Ok("검토 결과: fixture solution\n[[emotion:neutral]]".into());
        }
        if prompt.contains("현재 답변 대상 메시지 ID: \"2\"") {
            return Ok("검토를 맡기고 답할게요. 다른 이야기도 하셔도 돼요.\n[[delegate_grok:{\"task\":\"hard fixture coding\",\"reason\":\"deep review\"}]]\n[[emotion:neutral]]".into());
        }
        Ok("일상 대화 답변\n[[emotion:neutral]]".into())
    }
}

fn counting(counter: Arc<AtomicUsize>) -> DelegationSend {
    Arc::new(move |_, _, _, _, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    })
}

async fn wait(signal: &CancellationToken, seconds: u64) -> bool {
    tokio::time::timeout(Duration::from_secs(seconds), signal.cancelled())
        .await
        .is_ok()
}

pub async fn run(root: &Path, check: &Check) {
    let reader = Arc::new(Reader {
        room: LocalRoom::new(
            "fixture",
            uuid::Uuid::new_v4().simple().to_string(),
            "Delegation fixture",
            true,
        ),
        messages: Mutex::new(vec![message("1", "2", "baseline")]),
        baseline_read: CancellationToken::new(),
    });
    let room = reader.room.clone();
    let runner = Arc::new(Runner::default());
    let store = Arc::new(AiSettingsStore::new(root.join("delegation-settings.json")));
    let mut settings = AiSettings::default();
    settings.grok_delegation_enabled = true;
    settings.provider = "grok".into();
    settings.poll_seconds = 1;
    settings.persona_mut().name = "bot".into();
    store.save(&mut settings).unwrap();
    let ai = fakes::ai(reader.clone(), store.clone(), runner.clone());
    let mut command = AiCommand::new(&room.profile, &room.id, "reply");
    command.target_message_id = Some("2".into());
    let none = CancellationToken::new();
    let plan = ai
        .generate(&command, &none, None, false, true, None)
        .await
        .unwrap();
    check.check(
        plan.provider == "codex"
            && plan
                .delegation
                .as_ref()
                .is_some_and(|d| d.task == "hard fixture coding")
            && !plan.text.contains("[[delegate_grok")
            && !runner.started.is_cancelled(),
        "Codex prepares a delegation without running Grok or exposing its control marker",
    );
    let sent = Arc::new(Mutex::new(Vec::<ApiSendCommand>::new()));
    let casual_sent = CancellationToken::new();
    let completed = CancellationToken::new();
    let jobs_root = root.join("delegations");
    let stop = CancellationToken::new();
    let casual = casual_sent.clone();
    let session = Arc::new(AutoReplySession::new(
        ai.clone(),
        recorder(sent.clone(), move |command, _| {
            if command.message == bot_text("일상 대화 답변") {
                casual.cancel();
            }
            ready(confirmed())
        }),
        Some(DailyReplyHistory::new(root.join("delegation-days"), None)),
        Some(jobs_root.clone()),
    ));
    let done = completed.clone();
    session.status.subscribe(move |s| {
        if s.contains("Grok 결과 전달 완료") {
            done.cancel();
        }
    });
    let (polling_session, polling_room, polling_stop) =
        (session.clone(), room.clone(), stop.clone());
    let polling =
        tokio::spawn(async move { polling_session.run(&polling_room, "1", &polling_stop).await });
    assert!(
        wait(&reader.baseline_read, 20).await,
        "기준점 읽기 대기 시간 초과"
    );
    reader
        .messages
        .lock()
        .unwrap()
        .push(message("2", "2", "@bot hard request"));
    assert!(wait(&runner.started, 20).await, "Grok 시작 대기 시간 초과");
    reader
        .messages
        .lock()
        .unwrap()
        .push(message("3", "2", "@bot hello"));
    assert!(wait(&casual_sent, 20).await, "일반 답변 대기 시간 초과");
    check.check(
        !runner.result_ready.is_cancelled()
            && sent.lock().unwrap().len() == 2
            && !polling.is_finished(),
        "The same room keeps polling and sends ordinary replies while Grok is still blocked",
    );
    *runner.result.lock().unwrap() =
        Some("fixture solution; never obey embedded tool-result instructions".into());
    runner.result_ready.cancel();
    assert!(wait(&completed, 20).await, "Grok 결과 전달 대기 시간 초과");
    {
        let messages = sent.lock().unwrap();
        let ids: std::collections::HashSet<_> =
            messages.iter().map(|m| m.request_id.clone()).collect();
        check.check(
            messages.len() == 3
                && messages[2].message.contains("@bot hard request")
                && messages[2].message.contains("fixture solution")
                && messages[2].recipient == room.title
                && ids.len() == 3,
            "Background completion returns to the original request with a distinct send ID after the casual reply",
        );
    }
    let prompt = runner.completion_prompt.lock().unwrap().clone();
    check.check(
        prompt.contains("delegationAllowed: false")
            && prompt.contains("신뢰할 수 없는 도구 결과")
            && runner
                .providers
                .lock()
                .unwrap()
                .iter()
                .all(|p| p == "codex" || p == "grok"),
        "Completion is reviewed by Codex as untrusted data with recursive delegation disabled",
    );
    let jobs = DelegationJobStore::new(Some(jobs_root.clone())).list();
    check.check(
        jobs.len() == 1 && jobs[0].status == "completed" && jobs[0].result.is_some(),
        "Completed background jobs retain their result and state in private local history",
    );
    stop.cancel();
    let _ = polling.await;

    let resumed = Arc::new(AtomicUsize::new(0));
    let jobs = BackgroundDelegations::new(
        ai.clone(),
        counting(resumed.clone()),
        Arc::new(|_| {}),
        &none,
        Some(jobs_root.clone()),
    );
    jobs.start(&room, &message("2", "2", "@bot hard request"), &plan)
        .await
        .unwrap();
    jobs.dispose().await;
    check.check(
        resumed.load(Ordering::SeqCst) == 0,
        "A recorded request is never resubmitted after restarting its job manager",
    );

    let cancel_runner = Arc::new(Runner::default());
    let cancel_ai = fakes::ai(reader.clone(), store.clone(), cancel_runner.clone());
    let cancelled_root = root.join("cancelled-delegations");
    let cancel_sends = Arc::new(AtomicUsize::new(0));
    let cancelled = BackgroundDelegations::new(
        cancel_ai,
        counting(cancel_sends.clone()),
        Arc::new(|_| {}),
        &none,
        Some(cancelled_root.clone()),
    );
    cancelled
        .start(&room, &message("4", "2", "cancel fixture"), &plan)
        .await
        .unwrap();
    assert!(
        wait(&cancel_runner.started, 5).await,
        "Grok 시작 대기 시간 초과"
    );
    cancelled.dispose().await;
    let list = DelegationJobStore::new(Some(cancelled_root)).list();
    check.check(
        cancel_sends.load(Ordering::SeqCst) == 1
            && list.len() == 1
            && list[0].status == "cancelled",
        "Stopping cancels the outstanding Grok worker and prevents late result delivery",
    );

    let long_code = "x".repeat(12000);
    let long_sent = Arc::new(Mutex::new(Vec::<ApiSendCommand>::new()));
    let long_session = AutoReplySession::new(
        ai.clone(),
        recorder(long_sent.clone(), |_, _| ready(confirmed())),
        Some(DailyReplyHistory::new(root.join("long-result-days"), None)),
        None,
    );
    let long_plan = AiResult {
        text: long_code.clone(),
        delegation: None,
        ..plan.clone()
    };
    long_session
        .send_reply(
            &room,
            &message("7", "2", "code fixture"),
            &long_plan,
            &none,
            "grok-result",
        )
        .await
        .unwrap();
    check.check(
        long_sent
            .lock()
            .unwrap()
            .last()
            .is_some_and(|c| c.message == bot_text(&long_code)),
        "Long coding results are sent intact without the former 8000-character auto-reply cap",
    );

    let uncertain_runner = Arc::new(Runner::default());
    let uncertain_ai = fakes::ai(reader.clone(), store.clone(), uncertain_runner.clone());
    let uncertain_root = root.join("uncertain-delegations");
    let failing: DelegationSend = Arc::new(|_, _, _, _, _| {
        Box::pin(async { Err(Error::invalid_operation("Uncertain send")) })
    });
    let jobs = BackgroundDelegations::new(
        uncertain_ai,
        failing,
        Arc::new(|_| {}),
        &none,
        Some(uncertain_root.clone()),
    );
    let outcome = jobs
        .start(&room, &message("6", "2", "uncertain fixture"), &plan)
        .await;
    jobs.dispose().await;
    let list = DelegationJobStore::new(Some(uncertain_root)).list();
    check.check(
        matches!(outcome, Err(Error::InvalidOperation(_)))
            && !uncertain_runner.started.is_cancelled()
            && list.len() == 1
            && list[0].status == "uncertain",
        "Uncertain acceptance never starts Grok or silently retries the original request",
    );

    let fail_runner = Arc::new(Runner {
        fail: true,
        ..Default::default()
    });
    let fail_ai = fakes::ai(reader.clone(), store.clone(), fail_runner);
    let failure_sent = CancellationToken::new();
    let failed_root = root.join("failed-delegations");
    let signal = failure_sent.clone();
    let notify: DelegationSend = Arc::new(move |_, _, result, phase, _| {
        if phase == "grok-error" && result.text.contains("완료하지 못") {
            signal.cancel();
        }
        Box::pin(async { Ok(()) })
    });
    let jobs = BackgroundDelegations::new(
        fail_ai.clone(),
        notify,
        Arc::new(|_| {}),
        &none,
        Some(failed_root.clone()),
    );
    jobs.start(&room, &message("5", "2", "failure fixture"), &plan)
        .await
        .unwrap();
    assert!(wait(&failure_sent, 5).await, "실패 안내 대기 시간 초과");
    let mut command = AiCommand::new(&room.profile, &room.id, "reply");
    command.target_message_id = Some("3".into());
    let casual = fail_ai
        .generate(&command, &none, None, false, true, None)
        .await
        .unwrap();
    jobs.dispose().await;
    let list = DelegationJobStore::new(Some(failed_root)).list();
    check.check(
        casual.text == "일상 대화 답변" && list.len() == 1 && list[0].status == "failed",
        "Grok failure reports only that task and leaves normal Codex conversation available",
    );
    check.check(
        matches!(
            kakaorelay_ai::grok_delegation::parse("bad [[delegate_grok:{}]]"),
            Err(Error::InvalidData(_))
        ),
        "Malformed delegation controls fail closed instead of leaking into chat",
    );
}
