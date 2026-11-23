//! 원본: `AutoKnowledgeChecks.cs`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use kakaorelay_ai::auto_knowledge::{
    AutoKnowledgeService, Checkpoint, KnowledgeClaim, KnowledgeDigest, KnowledgeEvidence, Pending,
};
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::chat_models::LocalRoom;
use kakaorelay_ai::conversation::AiConversation;
use kakaorelay_ai::settings::{
    AiSettings, AiSettingsStore, AutoKnowledgeSettings, KnowledgeRoomSubscription,
    KnowledgeSettings, ORDER,
};
use kakaorelay_ai::{Error, documents, json, obsidian};

use crate::fakes::{Reader, Runner, message};
use crate::support::Check;

fn digest(claims: Vec<KnowledgeClaim>) -> String {
    serde_json::to_string(&KnowledgeDigest { items: claims }).unwrap()
}

fn claim(topic: &str, text: &str, id: i64, quote: &str, status: &str) -> KnowledgeClaim {
    KnowledgeClaim::new(
        topic,
        text,
        "knowledge",
        status,
        vec![KnowledgeEvidence::new(&id.to_string(), quote)],
    )
}

fn md_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
                .count()
        })
        .unwrap_or(0)
}

fn md_files(dir: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect()
}

fn watch(service: &AutoKnowledgeService) -> Arc<Mutex<String>> {
    let status = Arc::new(Mutex::new(String::new()));
    let sink = status.clone();
    service.on_status(move |s| *sink.lock().unwrap() = s.to_string());
    status
}

fn room() -> LocalRoom {
    LocalRoom::new("account", "room", "지식 테스트", true)
}

fn settings_for(vault: &Path, enabled: bool, title: &str) -> AiSettings {
    let mut settings = AiSettings::default();
    settings.self_id = "self".into();
    settings.ensure_personas();
    settings.personas[0].knowledge = KnowledgeSettings {
        enabled,
        vault_path: vault.to_string_lossy().into_owned(),
    };
    settings.auto_knowledge = AutoKnowledgeSettings {
        enabled: true,
        rooms: vec![KnowledgeRoomSubscription::new(
            "account",
            "room",
            title,
            uuid::Uuid::new_v4(),
        )],
        ..Default::default()
    };
    settings
}

