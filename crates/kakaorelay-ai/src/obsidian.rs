//! 원본: `ObsidianKnowledge.cs`. 보관함 Markdown을 직접 읽어 검색하고 노트를 저장한다.
//! 구조화 카탈로그는 현재 노트와 이전 형식 이관 원본을 가리는 데만 쓴다.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::ai_service::AiCommand;
use crate::cancel::CancellationToken;
use crate::chat_models::{ChatContext, LocalRoom};
use crate::error::{Error, Result};
use crate::provider_storage::absolute;
use crate::settings::KnowledgeSettings;
use crate::text::{is_blank, lower_invariant, ordinal_cmp, utf16_len, utf16_prefix};

pub const MAX_FILES: usize = 2000;
pub const MAX_FILE_BYTES: usize = 256 * 1024;
pub const MAX_CHARACTERS: usize = 12000;
const CHUNK_SIZE: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeExcerpt {
    pub source: String,
    pub part: i32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KnowledgeSearch {
    pub excerpts: Vec<KnowledgeExcerpt>,
    pub notes: usize,
    pub skipped: usize,
    pub limited: bool,
}

/// .NET `FileAttributes.ReparsePoint`: 심볼릭 링크(Windows는 정션 등 재분석 지점 포함).
pub fn is_link(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// 보관함이 있는지, 그리고 경로의 어느 단계도 링크가 아닌지 확인한다.
pub fn check_directory(root: &Path) -> Result<()> {
    if !root.is_dir() {
        return Err(Error::DirectoryNotFound(
            "지식베이스 보관함을 찾을 수 없습니다. 폴더 경로를 확인하세요.".into(),
        ));
    }
    let mut current = Some(absolute(root));
    while let Some(dir) = current {
        if is_link(&dir) {
            return Err(Error::io(
                "지식베이스에는 바로 가기나 심볼릭 링크가 아닌 실제 폴더를 선택하세요.",
            ));
        }
        current = dir.parent().map(Path::to_path_buf);
    }
    Ok(())
}

pub fn query(context: &ChatContext, command: &AiCommand) -> String {
    if let Some(id) = &command.target_message_id {
        return context
            .messages
            .iter()
            .rev()
            .find(|m| &m.id == id && !m.deleted)
            .map(|m| m.text.clone())
            .unwrap_or_default();
    }
    if !is_blank(&command.instruction) {
        return command.instruction.clone();
    }
    let live: Vec<&str> = context
        .messages
        .iter()
        .filter(|m| !m.deleted)
        .map(|m| m.text.as_str())
        .collect();
    live[live.len().saturating_sub(3)..].join("\n")
}

pub async fn search(
    settings: &KnowledgeSettings,
    query: &str,
    cancel: &CancellationToken,
) -> Result<KnowledgeSearch> {
    settings.validate()?;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if !settings.enabled {
        return Ok(KnowledgeSearch::default());
    }
    let settings = settings.clone();
    let query = query.to_string();
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || search_blocking(&settings, &query, &cancel))
        .await
        .map_err(|e| Error::invalid_operation(e.to_string()))?
}

pub(crate) fn search_blocking(
    settings: &KnowledgeSettings,
    query: &str,
    cancel: &CancellationToken,
) -> Result<KnowledgeSearch> {
    settings.validate()?;
    if !settings.enabled {
        return Ok(KnowledgeSearch::default());
    }
    let root = absolute(Path::new(&settings.vault_path));
    check_directory(&root)?;
    let migrated = crate::structured::load(&root)?.migrated;
    let terms = terms(utf16_prefix(query, 8000));
    let mut best: Vec<(KnowledgeExcerpt, usize)> = vec![];
    let mut directories = vec![root.clone()];
    let (mut notes, mut skipped, mut entries, mut characters) = (0usize, 0usize, 0usize, 0usize);
    let mut limited = false;
    while let Some(directory) = directories.pop() {
        if limited {
            break;
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let listing = match std::fs::read_dir(&directory) {
            Ok(listing) => listing,
            Err(_) if directory == root => {
                return Err(Error::io("지식베이스 보관함을 읽을 수 없습니다."));
            }
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        for entry in listing.flatten() {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            entries += 1;
            if entries > 10000 {
                limited = true;
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.')
                || (directory == root
                    && matches!(name.as_str(), "검토필요" | "04-대화기록" | "05-변경이력"))
            {
                continue;
            }
            if is_link(&path) {
                skipped += 1;
                continue;
            }
            if path.is_dir() {
                directories.push(path);
                continue;
            }
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                continue;
            }
            let source = relative_source(&root, &path);
            if migrated.contains(&source) {
                continue;
            }
            if notes >= MAX_FILES || characters >= 16_000_000 {
                limited = true;
                break;
            }
            let Ok(metadata) = std::fs::metadata(&path) else {
                skipped += 1;
                continue;
            };
            if metadata.len() as usize > MAX_FILE_BYTES {
                skipped += 1;
                continue;
            }
            let Ok(text) = crate::text::read_text(&path) else {
                skipped += 1;
                continue;
            };
            let length = utf16_len(&text);
            if length > MAX_FILE_BYTES {
                skipped += 1;
                continue;
            }
            notes += 1;
            characters += length;
            let title_terms = self::terms(&source);
            for (index, chunk) in chunks(&text, CHUNK_SIZE).into_iter().enumerate() {
                let body_terms = self::terms(chunk);
                let score: usize = terms.iter().map(|t| if title_terms.contains(t) { 3 } else { 0 } + if body_terms.contains(t) { 1 } else { 0 }).sum();
                if score == 0 {
                    continue;
                }
                best.push((
                    KnowledgeExcerpt {
                        source: source.clone(),
                        part: index as i32 + 1,
                        text: chunk.to_string(),
                    },
                    score,
                ));
                best.sort_by(|a, b| {
                    let current =
                        |x: &(KnowledgeExcerpt, usize)| crate::structured::current(&x.0.source);
                    current(b)
                        .cmp(&current(a))
                        .then(b.1.cmp(&a.1))
                        .then_with(|| ordinal_cmp(&a.0.source, &b.0.source))
                        .then(a.0.part.cmp(&b.0.part))
                });
                best.truncate(6);
            }
        }
    }
    Ok(KnowledgeSearch {
        excerpts: best.into_iter().map(|x| x.0).collect(),
        notes,
        skipped,
        limited,
    })
}

/// 보관함 기준 상대 경로(`/` 구분).
pub(crate) fn relative_source(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// `text.Substring(offset, ChunkSize)` 반복. 서로게이트 쌍은 나누지 않는다.
fn chunks(text: &str, size: usize) -> Vec<&str> {
    let mut result = vec![];
    let mut rest = text;
    while !rest.is_empty() {
        let mut piece = utf16_prefix(rest, size);
        if piece.is_empty() {
            piece = &rest[..rest
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(rest.len())];
        }
        result.push(piece);
        rest = &rest[piece.len()..];
    }
    result
}

static WORD: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[\p{L}\p{N}]+").expect("정규식"));

fn terms(text: &str) -> HashSet<String> {
    let mut result = HashSet::new();
    let lower = lower_invariant(text);
    for m in WORD.find_iter(&lower) {
        let word = m.as_str();
        if utf16_len(word) > 1 {
            result.insert(word.to_string());
        }
        // 한국어 조사 때문에 '휴가는'이 노트의 '휴가'와 맞지 않는 문제를 두 글자 조각으로 푼다.
        let chars: Vec<char> = word.chars().collect();
        if chars.iter().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(c)) {
            for pair in chars.windows(2) {
                result.insert(pair.iter().collect());
            }
        }
    }
    result
}

pub async fn save_note(
    settings: &KnowledgeSettings,
    title: &str,
    body: &str,
    room: Option<&LocalRoom>,
    cancel: &CancellationToken,
) -> Result<PathBuf> {
    settings.validate()?;
    if is_blank(&settings.vault_path) {
        return Err(Error::argument("먼저 옵시디언 보관함을 선택하세요."));
    }
    if is_blank(title) || utf16_len(title) > 200 || is_blank(body) || utf16_len(body) > 100_000 {
        return Err(Error::argument(
            "노트 제목(200자 이하)과 내용(10만 자 이하)을 확인하세요.",
        ));
    }
    let root = absolute(Path::new(&settings.vault_path));
    check_directory(&root)?;
    let folder = root.join("KakaoRelay");
    std::fs::create_dir_all(&folder)?;
    check_directory(&folder)?;
    let now = Local::now().fixed_offset();
    let path = folder.join(format!(
        "{}-{}.md",
        now.format("%Y%m%d-%H%M%S"),
        uuid::Uuid::new_v4().simple()
    ));
    let heading = crate::text::single_line(title);
    let metadata = match room {
        None => "사용자 작성 노트".to_string(),
        Some(room) => format!(
            "대화 분석 결과 · 방: {}",
            crate::text::single_line(&room.title)
        ),
    };
    let text = format!(
        "# {heading}\n\n> {metadata}\n> 저장: {}\n\n{}\n",
        crate::json::time::round_trip(&now),
        body.trim()
    );
    if text.len() > MAX_FILE_BYTES {
        return Err(Error::argument(
            "노트가 256KB를 넘습니다. 요약을 나누어 저장하세요.",
        ));
    }
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let temporary = crate::settings::path_with_suffix(&path, ".tmp");
    let result = (|| -> Result<PathBuf> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        drop(file);
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if path.exists() {
            return Err(Error::io("같은 이름의 노트가 이미 있습니다."));
        }
        std::fs::rename(&temporary, &path)?;
        Ok(path.clone())
    })();
    if temporary.exists() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
