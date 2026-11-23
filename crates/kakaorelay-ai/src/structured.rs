//! 원본: `StructuredKnowledge.cs`. 숨김 색인(`.kakaorelay/structured.json`)이 식별자를 갖고,
//! Markdown은 옵시디언에서 읽고 고칠 수 있게 둔다. 관리 영역 마커와 해시 규칙은 기존 보관함과 같아야 한다(inventory §4-3).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use chrono::{DateTime, FixedOffset, Utc};
use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auto_knowledge::{KnowledgeClaim, KnowledgeDigest};
use crate::chat_models::LocalMessage;
use crate::error::{Error, Result};
use crate::json::{self, time};
use crate::obsidian::{MAX_FILE_BYTES, check_directory, is_link};
use crate::provider_storage::absolute;
use crate::settings::KnowledgeRoomSubscription;
use crate::text::{
    NEW_LINE, collapse_whitespace, is_blank, lower_invariant, nfc, ordinal_cmp, utf16_len,
};

const INDEX_PATH: &str = ".kakaorelay/structured.json";
const RECOVERY_PATH: &str = ".kakaorelay/migration.pending.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeWrite {
    pub source: String,
    pub before: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgePlan {
    pub writes: Vec<KnowledgeWrite>,
    pub saved: usize,
    pub review: usize,
    pub duplicates: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeEntity {
    pub id: String,
    pub project_id: String,
    pub project: String,
    pub topic: String,
    pub kind: String,
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub task_state: Option<String>,
    pub source: String,
    pub profile: String,
    pub room_id: String,
    pub room_title: String,
    pub evidence: String,
    #[serde(with = "crate::json::time")]
    pub created_at: DateTime<FixedOffset>,
    #[serde(with = "crate::json::time")]
    pub updated_at: DateTime<FixedOffset>,
    #[serde(default)]
    pub changes: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Catalog {
    pub version: i32,
    pub entities: IndexMap<String, KnowledgeEntity>,
    pub blocks: IndexMap<String, String>,
    pub migrated: IndexSet<String>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            version: 1,
            entities: IndexMap::new(),
            blocks: IndexMap::new(),
            migrated: IndexSet::new(),
        }
    }
}

/// 인라인 속성 값(`JsonSerializer.Serialize(value, { Encoder = PromptJson.Encoder })`).
fn property<T: Serialize + ?Sized>(value: &T) -> String {
    json::prompt(value)
}

fn header(title: &str, kind: &str, tags: &[String], now: &DateTime<FixedOffset>) -> String {
    format!(
        "---\ntitle: {}\ntype: {kind}\ntags: {}\ncreated: {}\n---\n",
        property(title),
        property(tags),
        time::round_trip(now)
    )
}

pub fn load(vault: &Path) -> Result<Catalog> {
    let path = safe_path(vault, INDEX_PATH)?;
    if !path.exists() {
        return Ok(Catalog::default());
    }
    let catalog: Option<Catalog> = serde_json::from_str(&crate::text::read_text(&path)?)?;
    catalog.ok_or_else(|| Error::io("구조화 지식 색인을 읽을 수 없습니다."))
}

fn key(value: &str) -> String {
    collapse_whitespace(&lower_invariant(&nfc(value)))
        .trim()
        .to_string()
}

fn id(value: &str) -> String {
    json::hash(value)[..32].to_string()
}

static LETTER_OR_NUMBER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[\p{L}\p{N}]$").expect("정규식"));

/// .NET 정규식은 UTF-16 단위로 동작해 BMP 밖 문자는 문자·숫자로 보지 않는다.
fn letter_or_number(c: char) -> bool {
    (c as u32) <= 0xFFFF && LETTER_OR_NUMBER.is_match(c.encode_utf8(&mut [0; 4]))
}

fn tag(value: &str) -> String {
    let lowered = lower_invariant(nfc(value).trim().trim_start_matches('#'));
    let dashed: String = {
        let mut out = String::new();
        let mut in_space = false;
        for c in lowered.chars() {
            if c.is_whitespace() {
                if !in_space {
                    out.push('-');
                }
                in_space = true;
            } else {
                out.push(c);
                in_space = false;
            }
        }
        out
    };
    let kept: String = dashed
        .chars()
        .filter(|&c| letter_or_number(c) || c == '_' || c == '-')
        .collect();
    kept.trim_matches('-').to_string()
}

