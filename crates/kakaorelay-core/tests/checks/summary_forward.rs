//! 원본: `SummaryForwardChecks.cs`(24개).

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::cli_runner::AiRunner;
use kakaorelay_ai::settings::{AiProviderSettings, AiSettingsStore, room_key};
use kakaorelay_ai::summary::{SummaryDigest, SummaryEvidence, SummaryUpdate};
use kakaorelay_ai::summary_forward_settings::{SummaryForwardRule, SummaryRoom};
use kakaorelay_ai::{ApiFailure, ChatContext, ChatReader, LocalMessage, LocalRoom, Result};
use kakaorelay_core::api_send::ApiSendCommand;
use kakaorelay_core::platform::ConversationTarget;
use kakaorelay_core::summary_forwarder::{ForwardState, SummaryForwarder};
use kakaorelay_core::test_send::TestSendReceipt;

use crate::fakes::FixedClock;
use crate::support::Check;

struct Reader {
    rooms: Mutex<Vec<LocalRoom>>,
    messages: Mutex<Vec<LocalMessage>>,
    failed: Mutex<HashSet<String>>,
}

#[async_trait]
impl ChatReader for Reader {
    async fn rooms(&self) -> std::result::Result<Vec<LocalRoom>, ApiFailure> {
        Ok(self.rooms.lock().unwrap().clone())
    }
    async fn read(
        &self,
        _: &str,
        room_id: &str,
        limit: usize,
    ) -> std::result::Result<ChatContext, ApiFailure> {
        if self.failed.lock().unwrap().contains(room_id) {
            return Err(ApiFailure::new(500, "io", "test room unavailable"));
        }
        let room = self
            .rooms
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.id == room_id)
            .unwrap()
            .clone();
        let messages = self.messages.lock().unwrap();
        let start = messages.len().saturating_sub(limit);
        Ok(ChatContext {
            room,
            messages: messages[start..].to_vec(),
            read_at: chrono::Utc::now().fixed_offset(),
        })
    }
    async fn read_attachment(
        &self,
        message: LocalMessage,
    ) -> std::result::Result<LocalMessage, ApiFailure> {
        Ok(if message.kind == 18 {
            LocalMessage {
                text: format!("{}\n첨부 결정: 금요일 제출", message.text),
                ..message
            }
        } else {
            message
        })
    }
}

#[derive(Default)]
struct Runner {
    calls: AtomicUsize,
    result: Mutex<Option<SummaryDigest>>,
    before_result: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

#[async_trait]
impl AiRunner for Runner {
    async fn run(
        &self,
        _: &AiProviderSettings,
        _: &str,
        _: i32,
        _: &CancellationToken,
    ) -> Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = self.before_result.lock().unwrap().as_ref() {
            hook();
        }
        let digest = self
            .result
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(SummaryDigest {
                items: Some(vec![]),
            });
        Ok(serde_json::to_string(&digest).unwrap())
    }
}

fn digest(topic: &str, source: &str, id: &str, text: &str) -> SummaryDigest {
    SummaryDigest {
        items: Some(vec![SummaryUpdate {
            topic: topic.into(),
            text: text.into(),
            evidence: Some(vec![SummaryEvidence {
                source: source.into(),
                message_id: id.into(),
                quote: text.into(),
            }]),
        }]),
    }
}

