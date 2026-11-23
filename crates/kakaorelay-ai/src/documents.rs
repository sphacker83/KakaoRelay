//! 원본: `KnowledgeDocuments.cs`. 보관함 노트 목록과 MD·TXT·PDF·DOCX 가져오기.
//! PDF는 PdfPig 대신 `pdf_oxide`로 추출한다(한글 품질 비교는 이식 보고 참조). 200쪽 제한은 그대로다.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use chrono::Local;

use crate::cancel::CancellationToken;
use crate::error::{Error, Result};
use crate::obsidian::{self, MAX_FILE_BYTES, MAX_FILES, check_directory, is_link};
use crate::provider_storage::absolute;
use crate::text::{NEW_LINE, is_blank, ordinal_ignore_case_cmp, utf16_len, utf16_prefix};

pub const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_TEXT_CHARACTERS: usize = 2_000_000;
pub const EXTENSIONS: [&str; 4] = [".md", ".txt", ".pdf", ".docx"];
const PART_CHARACTERS: usize = 50_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeNote {
    pub path: PathBuf,
    pub source: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeLibrary {
    pub notes: Vec<KnowledgeNote>,
    pub skipped: usize,
    pub limited: bool,
}

fn cancelled(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

async fn blocking<T: Send + 'static>(
    cancel: &CancellationToken,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    cancelled(cancel)?;
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| Error::invalid_operation(e.to_string()))?
}

pub async fn list(vault: &Path, cancel: &CancellationToken) -> Result<KnowledgeLibrary> {
    let vault = vault.to_path_buf();
    let token = cancel.clone();
    blocking(cancel, move || list_blocking(&vault, &token)).await
}

fn list_blocking(vault: &Path, cancel: &CancellationToken) -> Result<KnowledgeLibrary> {
    let root = absolute(vault);
    check_directory(&root)?;
    let migrated = crate::structured::load(&root)?.migrated;
    let mut notes = vec![];
    let mut pending = vec![root.clone()];
    let (mut entries, mut skipped) = (0usize, 0usize);
    let mut limited = false;
    while let Some(directory) = pending.pop() {
        if limited {
            break;
        }
        cancelled(cancel)?;
        let listing = match std::fs::read_dir(&directory) {
            Ok(listing) => listing,
            Err(error) if directory == root => return Err(error.into()),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        for entry in listing {
            let Ok(entry) = entry else {
                skipped += 1;
                break;
            };
            cancelled(cancel)?;
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
                pending.push(path);
                continue;
            }
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                continue;
            }
            let source = obsidian::relative_source(&root, &path);
            if crate::structured::historical(&source) || migrated.contains(&source) {
                continue;
            }
            if notes.len() == MAX_FILES {
                limited = true;
                break;
            }
            let bytes = std::fs::metadata(&path)?.len();
            if bytes as usize > MAX_FILE_BYTES {
                skipped += 1;
                continue;
            }
            notes.push(KnowledgeNote {
                path,
                source,
                bytes,
            });
        }
    }
    notes.sort_by(|a, b| ordinal_ignore_case_cmp(&a.source, &b.source));
    Ok(KnowledgeLibrary {
        notes,
        skipped,
        limited,
    })
}

fn check_text_length(count: usize) -> Result<()> {
    if count > MAX_TEXT_CHARACTERS {
        return Err(Error::argument(
            "추출한 내용이 200만 자를 넘습니다. 문서를 나누어 추가하세요.",
        ));
    }
    Ok(())
}

