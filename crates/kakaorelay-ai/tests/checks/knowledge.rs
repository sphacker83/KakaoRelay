//! 원본: `KnowledgeChecks.cs`. `AiService` 응답 생성에 쓰는 두 검사는 `ai_service` 이식 뒤 `knowledge_service`에서 실행한다.

use std::path::Path;

use kakaorelay_ai::ai_service::AiCommand;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::chat_models::{ChatContext, LocalMessage, LocalRoom};
use kakaorelay_ai::conversation::AiConversation;
use kakaorelay_ai::obsidian::{self, MAX_CHARACTERS, MAX_FILE_BYTES};
use kakaorelay_ai::settings::{AiSettings, AiSettingsStore, KnowledgeSettings};
use kakaorelay_ai::{Error, documents, text::utf16_len};

use crate::support::Check;

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

pub async fn run(root: &Path, check: &Check) {
    let none = CancellationToken::new();
    let vault = root.join("vault");
    for dir in ["업무", ".obsidian", ".trash"] {
        std::fs::create_dir_all(vault.join(dir)).unwrap();
    }
    let policy = vault.join("업무").join("휴가.md");
    write(
        &policy,
        "# 휴가 정책\n휴가 신청은 사흘 전에 합니다. [[인사팀]] #업무",
    );
    write(&vault.join("무관.md"), "주말 축구 경기");
    write(&vault.join(".obsidian").join("private.md"), "휴가 SECRET");
    write(&vault.join(".trash").join("deleted.md"), "휴가 DELETED");
    write(&vault.join("private.txt"), "휴가 TXT");
    let mut settings = KnowledgeSettings {
        enabled: true,
        vault_path: vault.to_string_lossy().into_owned(),
    };
    let search = |s: &KnowledgeSettings, q: &str| {
        let (s, q) = (s.clone(), q.to_string());
        async move {
            obsidian::search(&s, &q, &CancellationToken::new())
                .await
                .unwrap()
        }
    };
    let found = search(&settings, "휴가는 언제 신청하나요?").await;
    check.check(found.notes == 2 && found.excerpts.len() == 1 && found.excerpts[0].source == "업무/휴가.md", "Knowledge retrieves Korean Markdown with particles and excludes hidden folders and non-Markdown");
    check.check(
        search(&settings, "xyzunrelated").await.excerpts.is_empty(),
        "Unrelated notes are not sent as fallback knowledge",
    );
    write(&policy, "# 휴가 정책\n휴가 신청은 닷새 전에 합니다.");
    let edited = search(&settings, "휴가").await;
    check.check(
        edited.excerpts.len() == 1 && edited.excerpts[0].text.contains("닷새"),
        "Edited notes are visible on the next search",
    );
    write(&vault.join("large.md"), &"a".repeat(MAX_FILE_BYTES + 1));
    check.check(
        search(&settings, "휴가").await.skipped == 1,
        "Oversized notes are skipped without unbounded reads",
    );
    write(&vault.join("many.md"), &"휴가 정책 상세 내용 ".repeat(5000));
    let bounded = search(&settings, "휴가").await;
    check.check(
        bounded.excerpts.len() <= 6
            && bounded
                .excerpts
                .iter()
                .map(|x| utf16_len(&x.text))
                .sum::<usize>()
                <= MAX_CHARACTERS,
        "Knowledge prompt excerpts obey total character and count limits",
    );
    std::fs::remove_file(vault.join("many.md")).unwrap();
    let store = AiSettingsStore::new(root.join("knowledge-settings.json"));
    let mut ai = AiSettings::default();
    ai.ensure_personas();
    ai.personas[0].knowledge = settings.clone();
    store.save(&mut ai).unwrap();
    check.check(
        store.load().unwrap().personas[0].knowledge.vault_path == settings.vault_path,
        "Per-persona vault settings round-trip",
    );
    let room = LocalRoom::new("imported", "knowledge", "knowledge", true);
    let time = kakaorelay_ai::json::time::parse("2026-09-28T12:00:00+09:00").unwrap();
    let message = LocalMessage::new("1", "1", "A", time, 1, "휴가는 언제 신청해요?", false);
    let context = ChatContext {
        room: room.clone(),
        messages: vec![message.clone()],
        read_at: time,
    };
    let command = |target: Option<&str>| AiCommand {
        target_message_id: target.map(Into::into),
        ..AiCommand::new(&room.profile, &room.id, "reply")
    };
    check.check(
        obsidian::query(&context, &command(Some("absent"))).is_empty(),
        "Missing target message cannot retrieve knowledge using unrelated past messages",
    );
    let deleted = ChatContext {
        messages: vec![LocalMessage {
            deleted: true,
            ..message
        }],
        ..context.clone()
    };
    check.check(
        obsidian::query(
            &deleted,
            &AiCommand::new(&room.profile, &room.id, "analyze"),
        )
        .is_empty(),
        "Deleted message text cannot become a knowledge query",
    );
    let saved = obsidian::save_note(
        &settings,
        "회의 요약",
        "금요일 출시 검토",
        Some(&room),
        &none,
    )
    .await
    .unwrap();
    let saved_again = obsidian::save_note(
        &settings,
        "회의 요약",
        "금요일 출시 확정",
        Some(&room),
        &none,
    )
    .await
    .unwrap();
    check.check(
        saved != saved_again
            && read(&saved).contains("금요일 출시 검토")
            && read(&saved).contains("대화 분석 결과"),
        "Summary notes preserve source metadata and never overwrite an existing note",
    );
    check.check(
        search(&settings, "출시").await.excerpts.len() == 2,
        "Saved conversation summaries are available to subsequent knowledge searches",
    );
    std::fs::remove_file(&policy).unwrap();
    check.check(
        search(&settings, "휴가").await.excerpts.is_empty(),
        "Deleted knowledge notes disappear on the next request",
    );
    let session_folder = root.join("knowledge-session");
    AiConversation {
        session_id: Some("existing".into()),
        knowledge_revision: Some("old".into()),
        ..Default::default()
    }
    .save(&session_folder)
    .unwrap();
    check.check(
        AiConversation::load(&session_folder, Some("old"))
            .session_id
            .as_deref()
            == Some("existing")
            && AiConversation::load(&session_folder, Some("new"))
                .session_id
                .is_none(),
        "Unchanged knowledge resumes the session; changed knowledge discards stale excerpts",
    );
    settings.enabled = false;
    settings.vault_path = root.join("missing").to_string_lossy().into_owned();
    check.check(
        search(&settings, "휴가").await.notes == 0,
        "Disconnected vault performs no filesystem reads",
    );
    settings.enabled = true;
    let missing = obsidian::search(&settings, "휴가", &none).await;
    check.check(
        matches!(missing, Err(Error::DirectoryNotFound(_))),
        "Missing enabled vault reports a configuration error",
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    check.check(
        obsidian::search(&settings, "휴가", &cancelled).await == Err(Error::Cancelled),
        "Knowledge scan supports cancellation",
    );
    imports(root, &vault, check).await;
}

async fn imports(root: &Path, vault: &Path, check: &Check) {
    let none = CancellationToken::new();
    let text = root.join("직원 안내.txt");
    write(&text, "교육 일정은 매주 금요일입니다.");
    let first = documents::import(vault, &text, &none).await.unwrap();
    let second = documents::import(vault, &text, &none).await.unwrap();
    check.check(
        first.len() == 1
            && second.len() == 1
            && first[0] != second[0]
            && read(&text) == "교육 일정은 매주 금요일입니다."
            && read(&first[0]).contains("교육 일정"),
        "Document import preserves originals and avoids duplicate-name overwrites",
    );
    let settings = KnowledgeSettings {
        enabled: true,
        vault_path: vault.to_string_lossy().into_owned(),
    };
    let found = obsidian::search(&settings, "교육 일정", &none)
        .await
        .unwrap();
    check.check(
        found
            .excerpts
            .iter()
            .any(|x| x.source.starts_with("자료/") && x.text.contains("금요일")),
        "Imported documents are immediately searchable by the bot",
    );
    let word = root.join("guide.docx");
    {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&word).unwrap());
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all("<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r><w:t>회의 안내</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>월요일</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>".as_bytes()).unwrap();
        zip.finish().unwrap();
    }
    let word_notes = documents::import(vault, &word, &none).await.unwrap();
    check.check(
        word_notes.len() == 1
            && read(&word_notes[0]).contains("회의 안내")
            && read(&word_notes[0]).contains("월요일"),
        "Word paragraphs and table text become knowledge notes",
    );
    let pdf = root.join("guide.pdf");
    std::fs::write(
        &pdf,
        kakaorelay_testkit::pdf::single_page(Some("Training is on Friday")),
    )
    .unwrap();
    let pdf_notes = documents::import(vault, &pdf, &none).await.unwrap();
    check.check(
        pdf_notes.len() == 1 && read(&pdf_notes[0]).contains("Training is on Friday"),
        "PDF text is extracted locally into searchable notes",
    );
    let empty_pdf = root.join("scan.pdf");
    std::fs::write(&empty_pdf, kakaorelay_testkit::pdf::single_page(None)).unwrap();
    let empty = documents::import(vault, &empty_pdf, &none).await;
    check.check(
        matches!(empty, Err(Error::Argument(ref m)) if m.contains("OCR")),
        "Image-only or empty PDF explains that text recognition is required",
    );
    let long_text = root.join("long.md");
    write(&long_text, &"가나다😀 ".repeat(20000));
    let parts = documents::import(vault, &long_text, &none).await.unwrap();
    check.check(
        parts.len() > 1
            && parts.iter().all(|p| {
                (std::fs::metadata(p).unwrap().len() as usize) < MAX_FILE_BYTES
                    && !read(p).contains('\u{FFFD}')
            }),
        "Long documents split into readable notes without corrupting Unicode",
    );
    let library = documents::list(vault, &none).await.unwrap();
    check.check(
        library.notes.iter().any(|n| n.path == first[0])
            && library.notes.iter().all(|n| !n.source.starts_with('.'))
            && library.skipped == 1,
        "Library lists imported notes while excluding hidden and oversized files",
    );
    let count = || walk_md(vault);
    let before = count();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let result = documents::import(vault, &text, &cancelled).await;
    check.check(
        result == Err(Error::Cancelled) && count() == before,
        "Cancelled imports publish no partial document",
    );
    let unsupported = documents::import(vault, &root.join("program.exe"), &none).await;
    check.check(
        matches!(unsupported, Err(Error::Argument(_))),
        "Unsupported source types are rejected before opening a file",
    );
    let binary = root.join("binary.txt");
    write(&binary, "hello\0world");
    check.check(
        matches!(
            documents::import(vault, &binary, &none).await,
            Err(Error::Argument(_))
        ),
        "Binary data disguised as text cannot become knowledge",
    );
}

pub fn walk_md(dir: &Path) -> usize {
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += walk_md(&path);
        } else if path.extension().is_some_and(|e| e == "md") {
            count += 1;
        }
    }
    count
}