pub async fn run(root: &Path, check: &Check) {
    let none = CancellationToken::new();
    let area = root.join("auto-knowledge");
    let vault = area.join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let state = area.join("state");
    let reader = Arc::new(Reader::new(room(), true));
    let runner = Arc::new(Runner::new("{\"items\":[]}"));
    let mut settings = settings_for(&vault, true, "지식 테스트");
    let store = AiSettingsStore::new(area.join("settings.json"));
    store.save(&mut settings).unwrap();
    let loaded = store.load().unwrap();
    check.check(
        loaded.auto_knowledge.rooms[0].generation == settings.auto_knowledge.rooms[0].generation
            && loaded.auto_knowledge.interval_minutes == 30,
        "Automatic knowledge settings and subscription generation persist",
    );
    let service = AutoKnowledgeService::new(reader.clone(), runner.clone(), Some(state.clone()));
    let status = watch(&service);
    let status = || status.lock().unwrap().clone();
    reader.push(message(1, "과거 대화", "person"));
    service.tick(&settings, true, &none).await.unwrap();
    check.check(
        *reader.discovered.lock().unwrap(),
        "Startup automation discovers local rooms without requiring a manual refresh",
    );
    check.check(
        runner.calls() == 0 && !vault.join("자동정리").exists(),
        "Enabling knowledge automation establishes a baseline without summarizing history",
    );
    reader.push(message(2, "휴가는 사흘 전에 신청합니다.", "person"));
    reader.push(message(3, "BOT-SECRET", "self"));
    reader.push(message(4, "회의는 금요일일지도 몰라요.", "person"));
    runner.set_text(digest(vec![
        claim(
            "휴가 신청",
            "휴가는 사흘 전에 신청합니다.",
            2,
            "휴가는 사흘 전에 신청합니다.",
            "confirmed",
        ),
        claim(
            "회의 일정",
            "회의는 금요일일 가능성이 있습니다.",
            4,
            "금요일일지도 몰라요.",
            "review",
        ),
    ]));
    service.tick(&settings, false, &none).await.unwrap();
    check.check(
        runner.calls() == 0,
        "A scheduled check waits for the configured interval",
    );
    service.tick(&settings, true, &none).await.unwrap();
    let accepted = md_files(&vault.join("02-업무지식"));
    let reviews = md_files(&vault.join("검토필요"));
    let prompt = runner.prompt();
    check.check(
        accepted.len() == 1
            && reviews.len() == 1
            && std::fs::read_to_string(&accepted[0]).unwrap().contains("메시지 2")
            && !prompt.contains("BOT-SECRET")
            && !prompt.contains("과거 대화"),
        "Only new human text is summarized; accepted notes preserve evidence while uncertain claims go to review",
    );
    check.check(
        obsidian::search(&settings.personas[0].knowledge, "회의 금요일", &none)
            .await
            .unwrap()
            .excerpts
            .is_empty()
            && documents::list(&vault, &none).await.unwrap().notes.len() == 2,
        "Review notes are excluded from bot search and the trusted library",
    );
    let restarted = AutoKnowledgeService::new(reader.clone(), runner.clone(), Some(state.clone()));
    restarted.tick(&settings, true, &none).await.unwrap();
    check.check(
        runner.calls() == 1,
        "Restart resumes the persisted cursor without repeated summaries",
    );
    reader.push(message(5, "휴가는 사흘 전에 신청합니다.", "person"));
    runner.set_text(digest(vec![claim(
        "휴가 신청",
        "휴가는 사흘 전에 신청합니다.",
        5,
        "휴가는 사흘 전에 신청합니다.",
        "confirmed",
    )]));
    service.tick(&settings, true, &none).await.unwrap();
    check.check(
        md_count(&vault.join("02-업무지식")) == 1 && status().contains("중복 1건"),
        "Repeated knowledge is deduplicated across batches",
    );
    reader.push(message(6, "휴가는 닷새 전에 신청합니다.", "person"));
    runner.set_text(digest(vec![claim(
        "휴가 신청",
        "휴가는 닷새 전에 신청합니다.",
        6,
        "휴가는 닷새 전에 신청합니다.",
        "confirmed",
    )]));
    service.tick(&settings, true, &none).await.unwrap();
    check.check(
        md_count(&vault.join("02-업무지식")) == 1 && md_count(&vault.join("검토필요")) == 2,
        "Conflicting content under an existing topic is held for review instead of replacing knowledge",
    );
    reader.push(message(7, "배포는 금요일입니다.", "person"));
    runner.set_text(digest(vec![claim(
        "배포 일정",
        "배포는 금요일입니다.",
        999,
        "존재하지 않는 근거",
        "confirmed",
    )]));
    service.tick(&settings, true, &none).await.unwrap();
    let failed_calls = runner.calls();
    check.check(
        status().contains("실패") && md_count(&vault.join("02-업무지식")) == 1,
        "Invalid source evidence fails closed and does not publish a note",
    );
    service.tick(&settings, false, &none).await.unwrap();
    check.check(
        runner.calls() == failed_calls,
        "Failed summaries wait until the next interval rather than retrying every minute",
    );
    runner.set_text(digest(vec![claim(
        "배포 일정",
        "배포는 금요일입니다.",
        7,
        "배포는 금요일입니다.",
        "confirmed",
    )]));
    settings.auto_knowledge.auto_save = false;
    service.tick(&settings, true, &none).await.unwrap();
    check.check(
        md_count(&vault.join("검토필요")) == 3,
        "Failure preserves the cursor for retry, and review-only mode never activates new facts",
    );
    settings.auto_knowledge.enabled = false;
    reader.push(message(8, "중지 이후", "person"));
    let before = runner.calls();
    service.tick(&settings, true, &none).await.unwrap();
    check.check(
        runner.calls() == before,
        "Disabled automation never reads or summarizes new content",
    );
    settings.auto_knowledge.enabled = true;
    *reader.messages.lock().unwrap() = (20..520).map(|i| message(i, "many", "person")).collect();
    service.tick(&settings, true, &none).await.unwrap();
    check.check(
        status().contains("500") && runner.calls() == before,
        "A gap beyond the reader window is reported without silently skipping messages",
    );
    catchup_and_comparison(&area, check).await;
    recovery(&area, &vault, check).await;
}