fn tags(kind: &str, project: &str, state: Option<&str>, topics: &[String]) -> Vec<String> {
    let kind_label = match kind {
        "project" => "프로젝트",
        "decision" => "결정",
        "task" => "할일",
        _ => "업무지식",
    };
    let status = match state {
        Some("todo") => "예정",
        Some("doing") => "진행중",
        Some("blocked") => "보류",
        Some("done") => "완료",
        Some("cancelled") => "취소",
        _ if kind == "task" => "미정",
        _ => "확정",
    };
    let mut result: Vec<String> = vec![];
    let base = [
        format!("유형/{kind_label}"),
        format!("프로젝트/{}", tag(project)),
        format!("상태/{status}"),
    ];
    let topics = topics
        .iter()
        .map(|t| format!("주제/{}", tag(t.split('/').next_back().unwrap_or(t))))
        .filter(|t| utf16_len(t) > 3);
    for item in base.into_iter().chain(topics) {
        if !result.contains(&item) {
            result.push(item);
        }
    }
    result
}

fn heading(value: &str) -> String {
    value
        .replace(['\r', '\n'], " ")
        .replace('[', "(")
        .replace(']', ")")
        .replace('|', " ")
}

fn slug(value: &str) -> String {
    let heading = heading(value);
    let replaced: String = heading
        .chars()
        .map(|c| {
            if letter_or_number(c) || matches!(c, ' ' | '_' | '-') {
                c.to_string()
            } else {
                "_".repeat(c.len_utf16())
            }
        })
        .collect();
    let cut = crate::text::utf16_prefix(&replaced, 48.min(utf16_len(&heading)));
    cut.trim().trim_end_matches('.').to_string()
}

fn link(source: &str, title: &str) -> String {
    format!(
        "[[{}|{}]]",
        source
            .strip_suffix(".md")
            .unwrap_or(&source[..source.len().saturating_sub(3)]),
        heading(title)
    )
}

fn wrap(id: &str, body: &str) -> String {
    let split = body
        .strip_prefix("---\n")
        .and_then(|rest| rest.find("\n---\n"))
        .map(|i| i + 4);
    let (metadata, content) = match split {
        Some(s) => (&body[..s + 5], &body[s + 5..]),
        None => ("", body),
    };
    format!(
        "{metadata}<!-- kakaorelay:{id}:start -->\n{}\n<!-- kakaorelay:{id}:end -->",
        content.trim()
    )
}

pub fn historical(source: &str) -> bool {
    source.starts_with("04-대화기록/") || source.starts_with("05-변경이력/")
}

pub fn current(source: &str) -> bool {
    source.starts_with("01-프로젝트/")
        || source.starts_with("02-업무지식/")
        || source.starts_with("03-할일/")
}

struct Planner<'a> {
    vault: &'a Path,
    catalog: Catalog,
    writes: Vec<KnowledgeWrite>,
}

impl Planner<'_> {
    fn add_raw(&mut self, source: &str, body: String) -> Result<()> {
        let existing = self.writes.iter().position(|w| w.source == source);
        let before = match existing {
            Some(i) => self.writes[i].before.clone(),
            None => hash_or_null(self.vault, source)?,
        };
        let write = KnowledgeWrite {
            source: source.into(),
            before,
            body,
        };
        match existing {
            Some(i) => self.writes[i] = write,
            None => self.writes.push(write),
        }
        Ok(())
    }

    fn managed(&mut self, source: &str, id: &str, body: &str) -> Result<()> {
        let mut block = wrap(id, body);
        let prior = match self.writes.iter().rev().find(|w| w.source == source) {
            Some(w) => w.body.clone(),
            None => read(self.vault, source)?,
        };
        let start = format!("<!-- kakaorelay:{id}:start -->");
        let end = format!("<!-- kakaorelay:{id}:end -->");
        if !prior.is_empty() {
            let mut a = prior.find(&start);
            let b = prior.find(&end);
            if a.is_some() && prior.starts_with("---\n") {
                a = Some(0);
            }
            let manual = || {
                Error::io(format!(
                    "자동 관리 영역이 수동 수정되었습니다. 기존 내용을 보존했습니다: {source}"
                ))
            };
            let (Some(a), Some(b)) = (a, b) else {
                return Err(manual());
            };
            if b < a
                || self
                    .catalog
                    .blocks
                    .get(source)
                    .is_none_or(|expected| json::hash(&prior[a..b + end.len()]) != *expected)
            {
                return Err(manual());
            }
            block = format!("{}{}{}", &prior[..a], block, &prior[b + end.len()..]);
        } else {
            block.push_str("\n\n## 직접 메모\n\n");
        }
        self.catalog
            .blocks
            .insert(source.into(), json::hash(&wrap(id, body)));
        self.add_raw(source, block)
    }
}

