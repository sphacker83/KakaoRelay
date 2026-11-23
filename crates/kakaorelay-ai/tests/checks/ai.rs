//! 원본: `AiChecks.cs`(끝의 `ResidentCliChecks` 호출은 `resident` 묶음으로 따로 실행한다).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::Timelike;
use kakaorelay_ai::ai_service::{AiCommand, AiService};
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::chat_models::{ChatContext, ChatReader, LocalMessage};
use kakaorelay_ai::cli_runner::{AiRunner, CliAiRunner};
use kakaorelay_ai::context_judge::ContextReplyJudge;
use kakaorelay_ai::conversation::AiConversation;
use kakaorelay_ai::image_cli::PersonaImageCliGenerator;
use kakaorelay_ai::imported_reader::ImportedChatReader;
use kakaorelay_ai::persona_images::PersonaImageService;
use kakaorelay_ai::settings::{
    AiProviderSettings, AiSettings, AiSettingsStore, BotPersona, ORDER, room_key,
};
use kakaorelay_ai::{Error, Result, json, model_catalog};

use crate::fakes::Runner;
use crate::support::{Check, fake_exe};

/// 세 번째 진입 때 알리고, 풀어 줄 때까지 기다리는 실행기.
struct BlockingRunner {
    entered: AtomicUsize,
    three: tokio::sync::Notify,
    release: CancellationToken,
}

#[async_trait]
impl AiRunner for BlockingRunner {
    async fn run(
        &self,
        _provider: &AiProviderSettings,
        _prompt: &str,
        _timeout: i32,
        cancel: &CancellationToken,
    ) -> Result<String> {
        if self.entered.fetch_add(1, Ordering::SeqCst) + 1 == 3 {
            self.three.notify_one();
        }
        tokio::select! {
            () = self.release.cancelled() => Ok("ok".into()),
            () = cancel.cancelled() => Err(Error::Cancelled),
        }
    }
}

fn command(profile: &str, room: &str) -> AiCommand {
    AiCommand::new(profile, room, "analyze")
}

