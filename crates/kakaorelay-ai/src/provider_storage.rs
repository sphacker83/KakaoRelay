//! 원본: `ProviderStorage.cs`. 앱 전용 CLI 홈(`data/providers/<p>`), 자식 환경변수 주입,
//! 로그인 파일 최초 복사·Codex 로그인 동기화, 이전 버전 세션 이전.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::conversation::AiConversation;
use crate::error::{Error, Result};
use crate::json;
use crate::process::CliCommand;
use crate::settings::ORDER;

static SYNC: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    SYNC.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn root() -> PathBuf {
    crate::paths::provider_data().join("providers")
}

pub fn home(provider: &str) -> Result<PathBuf> {
    if !ORDER.contains(&provider) {
        return Err(Error::argument("지원하지 않는 프로바이더입니다."));
    }
    Ok(root().join(provider))
}

fn home_variable(provider: &str) -> &'static str {
    match provider {
        "codex" => "CODEX_HOME",
        "grok" => "GROK_HOME",
        _ => "CLAUDE_CONFIG_DIR",
    }
}

fn legacy_home(provider: &str) -> PathBuf {
    match std::env::var(home_variable(provider)) {
        Ok(configured) if !crate::text::is_blank(&configured) => absolute(Path::new(&configured)),
        _ => crate::paths::user_profile().join(format!(".{provider}")),
    }
}

/// `Path.GetFullPath`. 존재하지 않는 경로도 다룰 수 있게 정규화만 한다.
pub fn absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