/// 숨김 폴더에 문서 전체를 준비한 뒤 한 번에 옮긴다. 원본 파일은 건드리지 않고, 반복 가져오기는 매번 다른 폴더가 된다.
pub async fn import(
    vault: &Path,
    source: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<PathBuf>> {
    let (vault, source) = (vault.to_path_buf(), source.to_path_buf());
    let token = cancel.clone();
    blocking(cancel, move || import_blocking(&vault, &source, &token)).await
}

fn import_blocking(
    vault: &Path,
    source: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<PathBuf>> {
    cancelled(cancel)?;
    let root = absolute(vault);
    check_directory(&root)?;
    let extension = source
        .extension()
        .map(|e| format!(".{}", crate::text::lower_invariant(&e.to_string_lossy())))
        .unwrap_or_default();
    if !EXTENSIONS.contains(&extension.as_str()) {
        return Err(Error::argument("MD, TXT, PDF, DOCX 파일을 선택하세요."));
    }
    let mut input = std::fs::File::open(source)?;
    if input.metadata()?.len() as usize > MAX_UPLOAD_BYTES {
        return Err(Error::argument("파일당 20MB 이하만 추가할 수 있습니다."));
    }
    let mut bytes = vec![];
    input.read_to_end(&mut bytes)?;
    let body = match extension.as_str() {
        ".pdf" => pdf_text(bytes, cancel)?,
        ".docx" => docx_text(&bytes, cancel)?,
        _ => plain_text(&bytes)?,
    };
    if is_blank(&body) {
        return Err(Error::argument(
            "추출할 텍스트가 없습니다. 스캔 PDF·이미지는 먼저 문자 인식(OCR)이 필요합니다.",
        ));
    }
    cancelled(cancel)?;
    let imports = root.join("자료");
    std::fs::create_dir_all(&imports)?;
    check_directory(&imports)?;
    let title = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let safe_title = safe_title(&title);
    let id = format!(
        "{safe_title}-{}-{}",
        Local::now().format("%Y%m%d-%H%M%S"),
        uuid::Uuid::new_v4().simple()
    );
    let stage = imports.join(format!(".import-{}", uuid::Uuid::new_v4().simple()));
    let destination = imports.join(id);
    std::fs::create_dir_all(&stage)?;
    let file_name = source
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let result = (|| -> Result<Vec<PathBuf>> {
        let mut names = vec![];
        let mut rest = body.as_str();
        let mut part = 1;
        while !rest.is_empty() {
            cancelled(cancel)?;
            // 서로게이트 쌍은 나누지 않는다(원본의 high surrogate 보정과 같다).
            let mut piece = utf16_prefix(rest, PART_CHARACTERS);
            if piece.is_empty() {
                piece = &rest[..rest
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(rest.len())];
            }
            let name = format!("{part:03}.md");
            let text = format!(
                "# {} · {part}\n\n> 원본: {file_name}\n\n{piece}",
                crate::text::single_line(&title)
            );
            std::fs::write(stage.join(&name), text.as_bytes())?;
            names.push(name);
            rest = &rest[piece.len()..];
            part += 1;
        }
        cancelled(cancel)?;
        std::fs::rename(&stage, &destination)?;
        Ok(names.into_iter().map(|n| destination.join(n)).collect())
    })();
    if stage.exists() {
        let _ = std::fs::remove_dir_all(&stage);
    }
    result
}

static LETTER_OR_DIGIT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[\p{L}\p{Nd}]$").expect("정규식"));

/// `title.Take(60)`과 `char.IsLetterOrDigit`. UTF-16 단위라 BMP 밖 문자는 단위마다 `_`가 된다.
fn safe_title(title: &str) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in title.chars() {
        if used >= 60 {
            break;
        }
        let units = c.len_utf16().min(60 - used);
        used += units;
        let keep = c.len_utf16() == 1
            && (LETTER_OR_DIGIT.is_match(c.encode_utf8(&mut [0; 4])) || matches!(c, '-' | '_'));
        if keep {
            out.push(c);
        } else {
            out.push_str(&"_".repeat(units));
        }
    }
    out
}

/// `StreamReader(new UTF8Encoding(false, true), detectEncodingFromByteOrderMarks: true)`: 잘못된 UTF-8은 거부한다.
fn plain_text(bytes: &[u8]) -> Result<String> {
    let invalid = || Error::argument("텍스트 파일이 아닙니다. UTF-8 텍스트로 저장해서 추가하세요.");
    let body = if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        crate::text::decode(bytes)
    } else {
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        String::from_utf8(bytes.to_vec()).map_err(|_| invalid())?
    };
    check_text_length(utf16_len(&body))?;
    if body.contains('\0') {
        return Err(invalid());
    }
    Ok(body)
}

fn pdf_text(bytes: Vec<u8>, cancel: &CancellationToken) -> Result<String> {
    let unreadable =
        |e: pdf_oxide::Error| Error::invalid_data(format!("PDF를 읽을 수 없습니다: {e}"));
    let document = pdf_oxide::PdfDocument::from_bytes(bytes).map_err(unreadable)?;
    let pages = document.page_count().map_err(unreadable)?;
    if pages > 200 {
        return Err(Error::argument("PDF는 200쪽 이하로 나누어 추가하세요."));
    }
    let mut text = String::new();
    for index in 0..pages {
        cancelled(cancel)?;
        let content = document.extract_text(index).map_err(unreadable)?;
        if is_blank(&content) {
            continue;
        }
        text.push_str(&format!(
            "\n## {}쪽\n{NEW_LINE}{content}{NEW_LINE}",
            index + 1
        ));
        check_text_length(utf16_len(&text))?;
    }
    Ok(text)
}

