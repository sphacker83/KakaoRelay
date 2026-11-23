//! 원본: `ModelCatalog.cs`. CLI가 알려주는 모델 목록 조회(codex·claude·grok).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliModel {
    pub id: String,
    pub label: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliModelCatalog {
    pub models: Vec<CliModel>,
    pub checked_at: DateTime<Utc>,
    pub login_required: bool,
}

/// `\A[a-zA-Z0-9._:/\[\]-]{1,150}\z`.
pub fn valid_id(id: &str) -> bool {
    (1..=150).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/[]-".contains(&b))
}

/// 빈 값(CLI 기본값), 목록, 현재 선택 순으로 중복 없이 나열한다. 목록에 없는 저장 값도 유지한다.
pub fn choices(models: &[CliModel], current: &str) -> Vec<String> {
    let mut result: Vec<String> = vec![String::new()];
    let current = (!current.is_empty()).then(|| current.to_string());
    for id in models.iter().map(|m| m.id.clone()).chain(current) {
        if !result.contains(&id) {
            result.push(id);
        }
    }
    result
}

fn result(models: Vec<CliModel>, login_required: bool) -> Result<CliModelCatalog> {
    let mut unique: Vec<CliModel> = vec![];
    for model in models {
        if !unique.iter().any(|m| m.id == model.id) {
            unique.push(model);
        }
    }
    if unique.is_empty() {
        return Err(Error::io("CLI가 제공한 모델 목록이 비어 있습니다."));
    }
    Ok(CliModelCatalog {
        models: unique,
        checked_at: Utc::now(),
        login_required,
    })
}

pub fn parse_grok(output: &str) -> Result<CliModelCatalog> {
    let ansi = regex::Regex::new("\u{1b}\\[[0-9;]*m").expect("정규식");
    let text = ansi.replace_all(output, "").to_string();
    let lower = text.to_ascii_lowercase();
    let marker = "Available models:";
    let Some(index) = lower.find(&marker.to_ascii_lowercase()) else {
        return Err(Error::io("Grok CLI 모델 목록 응답을 확인하지 못했습니다."));
    };
    let line = regex::Regex::new(
        r"(?m)^\s*[*-]\s+([a-zA-Z0-9._:/\[\]-]+)(?:[ \t]+\((default)\))?[ \t]*\r?$",
    )
    .expect("정규식");
    let models = line
        .captures_iter(&text[index + marker.len()..])
        .map(|c| CliModel {
            id: c[1].to_string(),
            label: c[1].to_string(),
            is_default: c.get(2).is_some(),
        })
        .filter(|m| valid_id(&m.id))
        .collect();
    result(models, lower.contains("not authenticated"))
}

/// CLI에 모델 목록을 묻는다. 전체 제한은 30초다.
pub async fn query(
    provider: &crate::settings::AiProviderSettings,
    cancel: &crate::cancel::CancellationToken,
) -> Result<CliModelCatalog> {
    use crate::cli_runner::{CliAiRunner, FolderCleanup};
    if !crate::settings::ORDER.contains(&provider.id.as_str()) {
        return Err(Error::argument("지원하지 않는 AI 서비스입니다."));
    }
    let exe = CliAiRunner::resolve(provider)
        .ok_or_else(|| Error::invalid_operation("설치된 CLI 실행 파일을 찾지 못했습니다."))?;
    let folder = crate::provider_storage::job_folder(&provider.id, "model-catalog")?;
    let _cleanup = FolderCleanup(folder.clone());
    let timeout = std::time::Duration::from_secs(30);
    let work = async {
        if provider.id == "grok" {
            return parse_grok(&grok_models(&exe, &folder).await?);
        }
        query_pipe(provider, &exe, &folder).await
    };
    match crate::cancel::run(
        work,
        &crate::cancel::CancellationToken::new(),
        Some(timeout),
    )
    .await
    {
        Err(Error::Cancelled) => Err(Error::Timeout(
            "CLI 모델 조회 시간이 초과되었습니다. 로그인 및 설치 상태를 확인하세요.".into(),
        )),
        other => other,
    }
    .and_then(|r| {
        if cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(r)
        }
    })
}