pub async fn run(root: &Path, check: &Check) {
    let none = CancellationToken::new();
    let reader = Arc::new(ImportedChatReader::new());
    let room = reader
        .import("sample.txt", "대화 내보내기\n--------------- 2026년 9월 28일 월요일 ---------------\n[민수] [오전 12:03] 첫 줄\n두 번째 줄\n[영희] [오후 12:05] 금요일까지 확인해주세요.")
        .unwrap();
    let context = reader.read(&room.profile, &room.id, 100).await.unwrap();
    check.check(
        context.messages.len() == 2
            && context.messages[0].text == "첫 줄\n두 번째 줄"
            && context.messages[0].time.hour() == 0
            && context.messages[1].time.hour() == 12,
        "Portable TXT keeps multiline bodies and Korean AM/PM timestamps",
    );
    let mobile = reader
        .import(
            "mobile.txt",
            "2026년 9월 28일 오후 1:12, 민수 : 안녕하세요\n다음 줄",
        )
        .unwrap();
    let mobile_messages = reader
        .read(&mobile.profile, &mobile.id, 1)
        .await
        .unwrap()
        .messages;
    check.check(
        mobile_messages.len() == 1 && mobile_messages[0].text == "안녕하세요\n다음 줄",
        "Mobile Korean export format imports without inventing timestamps",
    );
    let json_room = reader
        .import("sample.json", "[{\"id\":\"9\",\"author\":\"A\",\"authorId\":\"1\",\"time\":\"2026-09-28T12:00:00+09:00\",\"text\":\"hello\",\"type\":1,\"deleted\":false}]")
        .unwrap();
    let json_messages = reader
        .read(&json_room.profile, &json_room.id, 1)
        .await
        .unwrap()
        .messages;
    check.check(
        json_messages.len() == 1 && json_messages[0].text == "hello",
        "Portable JSON adapter reads the API message schema",
    );
    let settings_path = root.join("ai-settings.json");
    let store = Arc::new(AiSettingsStore::new(&settings_path));
    let mut settings = AiSettings::default();
    settings.persona_mut().name = "테스트봇".into();
    settings.providers[1].model = "grok-example".into();
    store.save(&mut settings).unwrap();
    let loaded = store.load().unwrap();
    check.check(
        loaded.persona().name == "테스트봇" && loaded.providers[1].model == "grok-example",
        "Persona and provider-specific models persist atomically",
    );
    check.check(
        std::fs::read_to_string(&settings_path)
            .unwrap()
            .contains("테스트봇"),
        "Settings JSON stores Korean names directly without Unicode escapes",
    );
    settings.self_id = "self-test".into();
    settings.selected_bot_rooms = vec![room_key("p", "r")];
    settings.selected_chat_room = Some(room_key("p", "r"));
    store.save(&mut settings).unwrap();
    let persisted = AiSettingsStore::new(&settings_path).load().unwrap();
    check.check(
        persisted.self_id == "self-test"
            && persisted.trigger() == "@테스트봇"
            && persisted.selected_bot_rooms == settings.selected_bot_rooms
            && persisted.selected_chat_room == settings.selected_chat_room,
        "Bot identity, persona-derived trigger and selected rooms survive a new settings store",
    );
    check.check(
        root.join("ai-settings.json.bak").exists()
            && model_catalog::choices(&[], "custom-model").contains(&"custom-model".to_string()),
        "Settings backup and custom model choices are preserved",
    );
    let runner = Arc::new(Runner::with(|_, id| {
        if id == "codex" {
            Err(Error::invalid_operation("offline"))
        } else {
            Ok("금요일까지 확인할게요.".into())
        }
    }));
    let service = AiService::new(reader.clone(), store.clone(), runner.clone(), None, None);
    let result = service
        .generate(
            &command(&room.profile, &room.id),
            &none,
            None,
            false,
            false,
            None,
        )
        .await
        .unwrap();
    check.check(
        result.provider == "grok"
            && runner.ids() == ["codex", "grok"]
            && result.attempts.len() == 2,
        "Automatic provider fallback preserves Codex then Grok priority",
    );
    settings.provider = "codex".into();
    store.save(&mut settings).unwrap();
    runner.calls.lock().unwrap().clear();
    let result = service
        .generate(
            &command(&room.profile, &room.id),
            &none,
            None,
            false,
            false,
            None,
        )
        .await
        .unwrap();
    check.check(
        result.provider == "grok" && runner.ids() == ["codex", "grok"],
        "Explicit provider is the first choice and still falls back on failure",
    );
    settings.provider = "claude".into();
    store.save(&mut settings).unwrap();
    let second_round = Arc::new(Runner::with(|n, _| {
        if n < 5 {
            Err(Error::json("Malformed provider response"))
        } else {
            Ok("recovered".into())
        }
    }));
    let result = AiService::new(
        reader.clone(),
        store.clone(),
        second_round.clone(),
        None,
        None,
    )
    .generate(
        &command(&room.profile, &room.id),
        &none,
        None,
        false,
        false,
        None,
    )
    .await
    .unwrap();
    check.check(
        result.provider == "codex"
            && second_round.ids() == ["claude", "codex", "grok", "claude", "codex"],
        "Claude failure wraps to Codex and malformed responses can recover on the second round",
    );
    for selected in ["auto", "codex", "grok", "claude"] {
        settings.provider = selected.into();
        store.save(&mut settings).unwrap();
        let exhausted = Arc::new(Runner::with(|_, _| Err(Error::Cancelled)));
        let start = if selected == "auto" {
            0
        } else {
            ORDER.iter().position(|p| *p == selected).unwrap()
        };
        let expected: Vec<String> = (0..6).map(|i| ORDER[(start + i) % 3].to_string()).collect();
        let outcome = AiService::new(reader.clone(), store.clone(), exhausted.clone(), None, None)
            .generate(
                &command(&room.profile, &room.id),
                &none,
                None,
                false,
                false,
                None,
            )
            .await;
        check.check(
            matches!(outcome, Err(Error::Api(ref f)) if f.code == "ai_unavailable")
                && exhausted.ids() == expected,
            &format!("{selected} stops after exactly two full rounds of provider timeouts"),
        );
    }
    settings.provider = "claude".into();
    settings.provider_mut("grok").unwrap().enabled = false;
    store.save(&mut settings).unwrap();
    let disabled = Arc::new(Runner::with(|_, _| {
        Err(Error::invalid_operation("offline"))
    }));
    let outcome = AiService::new(reader.clone(), store.clone(), disabled.clone(), None, None)
        .generate(
            &command(&room.profile, &room.id),
            &none,
            None,
            false,
            false,
            None,
        )
        .await;
    check.check(
        matches!(outcome, Err(Error::Api(_)))
            && disabled.ids() == ["claude", "codex", "claude", "codex"],
        "Disabled providers are skipped in both rounds",
    );
    settings.provider_mut("grok").unwrap().enabled = true;
    store.save(&mut settings).unwrap();
    let stop_during_call = CancellationToken::new();
    let stopper = stop_during_call.clone();
    let interrupted = Arc::new(Runner::with(move |_, _| {
        stopper.cancel();
        Err(Error::Cancelled)
    }));
    let outcome = AiService::new(
        reader.clone(),
        store.clone(),
        interrupted.clone(),
        None,
        None,
    )
    .generate(
        &command(&room.profile, &room.id),
        &stop_during_call,
        None,
        false,
        false,
        None,
    )
    .await;
    check.check(
        outcome == Err(Error::Cancelled) && interrupted.ids() == ["claude"],
        "User stop during a call prevents all fallback and second-round attempts",
    );
    settings.provider = "auto".into();
    store.save(&mut settings).unwrap();
    runner.calls.lock().unwrap().clear();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let outcome = service
        .generate(
            &command(&room.profile, &room.id),
            &cancelled,
            None,
            false,
            false,
            None,
        )
        .await;
    check.check(
        outcome == Err(Error::Cancelled) && runner.calls() == 0,
        "Cancelled AI requests do not invoke a fallback provider",
    );
    let deleted = ChatContext {
        messages: vec![LocalMessage {
            text: "do not disclose".into(),
            deleted: true,
            ..context.messages[0].clone()
        }],
        ..context.clone()
    };
    let prompt = AiService::build_prompt(
        settings.persona(),
        &deleted,
        &command(&room.profile, &room.id),
        false,
        &[],
        true,
        false,
        None,
    );
    check.check(
        !prompt.contains("do not disclose")
            && prompt.contains("conversation")
            && prompt.contains("신뢰할 수 없는"),
        "Prompts redact deleted text and distinguish untrusted chat data from instructions",
    );
    let quoted_korean = "한글 그대로 \"따옴표\" \\경로\n다음 줄\t탭";
    let readable = json::prompt(quoted_korean);
    check.check(
        readable.contains("한글 그대로")
            && !readable.to_lowercase().contains("\\ud55c")
            && serde_json::from_str::<String>(&readable).unwrap() == quoted_korean,
        "Prompt JSON preserves Korean while correctly quoting newlines, quotes and backslashes",
    );
    let korean = ChatContext {
        messages: vec![LocalMessage {
            text: quoted_korean.into(),
            ..context.messages[0].clone()
        }],
        ..context.clone()
    };
    let korean_command = AiCommand {
        instruction: "한글 지시".into(),
        ..AiCommand::new(&room.profile, &room.id, "reply")
    };
    let korean_prompt = AiService::build_prompt(
        settings.persona(),
        &korean,
        &korean_command,
        false,
        &[],
        true,
        false,
        None,
    );
    let marker = "conversation: ";
    let conversation_json = &korean_prompt[korean_prompt.find(marker).unwrap() + marker.len()..];
    let parsed: serde_json::Value = serde_json::from_str(conversation_json).unwrap();
    check.check(
        korean_prompt.contains("테스트봇") && korean_prompt.contains("한글 지시") && parsed[0]["text"] == quoted_korean,
        "Conversation prompts retain readable Korean persona, instructions and exact message structure",
    );
    check.check(
        ContextReplyJudge::prompt(settings.persona(), &korean.messages).contains("한글 그대로")
            && PersonaImageCliGenerator::build_prompt(
                "codex",
                &PersonaImageService::reference_prompt(settings.persona(), "마젠타 배경"),
                None,
            )
            .contains("마젠타 배경"),
        "Reply judging and nested image generation prompts also preserve Korean",
    );
    let provider = |id: &str| AiProviderSettings {
        model: "test-model".into(),
        effort: "high".into(),
        ..AiProviderSettings::new(id)
    };
    check.check(
        CliAiRunner::arguments(&provider("codex"), root)
            .unwrap()
            .contains(&"model_reasoning_effort=\"high\"".to_string()),
        "Codex effort is a separate TOML config argument",
    );
    check.check(
        CliAiRunner::arguments(&AiProviderSettings::new("claude"), root)
            .unwrap()
            .contains(&"WebSearch,WebFetch".to_string())
            && CliAiRunner::arguments(&AiProviderSettings::new("grok"), root)
                .unwrap()
                .contains(&"--prompt-file".to_string()),
        "Claude web tools are enabled and Grok prompts stay off command line",
    );
    check.check(
        CliAiRunner::parse_json_output("{\"text\":\"한글\"}").unwrap() == "한글"
            && CliAiRunner::parse_json_output("{\"result\":\"OK\",\"is_error\":false}").unwrap()
                == "OK",
        "Grok and Claude final JSON responses parse",
    );
    check.check(
        matches!(
            CliAiRunner::parse_json_output("{\"result\":\"error\",\"is_error\":true}"),
            Err(Error::InvalidOperation(_))
        ),
        "Provider error payloads cannot become chatbot answers",
    );
    let native = CliAiRunner::new();
    let fixture = AiProviderSettings {
        executable: fake_exe(),
        ..AiProviderSettings::new("codex")
    };
    let text = native
        .run(
            &fixture,
            "fixture 한국어 $(do-not-run) & quote \"",
            10,
            &none,
        )
        .await
        .unwrap();
    check.check(
        text == "fixture 한국어 $(do-not-run) & quote \"",
        "CLI subprocess passes Unicode and shell metacharacters through stdin literally",
    );
    let stop = CancellationToken::new();
    let stopper = stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        stopper.cancel();
    });
    check.check(
        native.run(&fixture, "HANG", 10, &stop).await == Err(Error::Cancelled),
        "Cancellation terminates a hung CLI process",
    );
    check.check(
        native.run(&fixture, "HANG", 1, &none).await == Err(Error::Cancelled),
        "Timeout terminates a hung CLI process",
    );
    check.check(
        !std::fs::read_to_string(&settings_path)
            .unwrap()
            .contains("첫 줄"),
        "Settings never persist conversation content",
    );
    let blocking = Arc::new(BlockingRunner {
        entered: AtomicUsize::new(0),
        three: tokio::sync::Notify::new(),
        release: CancellationToken::new(),
    });
    let parallel = Arc::new(AiService::new(
        reader.clone(),
        store.clone(),
        blocking.clone(),
        None,
        None,
    ));
    let spawn = |profile: String, room: String, cancel: CancellationToken| {
        let parallel = parallel.clone();
        tokio::spawn(async move {
            parallel
                .generate(&command(&profile, &room), &cancel, None, false, false, None)
                .await
        })
    };
    let a = spawn(room.profile.clone(), room.id.clone(), none.clone());
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let same = spawn(room.profile.clone(), room.id.clone(), none.clone());
    let b = spawn(mobile.profile.clone(), mobile.id.clone(), none.clone());
    let c = spawn(
        json_room.profile.clone(),
        json_room.id.clone(),
        none.clone(),
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        blocking.three.notified(),
    )
    .await
    .unwrap();
    check.check(
        blocking.entered.load(Ordering::SeqCst) == 3 && !same.is_finished(),
        "Three rooms run concurrently while a second call in the same room waits",
    );
    let queued_cancel = CancellationToken::new();
    let cancelled_queue = spawn(room.profile.clone(), room.id.clone(), queued_cancel.clone());
    queued_cancel.cancel();
    check.check(
        cancelled_queue.await.unwrap() == Err(Error::Cancelled)
            && blocking.entered.load(Ordering::SeqCst) == 3,
        "Cancelling a queued room request never launches its CLI",
    );
    blocking.release.cancel();
    for task in [a, same, b, c] {
        task.await.unwrap().unwrap();
    }
    check.check(
        blocking.entered.load(Ordering::SeqCst) == 4,
        "Repeated room call runs after its predecessor completes",
    );
    let session_id = uuid::Uuid::new_v4().to_string();
    for id in ORDER {
        let arguments = CliAiRunner::conversation_arguments(
            &AiProviderSettings::new(id),
            root,
            Some(&session_id),
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        let has = |x: &str| arguments.iter().any(|a| a == x);
        check.check(
            has(&session_id)
                && !has("--ephemeral")
                && !has("--no-session-persistence")
                && has(if id == "codex" { "resume" } else { "--resume" }),
            &format!(
                "{id} resumes an explicit persisted session, never the globally latest session"
            ),
        );
    }
    let p = AiProviderSettings::default();
    let persona = BotPersona::default();
    check.check(
        AiConversation::folder(&p, &persona, &command("profile", "room1")).unwrap()
            != AiConversation::folder(&p, &persona, &command("profile", "room2")).unwrap(),
        "Session storage is isolated by room",
    );
    let conversation = AiConversation {
        session_id: Some(session_id.clone()),
        seen: context
            .messages
            .iter()
            .map(AiConversation::fingerprint)
            .collect(),
        ..Default::default()
    };
    let session_folder = root.join("session");
    conversation.save(&session_folder).unwrap();
    let restored = AiConversation::load(&session_folder, None);
    let edited = LocalMessage {
        text: "edited".into(),
        ..context.messages[0].clone()
    };
    check.check(
        restored.session_id.as_deref() == Some(session_id.as_str())
            && context
                .messages
                .iter()
                .all(|m| restored.seen.contains(&AiConversation::fingerprint(m)))
            && !restored
                .seen
                .contains(&AiConversation::fingerprint(&edited)),
        "Resume metadata persists and detects changed messages without storing plaintext",
    );
}