pub fn initialize() -> Result<()> {
    for provider in ORDER {
        ensure(provider)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn ensure(provider: &str) -> Result<()> {
    let _guard = lock();
    ensure_locked(provider)
}

fn ensure_locked(provider: &str) -> Result<()> {
    let home = home(provider)?;
    std::fs::create_dir_all(&home)?;
    #[cfg(unix)]
    set_mode(&home, 0o700)?;
    let marker = home.join(".initialized-v1");
    if marker.exists() {
        return Ok(());
    }
    let auth = if provider == "claude" {
        ".credentials.json"
    } else {
        "auth.json"
    };
    copy_file(&legacy_home(provider).join(auth), &home.join(auth))?;
    // 개인 훅·플러그인·엔드포인트 설정과 상태 DB 경로는 물려받지 않는다.
    let config = home.join("config.toml");
    if provider == "codex" && !config.exists() {
        std::fs::write(&config, "cli_auth_credentials_store = \"file\"\n")?;
    }
    if provider == "grok" && !config.exists() {
        std::fs::write(&config, "[cli]\nauto_update = false\n")?;
    }
    std::fs::write(&marker, "KakaoRelay provider storage v1\n")?;
    Ok(())
}

pub fn job_folder(provider: &str, kind: &str) -> Result<PathBuf> {
    ensure(provider)?;
    let path = home(provider)?
        .join("jobs")
        .join(format!("{kind}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn claude_project(cwd: &Path) -> String {
    format!(
        "kakaorelay-{}",
        json::hash(&absolute(cwd).to_string_lossy())
    )
}

/// 자식 프로세스 환경만 설정한다. 사용자 데스크톱·CLI 환경은 바꾸지 않는다.
/// Codex는 인자 맨 앞에 `-c sqlite_home=<JSON 경로>`를 붙이므로 다른 인자보다 먼저 호출한다.
pub fn configure(command: &mut CliCommand, provider: &str) -> Result<()> {
    ensure(provider)?;
    let home = home(provider)?;
    for id in ORDER {
        command.set_env(home_variable(id), self::home(id)?.to_string_lossy());
    }
    command.set_env("CODEX_SQLITE_HOME", self::home("codex")?.to_string_lossy());
    if provider == "codex" {
        refresh_codex_login();
        command.arg("-c");
        command.arg(format!(
            "sqlite_home={}",
            json::default(&home.to_string_lossy())
        ));
    }
    if provider == "claude" {
        command.set_env("CLAUDE_CODE_PROJECT_DIR_NAME", claude_project(&command.cwd));
        let tmp = home.join("tmp");
        command.set_env("CLAUDE_CODE_TMPDIR", tmp.to_string_lossy());
        std::fs::create_dir_all(&tmp)?;
    }
    Ok(())
}

/// `account_id`가 같고 원본 `last_refresh`가 더 최신일 때만 덮어쓴다. 실패는 조용히 넘긴다(원본과 같다).
fn refresh_codex_login() {
    let _guard = lock();
    let source = legacy_home("codex").join("auth.json");
    let Ok(destination) = home("codex").map(|h| h.join("auth.json")) else {
        return;
    };
    if !source.is_file()
        || !destination.is_file()
        || absolute(&source)
            .to_string_lossy()
            .eq_ignore_ascii_case(&absolute(&destination).to_string_lossy())
    {
        return;
    }
    let Ok(text) = std::fs::read_to_string(&source) else {
        return;
    };
    let Ok(saved_text) = std::fs::read_to_string(&destination) else {
        return;
    };
    let (Ok(a), Ok(b)) = (
        serde_json::from_str::<serde_json::Value>(&text),
        serde_json::from_str::<serde_json::Value>(&saved_text),
    ) else {
        return;
    };
    let account = |v: &serde_json::Value| {
        v.get("tokens")
            .and_then(|t| t.get("account_id"))
            .and_then(|x| x.as_str())
            .map(str::to_string)
    };
    match (account(&a), account(&b)) {
        (Some(x), Some(y)) if !crate::text::is_blank(&x) && x == y => {}
        _ => return,
    }
    let refreshed = |v: &serde_json::Value| {
        v.get("last_refresh")
            .and_then(|x| x.as_str())
            .and_then(json::time::parse)
    };
    match (refreshed(&a), refreshed(&b)) {
        (Some(newer), Some(older)) if newer > older => {}
        _ => return,
    }
    let temp = crate::settings::path_with_suffix(
        &destination,
        &format!(".sync-{}", uuid::Uuid::new_v4().simple()),
    );
    if std::fs::write(&temp, &text).is_ok() {
        let _ = std::fs::rename(&temp, &destination);
    }
    let _ = std::fs::remove_file(&temp);
}

/// `Uri.EscapeDataString`: RFC 3986 비예약 문자만 그대로 두고 UTF-8 바이트를 `%XX`로 쓴다.
fn escape_data_string(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// `Regex.Replace(path, "[^a-zA-Z0-9]", "-")`. .NET은 UTF-16 코드 단위마다 바꾸므로 BMP 밖 문자는 `--`가 된다.
fn claude_legacy_project(path: &str) -> String {
    path.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_string()
            } else {
                "-".repeat(c.len_utf16())
            }
        })
        .collect()
}

pub fn conversation_folder(provider: &str, identity: &str) -> Result<PathBuf> {
    ensure(provider)?;
    let folder = home(provider)?.join("conversations").join(identity);
    let _guard = lock();
    if folder.join("state.json").exists() {
        return Ok(folder);
    }
    let old_folder = crate::paths::app_data().join("ai-sessions").join(identity);
    if !old_folder.join("state.json").exists() {
        return Ok(folder);
    }
    let state = AiConversation::load(&old_folder, None);
    let mut imported = false;
    if let Some(session) = state
        .session_id
        .as_deref()
        .filter(|s| uuid::Uuid::parse_str(s).is_ok())
    {
        let source = legacy_home(provider);
        let home = home(provider)?;
        if provider == "codex" {
            for category in ["sessions", "archived_sessions"] {
                let logs = source.join(category);
                if !logs.is_dir() {
                    continue;
                }
                for file in walk_files(&logs) {
                    if file
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().ends_with(&format!("{session}.jsonl")))
                    {
                        // 알려진 세션 ID는 이 앱의 방 상태가 소유한다.
                        let relative = file.strip_prefix(&logs).unwrap_or(&file);
                        copy_file(&file, &home.join("sessions").join(relative))?;
                        imported = true;
                    }
                }
            }
        } else if provider == "grok" {
            let old_project = source
                .join("sessions")
                .join(escape_data_string(&old_folder.to_string_lossy()));
            let session_dir = old_project.join(session);
            if session_dir.is_dir() {
                copy_tree(
                    &session_dir,
                    &home
                        .join("sessions")
                        .join(escape_data_string(&folder.to_string_lossy()))
                        .join(session),
                )?;
                imported = true;
            }
        } else {
            let old_project = source
                .join("projects")
                .join(claude_legacy_project(&old_folder.to_string_lossy()));
            let transcript = old_project.join(format!("{session}.jsonl"));
            if transcript.is_file() {
                let destination = home.join("projects").join(claude_project(&folder));
                copy_file(&transcript, &destination.join(format!("{session}.jsonl")))?;
                copy_tree(&old_project.join(session), &destination.join(session))?;
                imported = true;
            }
        }
    }
    // 원래 대화 기록을 옮기지 못했으면 이어진 문맥이라고 주장하지 않는다.
    if imported {
        state.save(&folder)?
    } else {
        AiConversation::default().save(&folder)?
    }
    Ok(folder)
}

fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut files = vec![];
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path)
            } else {
                files.push(path)
            }
        }
    }
    files
}

fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_file() || destination.exists() {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, destination)?;
    #[cfg(unix)]
    set_mode(destination, 0o600)?;
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    for file in walk_files(source) {
        let relative = file.strip_prefix(source).unwrap_or(&file);
        copy_file(&file, &destination.join(relative))?;
    }
    Ok(())
}