async fn catchup_and_comparison(area: &Path, check: &Check) {
    let none = CancellationToken::new();
    let vault = area.join("comparison-vault");
    std::fs::create_dir_all(&vault).unwrap();
    let state_root = area.join("comparison-state");
    let mut settings = settings_for(&vault, false, "연속 처리");
    for provider in &mut settings.providers {
        provider.enabled = provider.id == ORDER[0];
    }
    let reader = Arc::new(Reader::new(room(), false));
    let runner = Arc::new(Runner::new("{\"items\":[]}"));
    let service =
        AutoKnowledgeService::new(reader.clone(), runner.clone(), Some(state_root.clone()));
    let status = watch(&service);
    let status = || status.lock().unwrap().clone();
    reader.push(message(1, "기준", "person"));
    service.tick(&settings, true, &none).await.unwrap();
    for i in 2..=236 {
        reader.push(message(i, &format!("잡담 {i}"), "person"));
    }
    runner.set(|calls, _| {
        if calls == 2 {
            Err(Error::io("중간 실패"))
        } else {
            Ok(digest(vec![]))
        }
    });
    service.tick(&settings, true, &none).await.unwrap();
    let states: Vec<_> = std::fs::read_dir(&state_root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    assert_eq!(states.len(), 1);
    let path = states[0].clone();
    let cursor = || {
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&path).unwrap()).unwrap()
            ["cursor"]
            .as_str()
            .unwrap()
            .to_string()
    };
    check.check(cursor() == "101" && status().contains("실패"), "A later batch failure preserves the successful batch cursor without skipping the failed range");
    runner.set_text(digest(vec![]));
    let calls = runner.calls();
    AutoKnowledgeService::new(reader.clone(), runner.clone(), Some(state_root.clone()))
        .tick(&settings, true, &none)
        .await
        .unwrap();
    check.check(cursor() == "236" && runner.calls() == calls + 2, "Restart drains every remaining batch to the latest message in one cycle, even when no knowledge is found");
    let saved = service.saved_statuses(&settings);
    check.check(
        saved.len() == 1 && saved[0].contains("135개 확인"),
        "The last classification result remains visible after restart",
    );
    std::fs::write(
        vault.join("휴가규칙.md"),
        "# 휴가 신청\n휴가는 사흘 전에 신청합니다.",
    )
    .unwrap();
    reader.push(message(237, "휴가 신청은 3일 전에 해주세요.", "person"));
    runner.set_text(digest(vec![KnowledgeClaim {
        relation: "duplicate".into(),
        related_source: Some("휴가규칙.md".into()),
        related_quote: Some("휴가는 사흘 전에 신청합니다.".into()),
        ..claim(
            "휴가 신청",
            "휴가 신청 기한은 3일 전입니다.",
            237,
            "휴가 신청은 3일 전에 해주세요.",
            "confirmed",
        )
    }]));
    service.tick(&settings, true, &none).await.unwrap();
    check.check(
        status().contains("중복 1건") && !vault.join("자동정리").exists() && runner.prompt().contains("휴가는 사흘 전에 신청합니다."),
        "Semantic duplicate classification compares actual vault notes even when bot retrieval is disabled",
    );
    reader.push(message(
        238,
        "휴가 신청 기한을 닷새 전으로 바꿉니다.",
        "person",
    ));
    runner.set_text(digest(vec![KnowledgeClaim {
        relation: "conflict".into(),
        related_source: Some("휴가규칙.md".into()),
        related_quote: Some("휴가는 사흘 전에 신청합니다.".into()),
        ..claim(
            "변경된 휴가 기한",
            "휴가는 닷새 전에 신청합니다.",
            238,
            "휴가 신청 기한을 닷새 전으로 바꿉니다.",
            "confirmed",
        )
    }]));
    service.tick(&settings, true, &none).await.unwrap();
    let reviews = md_files(&vault.join("검토필요"));
    check.check(
        !vault.join("자동정리").exists() && reviews.len() == 1 && std::fs::read_to_string(&reviews[0]).unwrap().contains("휴가규칙.md"),
        "Semantic conflict goes to review with the existing source even when the model changes the topic name",
    );
    reader.push(message(239, "휴가는 하루 전에 신청합니다.", "person"));
    runner.set_text(digest(vec![KnowledgeClaim {
        relation: "duplicate".into(),
        related_source: Some("없는노트.md".into()),
        related_quote: Some("거짓 근거".into()),
        ..claim(
            "휴가",
            "하루 전 신청",
            239,
            "휴가는 하루 전에 신청합니다.",
            "confirmed",
        )
    }]));
    service.tick(&settings, true, &none).await.unwrap();
    check.check(
        cursor() == "238" && status().contains("실패"),
        "Unsupported duplicate classification cannot silently discard new messages",
    );
}

async fn recovery(area: &Path, vault: &Path, check: &Check) {
    let state = area.join("recover");
    std::fs::create_dir_all(&state).unwrap();
    let reader = Arc::new(Reader::new(room(), false));
    let runner = Arc::new(Runner::new("{\"items\":[]}"));
    let mut settings = settings_for(vault, true, "회복");
    settings.auto_knowledge.rooms[0].title = "회복".into();
    let room = settings.auto_knowledge.rooms[0].clone();
    let id = AiConversation::hash(&json::prompt(&serde_json::json!({
        "Profile": room.profile, "RoomId": room.room_id, "Generation": room.generation, "vault": vault.to_string_lossy(), "SelfId": settings.self_id
    })));
    let checkpoint = Checkpoint {
        cursor: "9".into(),
        last_run: chrono::Utc::now().fixed_offset(),
        ..Default::default()
    };
    let pending = Pending {
        vault: vault.to_string_lossy().into_owned(),
        batch: "abcdef-12345".into(),
        accepted: "# recovered\n복구된 지식".into(),
        review: String::new(),
        next: checkpoint,
        writes: None,
    };
    let pending_path = state.join(format!("{id}.pending.json"));
    std::fs::write(&pending_path, serde_json::to_string(&pending).unwrap()).unwrap();
    let service = AutoKnowledgeService::new(reader, runner.clone(), Some(state.clone()));
    service
        .tick(&settings, false, &CancellationToken::new())
        .await
        .unwrap();
    check.check(
        vault.join("자동정리").join("abcdef-12345.md").exists()
            && !pending_path.exists()
            && runner.calls() == 0,
        "Interrupted publication recovers the exact saved summary without calling AI again",
    );
}