/// 이전 요약에서 해당 주제 절을 찾는다(`(?ms)^## 주제\r?\n.*?(?=^## |\z)`).
fn legacy_section(body: &str, topic: &str) -> String {
    let pattern = format!(
        r"(?ms)^## {}\r?\n.*?(?=^## |\z)",
        fancy_regex::escape(topic)
    );
    fancy_regex::Regex::new(&pattern)
        .ok()
        .and_then(|r| r.find(body).ok().flatten().map(|m| m.as_str().to_string()))
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
pub fn plan(
    vault: &Path,
    room: &KnowledgeRoomSubscription,
    digest: &KnowledgeDigest,
    messages: &[LocalMessage],
    now: DateTime<FixedOffset>,
    batch: &str,
    auto_save: bool,
    legacy_source: Option<&str>,
    legacy_body: Option<&str>,
) -> Result<KnowledgePlan> {
    let mut p = Planner {
        vault,
        catalog: load(vault)?,
        writes: vec![],
    };
    let mut changed_projects: IndexSet<String> = IndexSet::new();
    let mut review = String::new();
    let mut journal = String::new();
    let (mut saved, mut reviews, mut duplicates) = (0, 0, 0);
    for claim in &digest.items {
        if claim.relation == "duplicate" {
            duplicates += 1;
            continue;
        }
        let explicit = claim.project.as_deref().filter(|p| !is_blank(p));
        let mut project = explicit
            .map(|p| p.trim().to_string())
            .unwrap_or_else(|| format!("미분류 · {}", room.title));
        let mut project_id = match explicit {
            None => id(&format!("room:{}:{}", room.profile, room.room_id)),
            Some(_) => id(&format!("project:{}", key(&project))),
        };
        let entity_id = claim.entity_id.clone().unwrap_or_else(|| {
            id(&format!(
                "{project_id}:{}:{}",
                claim.kind,
                key(&claim.topic)
            ))
        });
        let old = p.catalog.entities.get(&entity_id).cloned();
        let evidence = match legacy_source {
            Some(source) => format!(
                "- 이전 요약: {}\n\n{}",
                link(source, "원본 대화 요약"),
                legacy_section(legacy_body.unwrap_or_default(), &claim.topic)
            ),
            None => {
                let mut lines = vec![];
                for e in &claim.evidence {
                    let matching: Vec<&LocalMessage> =
                        messages.iter().filter(|m| m.id == e.message_id).collect();
                    if matching.len() != 1 {
                        return Err(Error::invalid_operation(
                            "근거 메시지를 하나로 찾을 수 없습니다.",
                        ));
                    }
                    let m = matching[0];
                    lines.push(format!(
                        "- 근거: 메시지 {} · {} · {}\n  > {}",
                        m.id,
                        time::round_trip(&m.time),
                        heading(&m.author),
                        e.quote.replace('\n', "\n  > ")
                    ));
                }
                lines.join("\n")
            }
        };
        let owner = claim
            .owner
            .clone()
            .or_else(|| old.as_ref().and_then(|o| o.owner.clone()));
        let due = claim
            .due
            .clone()
            .or_else(|| old.as_ref().and_then(|o| o.due.clone()));
        let task_state = claim
            .task_state
            .clone()
            .or_else(|| old.as_ref().and_then(|o| o.task_state.clone()));
        let topics: Vec<String> = match (&claim.tags, &old) {
            (Some(t), _) if !t.is_empty() => t.clone(),
            (_, Some(o)) if !o.tags.is_empty() => o
                .tags
                .iter()
                .filter(|t| t.starts_with("주제/"))
                .cloned()
                .collect(),
            _ => vec![claim.topic.clone()],
        };
        let entity_tags = tags(
            &claim.kind,
            old.as_ref().map(|o| o.project.as_str()).unwrap_or(&project),
            task_state.as_deref(),
            &topics,
        );
        let sorted = |v: &[String]| {
            let mut v = v.to_vec();
            v.sort_by(|a, b| ordinal_cmp(a, b));
            v
        };
        let unchanged = old.as_ref().is_some_and(|o| {
            key(&o.text) == key(&claim.text)
                && o.owner == owner
                && o.due == due
                && o.task_state == task_state
                && o.topic == claim.topic
                && sorted(&o.tags) == sorted(&entity_tags)
        });
        if unchanged {
            duplicates += 1;
            continue;
        }
        let valid_update = match &old {
            Some(o) => {
                claim.relation == "update"
                    && claim.entity_id.as_deref() == Some(o.id.as_str())
                    && claim.related_source.as_deref() == Some(o.source.as_str())
                    && claim.kind == o.kind
                    && claim.related_quote.as_deref().is_some_and(|q| {
                        !is_blank(q)
                            && read(vault, &o.source)
                                .map(|t| t.contains(q))
                                .unwrap_or(false)
                    })
            }
            None => false,
        };
        let conflict = claim.relation == "conflict"
            || (old.is_some() && !valid_update)
            || (claim.relation == "update" && !valid_update)
            || (claim.entity_id.is_some() && old.is_none());
        if !auto_save || claim.status != "confirmed" || conflict {
            reviews += 1;
            review.push_str(&format!(
                "## {}\n\n{}\n\n프로젝트: {} · 종류: {}\n검토 사유: {}\n{NEW_LINE}",
                heading(&claim.topic),
                claim.text,
                heading(&project),
                claim.kind,
                if conflict {
                    "충돌 또는 갱신 대상 확인 필요"
                } else {
                    "미확정 또는 자동 저장 꺼짐"
                }
            ));
            if let Some(source) = &claim.related_source {
                review.push_str(&format!(
                    "비교 노트: {source}\n> {}\n{NEW_LINE}",
                    claim.related_quote.as_deref().unwrap_or_default()
                ));
            }
            review.push_str(&format!("{evidence}{NEW_LINE}{NEW_LINE}"));
            continue;
        }
        if let Some(o) = &old {
            project = o.project.clone();
            project_id = o.project_id.clone();
        }
        let folder = match claim.kind.as_str() {
            "task" => "03-할일",
            "knowledge" => "02-업무지식",
            _ => "01-프로젝트",
        };
        let source = old.as_ref().map(|o| o.source.clone()).unwrap_or_else(|| {
            format!(
                "{folder}/{}-{}-{}.md",
                slug(&project),
                slug(&claim.topic),
                &entity_id[..8]
            )
        });
        let mut aliases = vec![];
        if let Some(o) = &old {
            for alias in o.aliases.iter().chain(std::iter::once(&o.topic)) {
                if alias != &claim.topic && !aliases.contains(alias) {
                    aliases.push(alias.clone());
                }
            }
        }
        let mut entity = KnowledgeEntity {
            id: entity_id.clone(),
            project_id: project_id.clone(),
            project: project.clone(),
            topic: claim.topic.clone(),
            kind: claim.kind.clone(),
            text: claim.text.clone(),
            owner,
            due,
            task_state,
            source: source.clone(),
            profile: room.profile.clone(),
            room_id: room.room_id.clone(),
            room_title: room.title.clone(),
            evidence: evidence.clone(),
            created_at: old.as_ref().map(|o| o.created_at).unwrap_or(now),
            updated_at: now,
            changes: vec![],
            tags: entity_tags,
            aliases,
        };
        if let Some(o) = &old {
            let history = format!("05-변경이력/{entity_id}-{batch}.md");
            entity.changes = o
                .changes
                .iter()
                .cloned()
                .chain(std::iter::once(history.clone()))
                .collect();
            let opt = |v: &Option<String>| v.clone().unwrap_or_default();
            let body = header(
                &format!("{} 변경 이력", claim.topic),
                "change-log",
                &[
                    "유형/변경이력".into(),
                    format!("프로젝트/{}", tag(&project)),
                ],
                &now,
            ) + &format!(
                "# {} 변경 이력\n\n시각: {}\n현재 노트: {}\n\n## 변경 전\n\n제목: {}\n태그: {}\n\n{}\n\n담당자: {} · 기한: {} · 상태: {}\n\n{}\n\n## 변경 후\n\n제목: {}\n태그: {}\n\n{}\n\n담당자: {} · 기한: {} · 상태: {}\n\n{}\n",
                heading(&claim.topic),
                time::round_trip(&now),
                link(&source, &claim.topic),
                heading(&o.topic),
                o.tags.join(", "),
                o.text,
                opt(&o.owner),
                opt(&o.due),
                opt(&o.task_state),
                o.evidence,
                heading(&entity.topic),
                entity.tags.join(", "),
                entity.text,
                opt(&entity.owner),
                opt(&entity.due),
                opt(&entity.task_state),
                evidence
            );
            p.add_raw(&history, body)?;
        }
        let rendered = render(&entity);
        p.catalog.entities.insert(entity_id.clone(), entity);
        p.managed(&source, &entity_id, &rendered)?;
        changed_projects.insert(project_id);
        saved += 1;
        journal.push_str(&format!(
            "- {}: {}\n\n  {}\n\n{}\n{NEW_LINE}",
            if old.is_none() { "추가" } else { "갱신" },
            link(&source, &claim.topic),
            claim.text,
            evidence
        ));
    }
    for project_id in &changed_projects {
        let mut entities: Vec<KnowledgeEntity> = p
            .catalog
            .entities
            .values()
            .filter(|e| &e.project_id == project_id)
            .cloned()
            .collect();
        entities.sort_by(|a, b| {
            ordinal_cmp(&a.kind, &b.kind).then_with(|| ordinal_cmp(&a.topic, &b.topic))
        });
        let project = entities[0].project.clone();
        let source = format!(
            "01-프로젝트/프로젝트-{}-{}.md",
            slug(&project),
            &project_id[..8]
        );
        let mut body = format!(
            "---\nid: {project_id}\ntype: project-index\ntitle: {}\ntags: {}\nproject: {}\nupdated: {}\n---\n# {}\n",
            property(&project),
            property(&[
                "유형/프로젝트".to_string(),
                format!("프로젝트/{}", tag(&project))
            ]),
            property(&project),
            time::round_trip(&now),
            heading(&project)
        );
        for (kind, label) in [
            ("project", "목표·현재 상태"),
            ("decision", "결정사항"),
            ("task", "할 일"),
            ("knowledge", "업무 지식"),
        ] {
            body.push_str(&format!("\n## {label}\n{NEW_LINE}"));
            for e in entities.iter().filter(|e| e.kind == kind) {
                let check = if e.kind == "task" {
                    if matches!(e.task_state.as_deref(), Some("done" | "cancelled")) {
                        "[x] "
                    } else {
                        "[ ] "
                    }
                } else {
                    ""
                };
                let meta = if e.kind == "task" {
                    format!(
                        " · {} · {} · {}",
                        e.owner.as_deref().unwrap_or("담당 미정"),
                        e.due.as_deref().unwrap_or("기한 미정"),
                        e.task_state.as_deref().unwrap_or("상태 미정")
                    )
                } else {
                    String::new()
                };
                body.push_str(&format!(
                    "- {check}{} — {}{meta}{NEW_LINE}",
                    link(&e.source, &e.topic),
                    heading(&e.text)
                ));
            }
        }
        p.managed(&source, &format!("project-{project_id}"), &body)?;
    }
    let day = now.format("%Y-%m-%d").to_string();
    if saved > 0 {
        let body = header(
            &format!("{day} {} 대화 정리", room.title),
            "conversation-log",
            &["유형/대화기록".into()],
            &now,
        ) + &format!(
            "# {} 대화 정리\n\n시각: {}\n계정: {}\n방 ID: {}\n\n{journal}",
            heading(&room.title),
            time::round_trip(&now),
            room.profile,
            room.room_id
        );
        p.add_raw(
            &format!("04-대화기록/{day}-{}-{batch}.md", slug(&room.title)),
            body,
        )?;
    }
    if reviews > 0 {
        let body = header(
            &format!("{} 검토할 항목", room.title),
            "review",
            &["유형/검토".into(), "상태/검토필요".into()],
            &now,
        ) + &format!(
            "# {} · 검토할 항목\n\n> 봇 검색에서 제외됩니다.\n\n{review}",
            heading(&room.title)
        );
        p.add_raw(&format!("검토필요/{batch}.md"), body)?;
    }
    if let Some(source) = legacy_source {
        p.catalog.migrated.insert(source.into());
    }
    if !p.writes.is_empty() || legacy_source.is_some() {
        let index = json::indented(&p.catalog);
        p.add_raw(INDEX_PATH, index)?;
    }
    Ok(KnowledgePlan {
        writes: p.writes,
        saved,
        review: reviews,
        duplicates,
    })
}

fn render(e: &KnowledgeEntity) -> String {
    let task = if e.kind == "task" {
        format!(
            "- [{}] {}\n담당자: {} · 기한: {} · 상태: {}",
            if matches!(e.task_state.as_deref(), Some("done" | "cancelled")) {
                "x"
            } else {
                " "
            },
            heading(&e.topic),
            e.owner.as_deref().unwrap_or("미정"),
            e.due.as_deref().unwrap_or("미정"),
            e.task_state.as_deref().unwrap_or("미정")
        )
    } else {
        String::new()
    };
    let changes = if e.changes.is_empty() {
        "아직 변경 이력이 없습니다.".to_string()
    } else {
        e.changes
            .iter()
            .enumerate()
            .map(|(i, s)| format!("- {}", link(s, &format!("변경 {}", i + 1))))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "---\nid: {}\ntitle: {}\naliases: {}\ntype: {}\ntags: {}\nproject: {}\nowner: {}\ndue: {}\nstatus: {}\nsource_room: {}\nsource_room_id: {}\nsource_profile: {}\ncreated: {}\nupdated: {}\n---\n# {}\n\n## 현재 내용\n\n{}\n\n{}\n\n프로젝트: {}\n\n## 근거\n\n{}\n\n## 변경 이력\n\n{}",
        e.id,
        property(&e.topic),
        property(&e.aliases),
        e.kind,
        property(&e.tags),
        property(&e.project),
        property(&e.owner),
        property(&e.due),
        e.task_state.as_deref().unwrap_or("confirmed"),
        property(&e.room_title),
        property(&e.room_id),
        property(&e.profile),
        time::round_trip(&e.created_at),
        time::round_trip(&e.updated_at),
        heading(&e.topic),
        e.text,
        task,
        link(
            &format!(
                "01-프로젝트/프로젝트-{}-{}.md",
                slug(&e.project),
                &e.project_id[..8]
            ),
            &e.project
        ),
        e.evidence,
        changes
    )
}

const MANAGED_PREFIXES: [&str; 6] = [
    "01-프로젝트/",
    "02-업무지식/",
    "03-할일/",
    "04-대화기록/",
    "05-변경이력/",
    "검토필요/",
];

fn validate_write(vault: &Path, write: &KnowledgeWrite) -> Result<()> {
    if write.source != INDEX_PATH && !MANAGED_PREFIXES.iter().any(|p| write.source.starts_with(p)) {
        return Err(Error::io("자동 관리 경로가 아닙니다."));
    }
    let current = hash_or_null(vault, &write.source)?;
    if current != write.before && current.as_deref() != Some(json::hash(&write.body).as_str()) {
        return Err(Error::io(format!(
            "저장 도중 노트가 수정되어 덮어쓰지 않았습니다: {}",
            write.source
        )));
    }
    if write.source.ends_with(".md") && write.body.len() > MAX_FILE_BYTES {
        return Err(Error::io(format!(
            "관리 노트가 너무 큽니다. 프로젝트를 나누어 주세요: {}",
            write.source
        )));
    }
    Ok(())
}

/// 계획 전체를 먼저 검증하고, 이미 반영된 내용은 건너뛰어 반복 적용도 받아들인다.
pub fn apply(vault: &Path, writes: &[KnowledgeWrite]) -> Result<()> {
    for write in writes {
        validate_write(vault, write)?;
    }
    for write in writes {
        let path = safe_path(vault, &write.source)?;
        if hash_or_null(vault, &write.source)?.as_deref() == Some(json::hash(&write.body).as_str())
        {
            continue;
        }
        validate_write(vault, write)?;
        let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
        std::fs::create_dir_all(&parent)?;
        check_directory(&parent)?;
        let temp =
            crate::settings::path_with_suffix(&path, &format!(".{}.tmp", Uuid::new_v4().simple()));
        std::fs::write(&temp, write.body.as_bytes())?;
        std::fs::rename(&temp, &path)?;
    }
    Ok(())
}

fn hash_or_null(vault: &Path, source: &str) -> Result<Option<String>> {
    let path = safe_path(vault, source)?;
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(json::hash(&crate::text::read_text(&path)?)))
}

