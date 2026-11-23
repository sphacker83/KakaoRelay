//! 원본: `InstructionChecks.cs`. 실제 상주 실행기와 가짜 CLI로 지시문 일일 재사용을 확인한다.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{Duration, NaiveDate};
use kakaorelay_ai::ai_service::{AiCommand, AiResult, AiService, Clock};
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::chat_models::{ApiFailure, ChatContext, ChatReader, LocalMessage, LocalRoom};
use kakaorelay_ai::cli_runner::CliAiRunner;
use kakaorelay_ai::conversation::AiConversation;
use kakaorelay_ai::settings::{AiSettings, AiSettingsStore};

use crate::support::{Check, fake_exe};

struct FixedClock(Mutex<NaiveDate>);

impl Clock for FixedClock {
    fn today(&self) -> NaiveDate {
        *self.0.lock().unwrap()
    }
}

struct Reader {
    room: LocalRoom,
    messages: Mutex<Vec<LocalMessage>>,
}

#[async_trait]
impl ChatReader for Reader {
    async fn rooms(&self) -> Result<Vec<LocalRoom>, ApiFailure> {
        Ok(vec![self.room.clone()])
    }
    async fn read(
        &self,
        _profile: &str,
        _room_id: &str,
        _limit: usize,
    ) -> Result<ChatContext, ApiFailure> {
        Ok(ChatContext {
            room: self.room.clone(),
            messages: self.messages.lock().unwrap().clone(),
            read_at: chrono::Utc::now().fixed_offset(),
        })
    }
}

fn prompt(result: &AiResult) -> String {
    serde_json::from_str::<serde_json::Value>(&result.text).unwrap()["prompt"]
        .as_str()
        .unwrap()
        .to_string()
}

fn message(id: &str, text: &str) -> LocalMessage {
    LocalMessage::new(
        id,
        "user",
        "fixture",
        chrono::Utc::now().fixed_offset(),
        1,
        text,
        false,
    )
}

pub async fn run(root: &Path, check: &Check) {
    let none = CancellationToken::new();
    for id in ["codex", "claude"] {
        let reader = Arc::new(Reader {
            room: LocalRoom::new(
                "fixture",
                uuid::Uuid::new_v4().simple().to_string(),
                "Fixture",
                true,
            ),
            messages: Mutex::new(vec![message("1", "first-context")]),
        });
        let clock = Arc::new(FixedClock(Mutex::new(
            NaiveDate::from_ymd_opt(2026, 9, 29).unwrap(),
        )));
        let store = Arc::new(AiSettingsStore::new(
            root.join(format!("daily-instructions-{id}.json")),
        ));
        let mut settings = AiSettings::default();
        settings.provider = id.into();
        settings.persona_mut().name = "instruction-fixture".into();
        settings.persona_mut().instructions = "unique-persona-rule".into();
        for provider in &mut settings.providers {
            provider.enabled = provider.id == id;
            provider.executable = fake_exe();
        }
        store.save(&mut settings).unwrap();
        let mut command = AiCommand {
            instruction: "first-request".into(),
            target_message_id: Some("1".into()),
            ..AiCommand::new(&reader.room.profile, &reader.room.id, "reply")
        };
        let mut runner = Arc::new(CliAiRunner::new());
        let mut service = AiService::new(
            reader.clone(),
            store.clone(),
            runner.clone(),
            None,
            Some(clock.clone()),
        );
        macro_rules! ask {
            ($command:expr) => {
                prompt(
                    &service
                        .generate(&$command, &none, None, false, false, None)
                        .await
                        .unwrap(),
                )
            };
        }
        let first = ask!(command);
        reader
            .messages
            .lock()
            .unwrap()
            .push(message("2", "new-context"));
        command = AiCommand {
            instruction: "second-request".into(),
            target_message_id: Some("2".into()),
            ..command
        };
        let second = ask!(command);
        check.check(
            first.contains("unique-persona-rule")
                && first.contains("[[emotion:")
                && !second.contains("unique-persona-rule")
                && !second.contains("[[emotion:")
                && second.contains("second-request")
                && second.contains("new-context")
                && !second.contains("first-context"),
            &format!("{id} sends persona and fixed instructions once, then only the current request and changed context"),
        );
        runner.dispose();
        runner = Arc::new(CliAiRunner::new());
        service = AiService::new(
            reader.clone(),
            store.clone(),
            runner.clone(),
            None,
            Some(clock.clone()),
        );
        check.check(
            !ask!(command).contains("unique-persona-rule"),
            &format!("{id} same-day restart resumes without reinjecting persona"),
        );
        {
            let mut day = clock.0.lock().unwrap();
            *day += Duration::days(1);
        }
        check.check(
            ask!(command).contains("unique-persona-rule")
                && !ask!(command).contains("unique-persona-rule"),
            &format!("{id} refreshes instructions once on the next local day"),
        );
        settings.persona_mut().instructions = "changed-persona-rule".into();
        store.save(&mut settings).unwrap();
        check.check(
            ask!(command).contains("changed-persona-rule"),
            &format!("{id} persona edits initialize a fresh session immediately"),
        );
        let analyze = AiCommand {
            mode: "analyze".into(),
            ..command.clone()
        };
        check.check(
            ask!(analyze).contains("핵심 요약") && !ask!(analyze).contains("changed-persona-rule"),
            &format!("{id} task-mode changes refresh their rules once"),
        );
        let folder =
            AiConversation::folder(settings.provider(id).unwrap(), settings.persona(), &command)
                .unwrap();
        let state = AiConversation::load(&folder, None);
        check.check(
            state.instructions_date == Some(*clock.0.lock().unwrap())
                && state.instructions_revision.is_some(),
            &format!("{id} persists instruction date and revision with session metadata"),
        );
        AiConversation::default().save(&folder).unwrap();
        check.check(
            ask!(command).contains("changed-persona-rule"),
            &format!("{id} interrupted or lost sessions receive full instructions again"),
        );
        runner.dispose();
    }
    let legacy = AiConversation {
        session_id: Some("legacy".into()),
        ..Default::default()
    };
    check.check(
        legacy.needs_instructions(NaiveDate::from_ymd_opt(2026, 9, 29).unwrap(), "v1"),
        "Older session metadata gets one initial instruction refresh",
    );
}
