//! 원본: `ReplyModeChecks.cs`. 자동 답장 세션·후보 검사 4개(`AutoReplySession`)는 `kakaorelay-core/tests/checks/auto_reply.rs`에 있다.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::chat_models::LocalMessage;
use kakaorelay_ai::cli_runner::AiRunner;
use kakaorelay_ai::context_judge::ContextReplyJudge;
use kakaorelay_ai::settings::{AiProviderSettings, AiSettings, AiSettingsStore, room_key};
use kakaorelay_ai::{Error, Result};

use crate::support::Check;

#[derive(Default)]
struct Runner {
    calls: Mutex<Vec<String>>,
    fail_grok: Mutex<bool>,
    invalid: Mutex<bool>,
}

#[async_trait]
impl AiRunner for Runner {
    async fn run(
        &self,
        p: &AiProviderSettings,
        _prompt: &str,
        _seconds: i32,
        cancel: &CancellationToken,
    ) -> Result<String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{}:{}:{}", p.id, p.model, p.effort));
        let delay = std::time::Duration::from_millis(if p.id == "grok" { 10 } else { 60 });
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = cancel.cancelled() => return Err(Error::Cancelled),
        }
        if *self.fail_grok.lock().unwrap() && p.id == "grok" {
            return Err(Error::invalid_operation("offline"));
        }
        Ok(if *self.invalid.lock().unwrap() {
            "unclear"
        } else {
            "SKIP"
        }
        .into())
    }
}

pub async fn run(root: &Path, check: &Check) {
    let none = CancellationToken::new();
    let mut settings = AiSettings::default();
    settings.poll_seconds = 1;
    settings.reply_mode = "context".into();
    let store = AiSettingsStore::new(root.join("reply-mode.json"));
    store.save(&mut settings).unwrap();
    let loaded = store.load().unwrap();
    check.check(
        loaded.poll_seconds == 1 && loaded.reply_mode == "context",
        "Polling interval and reply mode persist",
    );
    settings
        .room_reply_modes
        .insert(room_key("one", "10"), "immediate".into());
    settings
        .room_reply_modes
        .insert(room_key("two", "10"), "trigger".into());
    store.save(&mut settings).unwrap();
    let restored = store.load().unwrap();
    check.check(
        restored.reply_mode_for_room("one", "10") == "immediate" && restored.reply_mode_for_room("two", "10") == "trigger" && restored.reply_mode_for_room("one", "11") == "context",
        "Room reply modes persist independently by profile and room; legacy rooms retain their previous mode",
    );
    let mut invalid_modes = settings.clone();
    invalid_modes
        .room_reply_modes
        .insert(room_key("one", "10"), "bad".into());
    check.check(
        matches!(invalid_modes.validate(), Err(Error::Argument(_))),
        "Invalid room reply modes cannot be saved",
    );
    for invalid in [0, 301] {
        settings.poll_seconds = invalid;
        check.check(
            matches!(settings.validate(), Err(Error::Argument(_))),
            "Reject polling interval outside 1–300 seconds",
        );
    }
    settings.poll_seconds = 1;
    let messages: Vec<LocalMessage> = (1..=25)
        .map(|i| {
            LocalMessage::new(
                i.to_string(),
                "2",
                "user",
                chrono::Utc::now().fixed_offset(),
                1,
                format!("message-{i}"),
                false,
            )
        })
        .collect();
    let prompt = ContextReplyJudge::prompt(settings.persona(), &messages);
    check.check(
        !prompt.contains("message-5\"")
            && prompt.contains("message-6\"")
            && prompt.contains("message-25\""),
        "Context judge receives exactly the most recent 20 messages",
    );
    let runner = Arc::new(Runner::default());
    let judge = ContextReplyJudge::new(runner.clone());
    let persona = settings.persona().clone();
    check.check(
        !judge
            .should_reply(&settings, &persona, &messages, &none, None)
            .await
            .unwrap()
            && judge.preferred_provider().as_deref() == Some("grok"),
        "Fastest valid judgment selects Grok and honors SKIP",
    );
    let count = runner.calls.lock().unwrap().len();
    judge
        .should_reply(&settings, &persona, &messages, &none, None)
        .await
        .unwrap();
    {
        let calls = runner.calls.lock().unwrap();
        check.check(
            calls.len() == count + 1
                && calls
                    .iter()
                    .all(|s| s == "codex:gpt-6-luna:low" || s == "grok:grok-4.7:low"),
            "Subsequent judgments reuse the selected exact model with low effort",
        );
    }
    *runner.fail_grok.lock().unwrap() = true;
    judge
        .should_reply(&settings, &persona, &messages, &none, None)
        .await
        .unwrap();
    check.check(
        judge.preferred_provider().as_deref() == Some("codex"),
        "Failed preferred judgment falls back to the other model",
    );
    *runner.invalid.lock().unwrap() = true;
    check.check(
        matches!(
            judge
                .should_reply(&settings, &persona, &messages, &none, None)
                .await,
            Err(Error::InvalidOperation(_))
        ),
        "Invalid judgments cannot authorize a reply",
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    check.check(
        judge
            .should_reply(&settings, &persona, &messages, &cancelled, None)
            .await
            == Err(Error::Cancelled),
        "Cancelled judgment does not return a reply decision",
    );
}