fn read(vault: &Path, source: &str) -> Result<String> {
    let path = safe_path(vault, source)?;
    if !path.is_file() {
        return Ok(String::new());
    }
    Ok(crate::text::read_text(&path)?)
}

fn safe_path(vault: &Path, source: &str) -> Result<PathBuf> {
    let root = absolute(vault);
    check_directory(&root)?;
    let path = absolute(&root.join(source));
    let root_text = root
        .to_string_lossy()
        .trim_end_matches(std::path::MAIN_SEPARATOR)
        .to_string()
        + std::path::MAIN_SEPARATOR_STR;
    if !path
        .to_string_lossy()
        .to_lowercase()
        .starts_with(&root_text.to_lowercase())
    {
        return Err(Error::io("보관함 밖 경로입니다."));
    }
    let mut parent = path.parent().map(Path::to_path_buf);
    while let Some(dir) = parent {
        if !dir
            .to_string_lossy()
            .to_lowercase()
            .starts_with(&root.to_string_lossy().to_lowercase())
        {
            break;
        }
        if dir.exists() && is_link(&dir) {
            return Err(Error::io("연결된 폴더에는 쓸 수 없습니다."));
        }
        parent = dir.parent().map(Path::to_path_buf);
    }
    if path.exists() && is_link(&path) {
        return Err(Error::io("연결된 파일에는 쓸 수 없습니다."));
    }
    Ok(path)
}