async fn query_pipe(
    provider: &crate::settings::AiProviderSettings,
    exe: &std::path::Path,
    folder: &std::path::Path,
) -> Result<CliModelCatalog> {
    use serde_json::{Value, json};
    let text = |v: &Value, name: &str| v.get(name).and_then(Value::as_str).map(str::to_string);
    let args: Vec<String> = if provider.id == "codex" {
        vec!["app-server".into(), "--stdio".into()]
    } else {
        [
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            "--no-session-persistence",
            "--disable-slash-commands",
            "--setting-sources",
            "",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    };
    let mut pipe = crate::resident::CliJsonPipe::start(exe, folder, &args, &provider.id)?;
    let mut models = vec![];
    if provider.id == "codex" {
        pipe.call(
            "initialize",
            json!({"clientInfo": {"name": "kakaorelay-models", "version": "1.0.0"}}),
            None,
        )
        .await?;
        pipe.send(&json!({"method": "initialized", "params": {}}))
            .await?;
        let mut cursor: Option<String> = None;
        let mut cursors = std::collections::HashSet::new();
        for page in 0.. {
            if page >= 20 || cursor.as_ref().is_some_and(|c| !cursors.insert(c.clone())) {
                return Err(Error::io("CLI 모델 목록 페이지 오류"));
            }
            let result = pipe
                .call(
                    "model/list",
                    json!({"limit": 100, "includeHidden": false, "cursor": cursor}),
                    None,
                )
                .await?;
            let data = result
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::KeyNotFound("data".into()))?;
            for m in data {
                if m.get("hidden") == Some(&Value::Bool(true)) {
                    continue;
                }
                let Some(id) = text(m, "model")
                    .or_else(|| text(m, "id"))
                    .filter(|id| valid_id(id))
                else {
                    continue;
                };
                models.push(CliModel {
                    label: text(m, "displayName").unwrap_or_else(|| id.clone()),
                    is_default: m.get("isDefault") == Some(&Value::Bool(true)),
                    id,
                });
            }
            cursor = text(&result, "nextCursor");
            if cursor.as_deref().is_none_or(str::is_empty) {
                break;
            }
        }
    } else {
        pipe.send(&json!({"type": "control_request", "request_id": "models", "request": {"subtype": "initialize"}})).await?;
        loop {
            let result = pipe.read().await?;
            let Some(response) = result.get("response") else {
                continue;
            };
            if text(&result, "type").as_deref() != Some("control_response")
                || text(response, "request_id").as_deref() != Some("models")
            {
                continue;
            }
            if text(response, "subtype").as_deref() == Some("error") {
                return Err(Error::io("Claude CLI 모델 조회를 확인하지 못했습니다."));
            }
            let list = response
                .get("response")
                .and_then(|r| r.get("models"))
                .and_then(Value::as_array)
                .ok_or_else(|| Error::KeyNotFound("models".into()))?;
            for m in list {
                if let Some(id) = text(m, "value").filter(|id| valid_id(id)) {
                    models.push(CliModel {
                        label: text(m, "displayName").unwrap_or_else(|| id.clone()),
                        is_default: id == "default",
                        id,
                    });
                }
            }
            break;
        }
    }
    result(models, false)
}

async fn grok_models(exe: &std::path::Path, folder: &std::path::Path) -> Result<String> {
    let mut command = crate::process::CliCommand::new(exe, folder);
    crate::provider_storage::configure(&mut command, "grok")?;
    command.arg("models");
    let mut process = command
        .spawn()
        .map_err(|_| Error::io("Grok CLI 시작 실패"))?;
    drop(process.take_stdin());
    let stderr = process
        .take_stderr()
        .map(|e| tokio::spawn(crate::process::drain(e)));
    let mut stdout = process
        .take_stdout()
        .ok_or_else(|| Error::io("Grok CLI 시작 실패"))?;
    let mut bytes = vec![];
    let mut buffer = [0u8; 4096];
    loop {
        use tokio::io::AsyncReadExt;
        let n = stdout.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..n]);
        if bytes.len() > 2_000_000 * 3
            && crate::text::utf16_len(&String::from_utf8_lossy(&bytes)) > 2_000_000
        {
            return Err(Error::io("모델 목록 응답 크기 초과"));
        }
    }
    let code = process.wait().await?;
    if let Some(stderr) = stderr {
        let _ = stderr.await;
    }
    if code != 0 {
        return Err(Error::io(
            "Grok CLI 모델 조회 실패 · 로그인 및 설치 상태를 확인하세요.",
        ));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
