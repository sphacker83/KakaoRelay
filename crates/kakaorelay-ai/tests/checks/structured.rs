//! 원본: `StructuredKnowledgeChecks.cs`.

use std::path::Path;

use chrono::{Duration, Utc};
use kakaorelay_ai::auto_knowledge::{
    KnowledgeClaim, KnowledgeDigest, KnowledgeEvidence, validate_digest,
};
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::chat_models::LocalMessage;
use kakaorelay_ai::documents;
use kakaorelay_ai::obsidian::{self, KnowledgeExcerpt};
use kakaorelay_ai::settings::{KnowledgeRoomSubscription, KnowledgeSettings};
use kakaorelay_ai::structured::{self, KnowledgeWrite};

use crate::support::Check;

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn files(dir: &Path, filter: impl Fn(&str) -> bool) -> Vec<std::path::PathBuf> {
    let mut result: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| filter(&p.file_name().unwrap().to_string_lossy()))
        .collect();
    result.sort();
    result
}

fn digest(claim: &KnowledgeClaim) -> KnowledgeDigest {
    KnowledgeDigest {
        items: vec![claim.clone()],
    }
}

pub async fn run(root: &Path, check: &Check) {
    let none = CancellationToken::new();
    let vault = root.join("structured-vault");
    std::fs::create_dir_all(&vault).unwrap();
    structured::ensure(&vault).unwrap();
    let room = KnowledgeRoomSubscription::new("fixture", "room", "Fixture", uuid::Uuid::new_v4());
    let now = Utc::now().fixed_offset();
    let message =
        |id: &str, text: &str| LocalMessage::new(id, "person", "Fixture", now, 1, text, false);
    let first = KnowledgeClaim {
        project: Some("테스트 프로젝트".into()),
        owner: Some("담당A".into()),
        due: Some("금요일".into()),
        task_state: Some("todo".into()),
        tags: Some(vec![" 배포 ".into(), "배포".into(), "CI CD".into()]),
        ..KnowledgeClaim::new(
            "출시 준비",
            "OLDSCHEDULE42 금요일 출시 준비",
            "task",
            "confirmed",
            vec![KnowledgeEvidence::new("1", "금요일")],
        )
    };
    let plan = structured::plan(
        &vault,
        &room,
        &digest(&first),
        &[message("1", "금요일")],
        now,
        "batch-1",
        true,
        None,
        None,
    )
    .unwrap();
    structured::apply(&vault, &plan.writes).unwrap();
    let catalog = structured::load(&vault).unwrap();
    let initial = catalog.entities.values().next().unwrap().clone();
    let path = vault.join(&initial.source);
    check.check(
        catalog.entities.len() == 1
            && plan.saved == 1
            && initial.owner.as_deref() == Some("담당A")
            && initial.due.as_deref() == Some("금요일")
            && initial.task_state.as_deref() == Some("todo")
            && read(&path).starts_with("---\n")
            && read(&path).contains("[ ] 출시 준비"),
        "Task notes expose Obsidian frontmatter, owner, due date, stable identity and checkbox state",
    );
    let projects = files(&vault.join("01-프로젝트"), |n| {
        n.starts_with("프로젝트-") && n.ends_with(".md")
    });
    check.check(
        projects.len() == 1
            && read(&projects[0]).contains("출시 준비")
            && read(&projects[0]).contains("03-할일/"),
        "Project dashboards link their current tasks and show present metadata",
    );
    let has = |tags: &[String], t: &str| tags.iter().any(|x| x == t);
    check.check(
        has(&initial.tags, "유형/할일")
            && has(&initial.tags, "프로젝트/테스트-프로젝트")
            && has(&initial.tags, "상태/예정")
            && initial.tags.iter().filter(|t| *t == "주제/배포").count() == 1
            && has(&initial.tags, "주제/ci-cd")
            && read(&path).contains("title:"),
        "Tags normalize topic variants while consistent type, project, state and human-readable titles are generated",
    );
    {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all("사용자 직접 메모를 보존하세요.\n".as_bytes())
            .unwrap();
    }
    let update = KnowledgeClaim {
        topic: "출시 준비 완료".into(),
        text: "NEWSCHEDULE42 월요일 출시 준비 완료".into(),
        relation: "update".into(),
        entity_id: Some(initial.id.clone()),
        related_source: Some(initial.source.clone()),
        related_quote: Some(initial.text.clone()),
        due: Some("월요일".into()),
        task_state: Some("done".into()),
        owner: None,
        evidence: vec![KnowledgeEvidence::new("2", "월요일 완료 확정")],
        ..first.clone()
    };
    let references = vec![KnowledgeExcerpt {
        source: initial.source.clone(),
        part: 1,
        text: read(&path),
    }];
    validate_digest(
        &digest(&update),
        &[message("2", "월요일 완료 확정")],
        Some(&references),
    )
    .unwrap();
    let changed = structured::plan(
        &vault,
        &room,
        &digest(&update),
        &[message("2", "월요일 완료 확정")],
        now + Duration::minutes(1),
        "batch-2",
        true,
        None,
        None,
    )
    .unwrap();
    // 첫 파일만 반영된 뒤 중단된 경우를 복구한다.
    structured::apply(&vault, &changed.writes[..1]).unwrap();
    structured::apply(&vault, &changed.writes).unwrap();
    structured::apply(&vault, &changed.writes).unwrap();
    let catalog = structured::load(&vault).unwrap();
    let current = catalog.entities.values().next().unwrap().clone();
    check.check(
        catalog.entities.len() == 1
            && current.id == initial.id
            && current.source == initial.source
            && current.topic == update.topic
            && current.owner.as_deref() == Some("담당A")
            && current.task_state.as_deref() == Some("done")
            && read(&path).contains("사용자 직접 메모")
            && read(&path).contains("[x]"),
        "Explicit updates retain identity, filename, unspecified owner and user notes while updating current task state",
    );
    check.check(
        has(&current.tags, "상태/완료")
            && !has(&current.tags, "상태/예정")
            && has(&current.aliases, &initial.topic),
        "Renamed notes retain their previous title as an alias and replace stale status tags",
    );
    let history = files(&vault.join("05-변경이력"), |n| n.ends_with(".md"));
    check.check(history.len() == 1 && read(&history[0]).contains(&initial.text), "Interrupted publication recovers idempotently and retains the previous fact and evidence once");
    let settings = KnowledgeSettings {
        enabled: true,
        vault_path: vault.to_string_lossy().into_owned(),
    };
    let search = |q: &str| {
        let (s, q) = (settings.clone(), q.to_string());
        async move {
            obsidian::search(&s, &q, &CancellationToken::new())
                .await
                .unwrap()
        }
    };
    check.check(
        search("OLDSCHEDULE42").await.excerpts.is_empty()
            && !search("NEWSCHEDULE42").await.excerpts.is_empty(),
        "Bot retrieval uses current knowledge and excludes stale conversation and change archives",
    );
    check.check(
        documents::list(&vault, &none)
            .await
            .unwrap()
            .notes
            .iter()
            .all(|n| !structured::historical(&n.source)),
        "The in-app library shows current notes rather than historical duplicates",
    );

    let conflict = KnowledgeClaim {
        text: "확인되지 않은 일정".into(),
        relation: "conflict".into(),
        related_quote: Some(current.text.clone()),
        evidence: vec![KnowledgeEvidence::new("3", "아마")],
        ..update.clone()
    };
    let held = structured::plan(
        &vault,
        &room,
        &digest(&conflict),
        &[message("3", "아마")],
        now,
        "batch-3",
        true,
        None,
        None,
    )
    .unwrap();
    structured::apply(&vault, &held.writes).unwrap();
    check.check(
        held.review == 1
            && held.saved == 0
            && structured::load(&vault).unwrap().entities[&current.id].text == current.text,
        "Unconfirmed contradictions go to review without replacing the current fact",
    );
    let bad_update = KnowledgeClaim {
        entity_id: Some("A".repeat(32)),
        related_quote: Some(current.text.clone()),
        ..update.clone()
    };
    let invalid = structured::plan(
        &vault,
        &room,
        &digest(&bad_update),
        &[message("2", "월요일 완료 확정")],
        now,
        "batch-4",
        true,
        None,
        None,
    )
    .unwrap();
    check.check(
        invalid.review == 1 && invalid.saved == 0,
        "Unknown update IDs never create or overwrite a canonical note",
    );

    let retag = KnowledgeClaim {
        tags: Some(vec!["릴리스".into()]),
        related_quote: Some(current.text.clone()),
        ..update.clone()
    };
    let retagged = structured::plan(
        &vault,
        &room,
        &digest(&retag),
        &[message("2", "월요일 완료 확정")],
        now,
        "retag",
        true,
        None,
        None,
    )
    .unwrap();
    structured::apply(&vault, &retagged.writes).unwrap();
    check.check(
        retagged.saved == 1 && has(&structured::load(&vault).unwrap().entities[&current.id].tags, "주제/릴리스") && read(&path).contains("\"주제/릴리스\""),
        "Verified metadata-only updates replace topic tags and write readable Korean Obsidian properties",
    );

    let next = KnowledgeClaim {
        text: "새 상태".into(),
        related_quote: Some("월요일 완료 확정".into()),
        ..update.clone()
    };
    let before = read(&path);
    std::fs::write(
        &path,
        before.replace(&current.text, "사람이 직접 수정한 관리 내용"),
    )
    .unwrap();
    let manual = structured::plan(
        &vault,
        &room,
        &digest(&next),
        &[message("2", "월요일 완료 확정")],
        now,
        "manual",
        true,
        None,
        None,
    );
    check.check(
        matches!(manual, Err(ref e) if e.is_io())
            && read(&path).contains("사람이 직접 수정한 관리 내용"),
        "Manual changes inside managed content are preserved rather than overwritten",
    );
    std::fs::write(&path, &before).unwrap();
    let race = structured::plan(
        &vault,
        &room,
        &digest(&next),
        &[message("2", "월요일 완료 확정")],
        now,
        "race",
        true,
        None,
        None,
    )
    .unwrap();
    {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all("편집 경합".as_bytes())
            .unwrap();
    }
    let raced = structured::apply(&vault, &race.writes);
    check.check(
        matches!(raced, Err(ref e) if e.is_io())
            && !vault
                .join("05-변경이력")
                .join(format!("{}-race.md", initial.id))
                .exists(),
        "A changed target stops the whole transaction before any overwrite",
    );
    let traversal = structured::apply(
        &vault,
        &[KnowledgeWrite {
            source: "../outside.md".into(),
            before: None,
            body: "bad".into(),
        }],
    );
    check.check(
        matches!(traversal, Err(ref e) if e.is_io()) && !root.join("outside.md").exists(),
        "Publication rejects paths outside the vault",
    );

    let migration = root.join("migration-vault");
    std::fs::create_dir_all(migration.join("자동정리")).unwrap();
    let original = "# Legacy 대화 자동 정리\n\n계정: fixture\n대화방 ID: legacy\n\n## 업무 규칙\n\nLEGACYFACT42 규칙입니다.\n\n> 종류: knowledge · 대화에서 확인된 내용 (외부 사실 검증 아님)\n\n- 근거: 메시지 5 · 원래 시각 · 작성자\n  > 실제 근거\n";
    let old_file = migration.join("자동정리").join("legacy.md");
    std::fs::write(&old_file, original).unwrap();
    structured::ensure(&migration).unwrap();
    structured::ensure(&migration).unwrap();
    let migrated = structured::load(&migration).unwrap();
    check.check(
        read(&old_file) == original && migrated.entities.len() == 1 && migrated.migrated.contains("자동정리/legacy.md"),
        "Legacy summaries migrate once into separate entities while their original files remain byte-for-byte intact",
    );
    let migrated_search = obsidian::search(
        &KnowledgeSettings {
            enabled: true,
            vault_path: migration.to_string_lossy().into_owned(),
        },
        "LEGACYFACT42",
        &none,
    )
    .await
    .unwrap();
    check.check(
        !migrated_search.excerpts.is_empty()
            && migrated_search
                .excerpts
                .iter()
                .all(|e| !e.source.starts_with("자동정리/")),
        "Migrated legacy summaries cannot compete with their current structured notes in retrieval",
    );
}