static LEGACY_ROOM: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\A# (.+) 대화 자동 정리\r?\n").expect("정규식"));
static LEGACY_PROFILE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^계정: (.+)$").expect("정규식"));
static LEGACY_ROOM_ID: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^대화방 ID: (.+)$").expect("정규식"));
static LEGACY_CLAIM: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(r"(?ms)^## ([^\r\n]+)\r?\n\s*(.*?)\r?\n> 종류: (knowledge|decision|task) · 대화에서 확인된 내용.*?(?=^## |\z)").expect("정규식")
});

/// 복구 기록을 마저 적용하고, 관리 폴더를 만들고, 이전 `자동정리/` 요약을 한 번만 구조화 노트로 옮긴다(원본은 그대로 둔다).
pub fn ensure(vault: &Path) -> Result<()> {
    let pending = safe_path(vault, RECOVERY_PATH)?;
    if pending.exists() {
        let writes: Option<Vec<KnowledgeWrite>> =
            serde_json::from_str(&crate::text::read_text(&pending)?)?;
        apply(
            vault,
            &writes.ok_or_else(|| Error::io("이전 구조 전환 기록 오류"))?,
        )?;
        std::fs::remove_file(&pending)?;
    }
    for folder in [
        ".kakaorelay",
        "01-프로젝트",
        "02-업무지식",
        "03-할일",
        "04-대화기록",
        "05-변경이력",
        "검토필요",
    ] {
        let path = safe_path(vault, folder)?;
        std::fs::create_dir_all(&path)?;
        check_directory(&path)?;
    }
    let legacy = vault.join("자동정리");
    if !legacy.is_dir() {
        return Ok(());
    }
    check_directory(&legacy)?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&legacy)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    for file in files {
        let source = format!(
            "자동정리/{}",
            file.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        if load(vault)?.migrated.contains(&source) {
            continue;
        }
        let text = read(vault, &source)?;
        let Some(room) = LEGACY_ROOM.captures(&text) else {
            continue;
        }; // 알 수 없는·사용자 노트는 검색 가능한 원본으로 둔다.
        let profile = LEGACY_PROFILE
            .captures(&text)
            .map(|c| c[1].trim().to_string())
            .unwrap_or_default();
        let room_id = LEGACY_ROOM_ID
            .captures(&text)
            .map(|c| c[1].trim().to_string())
            .unwrap_or_default();
        let mut claims = vec![];
        for captures in LEGACY_CLAIM.captures_iter(&text).flatten() {
            claims.push(KnowledgeClaim::new(
                &captures[1],
                captures[2].trim(),
                &captures[3],
                "confirmed",
                vec![],
            ));
        }
        if claims.is_empty() {
            continue;
        }
        let subscription = KnowledgeRoomSubscription::new(profile, room_id, &room[1], Uuid::nil());
        let plan = plan(
            vault,
            &subscription,
            &KnowledgeDigest { items: claims },
            &[],
            Utc::now().fixed_offset(),
            &format!("migration-{}", id(&source)),
            true,
            Some(&source),
            Some(&text),
        )?;
        std::fs::write(&pending, json::indented(&plan.writes))?;
        apply(vault, &plan.writes)?;
        std::fs::remove_file(&pending)?;
    }
    Ok(())
}