pub async fn run(root: &Path, check: &Check) {
    let reader = Arc::new(Reader {
        rooms: Mutex::new(vec![
            LocalRoom::new("test", "1", "원본", true),
            LocalRoom::new("test", "2", "수신", true),
        ]),
        messages: Mutex::new(vec![]),
        failed: Mutex::new(HashSet::new()),
    });
    let runner = Arc::new(Runner::default());
    let clock = FixedClock::utc(2026, 10, 20, 1, 0);
    let store = Arc::new(AiSettingsStore::new(root.join("summary-settings.json")));
    let mut settings = store.load().unwrap();
    let source = SummaryRoom::new("test", "1", "원본");
    settings.summary_forwards = vec![SummaryForwardRule {
        enabled: true,
        interval_minutes: 1,
        sources: vec![source.clone()],
        destination: Some(SummaryRoom::new("test", "2", "수신")),
        ..Default::default()
    }];
    settings.selected_bot_rooms = vec![source.key()];
    for p in &mut settings.providers {
        p.enabled = p.id == "codex";
    }
    store.save(&mut settings).unwrap();
    let ai = crate::fakes::ai(reader.clone(), store.clone(), runner.clone());
    let sent = Arc::new(Mutex::new(Vec::<ApiSendCommand>::new()));
    let uncertain = Arc::new(AtomicBool::new(false));
    let open = Arc::new(AtomicBool::new(true));
    let state = root.join("summary-state");
    let forwarder = || {
        let (sent, uncertain, open) = (sent.clone(), uncertain.clone(), open.clone());
        SummaryForwarder::new(
            ai.clone(),
            Arc::new(move |command: ApiSendCommand, _| {
                sent.lock().unwrap().push(command.clone());
                let receipt = TestSendReceipt {
                    request_id: command.request_id,
                    enter_posted: true,
                    input_cleared: Some(!uncertain.load(Ordering::SeqCst)),
                    ..Default::default()
                };
                Box::pin(async move { Ok(receipt) })
            }),
            Arc::new(move || {
                Ok(if open.load(Ordering::SeqCst) {
                    vec![ConversationTarget {
                        title: "수신".into(),
                        process_id: 1,
                        handle: "0x10".into(),
                        ambiguous: false,
                        room_id: None,
                    }]
                } else {
                    vec![]
                })
            }),
            Some(state.clone()),
            Some(clock.clone()),
        )
    };
    let add = |id: &str, text: &str| {
        let now = kakaorelay_core::support::Clock::now(clock.as_ref());
        reader.messages.lock().unwrap().push(LocalMessage::new(
            id,
            "human",
            "작성자",
            now,
            1,
            text,
            false,
        ));
    };
    let result = |id: &str, text: &str| {
        *runner.result.lock().unwrap() = Some(digest("일정", &source.key(), id, text))
    };
    let count = || sent.lock().unwrap().len();
    let calls = || runner.calls.load(Ordering::SeqCst);
    let advance = || clock.advance(chrono::Duration::minutes(1));
    let none = CancellationToken::new();
    let renew = |settings: &mut kakaorelay_ai::settings::AiSettings| {
        settings.summary_forwards[0].id = uuid::Uuid::new_v4().simple().to_string();
        store.save(settings).unwrap();
    };

    let mut f = forwarder();
    add("1", "기존 대화");
    f.tick(&none).await.unwrap();
    check.check(
        count() == 0 && calls() == 0,
        "Summary forwarding baselines existing messages without generating or sending",
    );
    add("2", "금요일 확정");
    result("2", "금요일 확정");
    f.tick(&none).await.unwrap();
    check.check(
        count() == 0 && calls() == 0,
        "Summary forwarding respects the configured interval",
    );
    advance();
    f.tick(&none).await.unwrap();
    {
        let list = sent.lock().unwrap();
        check.check(
            list.len() == 1
                && list[0].recipient == "수신"
                && list[0].message.contains("금요일 확정")
                && list[0].message.contains("출처: 원본"),
            "New evidenced updates are summarized and forwarded only to the configured recipient",
        );
    }
    f = forwarder();
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == 1 && calls() == 1,
        "Restart retains the cursor and no new messages cause no model calls or sends",
    );
    add("3", "금요일 확정");
    result("3", "금요일 확정");
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == 1,
        "Repeated current facts do not produce duplicate notifications",
    );
    add("4", "안녕하세요");
    *runner.result.lock().unwrap() = Some(SummaryDigest {
        items: Some(vec![]),
    });
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == 1,
        "New chatter without substantive additions advances the cursor without sending",
    );
    add("5", "월요일 변경");
    result("5", "월요일 변경");
    open.store(false, Ordering::SeqCst);
    advance();
    f.tick(&none).await.unwrap();
    let before = calls();
    check.check(
        count() == 1,
        "Closed recipient keeps the prepared summary without dispatching",
    );
    f = forwarder();
    open.store(true, Ordering::SeqCst);
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == 2
            && calls() == before
            && sent.lock().unwrap()[1].message.contains("월요일 변경"),
        "A retained summary resumes after restart when its recipient opens without regenerating",
    );
    add("6", "금요일 확정");
    result("6", "금요일 확정");
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == 3,
        "A genuine change back to an earlier state can be reported again",
    );
    add("7", "화요일 변경");
    result("7", "화요일 변경");
    uncertain.store(true, Ordering::SeqCst);
    advance();
    f.tick(&none).await.unwrap();
    let attempts = count();
    f = forwarder();
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == attempts,
        "Uncertain dispatch survives restart and is never automatically resent",
    );
    renew(&mut settings);
    uncertain.store(false, Ordering::SeqCst);
    f.tick(&none).await.unwrap();
    check.check(
        count() == attempts,
        "Re-enabled fresh rule baselines instead of replaying an uncertain past send",
    );
    add("8", "수요일 확정");
    result("8", "조작된 인용");
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == attempts,
        "Unverifiable model evidence cannot cause a send",
    );
    result("8", "수요일 확정");
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == attempts + 1,
        "Failed generation preserves the cursor so the new facts can be retried",
    );
    settings.selected_bot_rooms.clear();
    store.save(&mut settings).unwrap();
    add("9", "목요일 변경");
    result("9", "목요일 변경");
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == attempts + 2,
        "Summary monitoring continues with no automatic-reply rooms selected",
    );
    add("10", "토요일 변경");
    result("10", "토요일 변경");
    let hook_store = store.clone();
    *runner.before_result.lock().unwrap() = Some(Box::new(move || {
        let mut current = hook_store.load().unwrap();
        current.summary_forwards[0].enabled = false;
        hook_store.save(&mut current).unwrap();
    }));
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == attempts + 2,
        "Disabling a rule while its summary is being generated prevents dispatch",
    );
    *runner.before_result.lock().unwrap() = None;
    settings.summary_forwards[0].enabled = true;
    renew(&mut settings);
    f.tick(&none).await.unwrap();
    reader.messages.lock().unwrap().clear();
    for n in 20..520 {
        add(&n.to_string(), "대화");
    }
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == attempts + 2,
        "A source exceeding the 500-message read window fails without silently advancing or sending",
    );
    let invalid = SummaryForwardRule {
        sources: vec![source.clone()],
        destination: Some(source.clone()),
        ..Default::default()
    };
    reader.messages.lock().unwrap().clear();
    renew(&mut settings);
    f.tick(&none).await.unwrap();
    let now = kakaorelay_core::support::Clock::now(clock.as_ref());
    reader.messages.lock().unwrap().push(LocalMessage::new(
        "600",
        "human",
        "작성자",
        now,
        18,
        "회의록.pdf",
        false,
    ));
    result("600", "첨부 결정: 금요일 제출");
    let before_file = count();
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == before_file + 1
            && sent
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .message
                .contains("금요일 제출"),
        "Summary monitoring extracts file contents and validates quotes against extracted text",
    );
    // 실패한 원본 방이 정상 방을 막거나 저장된 커서를 잃게 하면 안 된다.
    reader
        .rooms
        .lock()
        .unwrap()
        .push(LocalRoom::new("test", "3", "오류방", true));
    settings.summary_forwards[0]
        .sources
        .push(SummaryRoom::new("test", "3", "오류방"));
    renew(&mut settings);
    f.tick(&none).await.unwrap();
    reader.failed.lock().unwrap().insert("3".into());
    add("601", "일요일 변경");
    result("601", "일요일 변경");
    let before_partial = count();
    let status = Arc::new(Mutex::new(String::new()));
    let observed = status.clone();
    f.status
        .subscribe(move |s| *observed.lock().unwrap() = s.to_string());
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == before_partial + 1 && status.lock().unwrap().contains("오류방"),
        "A failed room is named while healthy room updates are delivered",
    );
    let failed_key = room_key("test", "3");
    let partial = std::fs::read_dir(&state)
        .unwrap()
        .filter_map(|e| {
            let path = e.unwrap().path();
            (path.extension().is_some_and(|x| x == "json")).then(|| {
                ForwardState::from_json(&std::fs::read_to_string(path).unwrap())
                    .unwrap()
                    .unwrap()
            })
        })
        .filter(|s| s.source_errors.contains_key(&failed_key))
        .collect::<Vec<_>>();
    check.check(
        partial.len() == 1
            && partial[0].cursors.get(&source.key()).map(String::as_str) == Some("601")
            && partial[0].cursors.get(&failed_key).map(String::as_str) == Some("600"),
        "Failed room cursor stays unchanged while healthy room advances",
    );
    reader.failed.lock().unwrap().clear();
    *runner.result.lock().unwrap() = Some(SummaryDigest {
        items: Some(vec![SummaryUpdate {
            topic: "복구".into(),
            text: "일요일 변경".into(),
            evidence: Some(vec![SummaryEvidence {
                source: failed_key.clone(),
                message_id: "601".into(),
                quote: "일요일 변경".into(),
            }]),
        }]),
    });
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == before_partial + 2,
        "Recovered room resumes unread messages from its retained cursor",
    );
    renew(&mut settings);
    reader.failed.lock().unwrap().insert("3".into());
    f.tick(&none).await.unwrap();
    add("602", "다음 주 확정");
    result("602", "다음 주 확정");
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == before_partial + 3,
        "A source unavailable at initial baseline does not block healthy source updates",
    );
    reader.failed.lock().unwrap().clear();
    *runner.result.lock().unwrap() = Some(SummaryDigest {
        items: Some(vec![]),
    });
    advance();
    f.tick(&none).await.unwrap();
    check.check(
        count() == before_partial + 3,
        "First successful read of an unavailable source baselines without replaying history",
    );
    check.check(
        matches!(invalid.validate(), Err(kakaorelay_ai::Error::Argument(_))),
        "A forwarding rule cannot send summaries back to its own source room",
    );
}