const WORD_NS: &[u8] = b"http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// 이름공간을 풀어 둔 최소 XML 트리. 원본의 `XDocument.Descendants` 순회를 그대로 따르기 위해 쓴다.
struct Node {
    word: bool,
    local: Vec<u8>,
    text: String,
    children: Vec<usize>,
}

fn docx_text(bytes: &[u8], cancel: &CancellationToken) -> Result<String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| Error::invalid_data(e.to_string()))?;
    let mut entry = archive
        .by_name("word/document.xml")
        .map_err(|_| Error::argument("Word 문서 내용을 찾을 수 없습니다."))?;
    if entry.size() as usize > MAX_UPLOAD_BYTES {
        return Err(Error::argument(
            "Word 문서의 압축 해제 내용이 20MB를 넘습니다.",
        ));
    }
    let mut xml = vec![];
    entry
        .by_ref()
        .take(MAX_UPLOAD_BYTES as u64 + 1)
        .read_to_end(&mut xml)?;
    if xml.len() > MAX_UPLOAD_BYTES {
        return Err(Error::argument(
            "Word 문서의 압축 해제 내용이 20MB를 넘습니다.",
        ));
    }
    let nodes = parse_xml(&xml)?;
    let is = |n: &Node, name: &[u8]| n.word && n.local == name;
    let mut text = String::new();
    // 문서 순서의 모든 w:p(중첩 문단 포함)마다 하위의 w:t·w:tab·w:br을 이어 붙인다.
    for index in descendants(&nodes, 0) {
        if !is(&nodes[index], b"p") {
            continue;
        }
        cancelled(cancel)?;
        for child in descendants(&nodes, index) {
            let node = &nodes[child];
            if is(node, b"t") {
                text.push_str(&node.text);
            } else if is(node, b"tab") {
                text.push('\t');
            } else if is(node, b"br") {
                text.push_str(NEW_LINE);
            }
        }
        text.push_str(NEW_LINE);
        check_text_length(utf16_len(&text))?;
    }
    Ok(text)
}

fn descendants(nodes: &[Node], index: usize) -> Vec<usize> {
    let mut result = vec![];
    let mut stack: Vec<usize> = nodes[index].children.iter().rev().copied().collect();
    while let Some(i) = stack.pop() {
        result.push(i);
        stack.extend(nodes[i].children.iter().rev());
    }
    result
}

fn parse_xml(xml: &[u8]) -> Result<Vec<Node>> {
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;
    let bad = |e: &dyn std::fmt::Display| Error::invalid_data(format!("Word 문서 XML 오류: {e}"));
    let mut reader = quick_xml::NsReader::from_reader(xml);
    let mut nodes = vec![Node {
        word: false,
        local: vec![],
        text: String::new(),
        children: vec![],
    }];
    let mut stack = vec![0usize];
    let mut buffer = vec![];
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|e| bad(&e))?;
        let parent = *stack.last().unwrap_or(&0);
        let open = |start: &quick_xml::events::BytesStart, nodes: &mut Vec<Node>| {
            let word = matches!(namespace, ResolveResult::Bound(ref ns) if ns.as_ref() == WORD_NS);
            nodes.push(Node {
                word,
                local: start.local_name().as_ref().to_vec(),
                text: String::new(),
                children: vec![],
            });
            let index = nodes.len() - 1;
            nodes[parent].children.push(index);
            index
        };
        match event {
            Event::Start(start) => {
                let index = open(&start, &mut nodes);
                stack.push(index);
            }
            Event::Empty(start) => {
                open(&start, &mut nodes);
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Text(text) => {
                let value = text.decode().map_err(|e| bad(&e))?;
                let value = quick_xml::escape::unescape(&value).map_err(|e| bad(&e))?;
                nodes[parent].text.push_str(&value);
            }
            Event::GeneralRef(reference) => {
                let name = reference.decode().map_err(|e| bad(&e))?;
                let resolved = match reference.resolve_char_ref().map_err(|e| bad(&e))? {
                    Some(c) => c.to_string(),
                    None => quick_xml::escape::resolve_xml_entity(&name)
                        .ok_or_else(|| bad(&format!("알 수 없는 엔터티 {name}")))?
                        .to_string(),
                };
                nodes[parent].text.push_str(&resolved);
            }
            Event::CData(data) => nodes[parent].text.push_str(&String::from_utf8_lossy(&data)),
            // XmlReaderSettings.DtdProcessing = Prohibit와 같다.
            Event::DocType(_) => return Err(bad(&"DTD는 허용하지 않습니다.")),
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(nodes)
}
