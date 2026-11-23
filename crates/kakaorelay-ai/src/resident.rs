//! 원본: `ResidentCli.cs`. 방·프로바이더·설정 조합마다 상주 CLI 하나와 직렬 턴 하나를 둔다.
//! Codex는 `app-server --stdio` 줄 단위 JSON-RPC(`jsonrpc` 필드 없음), Claude는 stream-json이다(inventory §5-1, §5-2).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::task::JoinHandle;

use crate::attachment::AgentAttachment;
use crate::cancel::CancellationToken;
use crate::cli_runner::CliAiRunner;
use crate::error::{Error, Result};
use crate::json;
use crate::process::{ChildProcess, CliCommand, SESSION_MARKERS};
use crate::provider_storage;
use crate::settings::AiProviderSettings;

/// 응답을 기다리는 동안 알림 줄을 관찰하는 콜백.
type Observer<'a> = &'a mut (dyn FnMut(&Value) -> Result<()> + Send);

const IDLE: Duration = Duration::from_secs(600);
const MAX_LINE: usize = 2_000_000;

struct Entry {
    /// 원본 `Gate`(SemaphoreSlim 1). 세션은 턴 동안 꺼내 쓰고 성공했을 때만 되돌려 놓는다.
    /// 시간 초과·취소로 턴이 버려지면 꺼낸 세션도 함께 버려져(프로세스 종료) 다시 쓰이지 않는다.
    gate: tokio::sync::Mutex<Option<ResidentCliSession>>,
    last_used: Mutex<Instant>,
}

pub(crate) struct ResidentCliPool {
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    shutdown: CancellationToken,
    disposed: AtomicBool,
}

impl ResidentCliPool {
    pub(crate) fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            shutdown: CancellationToken::new(),
            disposed: AtomicBool::new(false),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run(
        &self,
        provider: &AiProviderSettings,
        prompt: &str,
        seconds: i32,
        cancel: &CancellationToken,
        folder: &Path,
        session_id: Option<&str>,
        attachments: &[AgentAttachment],
    ) -> Result<(String, String)> {
        let file_tools = provider.id == "codex" && attachments.iter().any(AgentAttachment::is_pdf);
        let key = format!(
            "{}|{}|{}",
            provider_storage::absolute(folder).display(),
            json::default_pascal(provider),
            if file_tools { "True" } else { "False" }
        );
        let entry = {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if self.disposed.load(Ordering::SeqCst) {
                return Err(Error::Cancelled);
            }
            for existing in entries.values() {
                let idle = existing
                    .last_used
                    .lock()
                    .map(|t| t.elapsed() > IDLE)
                    .unwrap_or(false);
                if idle && let Ok(mut gate) = existing.gate.try_lock() {
                    gate.take();
                }
            }
            entries
                .entry(key)
                .or_insert_with(|| {
                    Arc::new(Entry {
                        gate: tokio::sync::Mutex::new(None),
                        last_used: Mutex::new(Instant::now()),
                    })
                })
                .clone()
        };
        let work = async {
            let mut gate = entry.gate.lock().await;
            let mut session = gate
                .take()
                .filter(|c| c.session_id.as_deref() == session_id);
            if session.as_mut().is_some_and(|c| !c.alive()) {
                session = None;
            }
            if session.is_none() {
                std::fs::create_dir_all(folder)?;
                let mut started =
                    ResidentCliSession::new(provider, folder, session_id, file_tools)?;
                if self.disposed.load(Ordering::SeqCst) {
                    return Err(Error::Cancelled);
                }
                started.initialize().await?;
                session = Some(started);
            }
            let mut session = session.expect("세션");
            // 실패한 턴은 이미 받아들여졌을 수 있으므로 같은 세션으로 다시 보내지 않는다(세션을 버린다).
            let result = session.run(prompt, attachments).await?;
            *gate = Some(session);
            Ok(result)
        };
        let deadline = tokio::time::sleep(crate::cancel::seconds(seconds));
        let result = tokio::select! {
            r = work => r,
            _ = cancel.cancelled() => Err(Error::Cancelled),
            _ = self.shutdown.cancelled() => Err(Error::Cancelled),
            _ = deadline => Err(Error::Cancelled),
        };
        *entry.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        result
    }

    pub(crate) fn dispose(&self) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if self.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shutdown.cancel();
        for entry in entries.values() {
            if let Ok(mut gate) = entry.gate.try_lock() {
                gate.take();
            }
        }
        entries.clear();
    }
}

impl Drop for ResidentCliPool {
    fn drop(&mut self) {
        self.dispose();
    }
}

pub(crate) struct ResidentCliSession {
    provider: AiProviderSettings,
    folder: PathBuf,
    pipe: CliJsonPipe,
    resume_id: Option<String>,
    file_tools: bool,
    pub(crate) session_id: Option<String>,
}

impl ResidentCliSession {
    fn new(
        provider: &AiProviderSettings,
        folder: &Path,
        session_id: Option<&str>,
        file_tools: bool,
    ) -> Result<Self> {
        let folder = provider_storage::absolute(folder);
        let current = session_id
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let exe = CliAiRunner::resolve(provider)
            .ok_or_else(|| Error::invalid_operation("CLI 실행 파일 없음"))?;
        let args = CliAiRunner::resident_arguments(provider, session_id, &current, file_tools)?;
        let pipe = CliJsonPipe::start(&exe, &folder, &args, &provider.id)?;
        Ok(Self {
            provider: provider.clone(),
            folder,
            pipe,
            resume_id: session_id.map(str::to_string),
            file_tools,
            session_id: Some(current),
        })
    }

    fn alive(&mut self) -> bool {
        self.pipe.alive()
    }

    async fn initialize(&mut self) -> Result<()> {
        if self.provider.id == "claude" {
            return Ok(()); // 첫 user 메시지가 스트림을 시작한다.
        }
        self.pipe
            .call(
                "initialize",
                json!({"clientInfo": {"name": "kakaorelay", "version": "1.0.0"}}),
                None,
            )
            .await?;
        self.pipe
            .send(&json!({"method": "initialized", "params": {}}))
            .await?;
        let configuration = self
            .pipe
            .call("config/read", json!({"includeLayers": false}), None)
            .await?;
        let mut overrides = Map::new();
        overrides.insert("features.shell_tool".into(), json!(self.file_tools));
        overrides.insert("web_search".into(), json!("live"));
        overrides.insert(
            "sandbox_mode".into(),
            json!(if self.file_tools {
                "workspace-write"
            } else {
                "read-only"
            }),
        );
        if self.file_tools && cfg!(windows) {
            overrides.insert("windows.sandbox".into(), json!("unelevated"));
        }
        if self.provider.effort != "default" {
            overrides.insert("model_reasoning_effort".into(), json!(self.provider.effort));
        }
        // 사용자 설정을 고치지 않고 이 챗봇 스레드에서만 물려받은 통합을 끈다.
        if let Some(config) = configuration.get("config") {
            for section in ["mcp_servers", "plugins"] {
                if let Some(Value::Object(values)) = config.get(section) {
                    for name in values.keys() {
                        if name.contains('.') {
                            return Err(Error::invalid_operation(
                                "Codex 통합 이름에 점이 있어 도구 비활성화 설정을 적용할 수 없습니다.",
                            ));
                        }
                        overrides.insert(format!("{section}.{name}.enabled"), json!(false));
                    }
                }
            }
        }
        let mut parameters = Map::new();
        parameters.insert(
            "model".into(),
            if crate::text::is_blank(&self.provider.model) {
                Value::Null
            } else {
                json!(self.provider.model)
            },
        );
        parameters.insert("cwd".into(), json!(self.folder.to_string_lossy()));
        parameters.insert("approvalPolicy".into(), json!("never"));
        parameters.insert(
            "sandbox".into(),
            json!(if self.file_tools {
                "workspace-write"
            } else {
                "read-only"
            }),
        );
        parameters.insert("config".into(), Value::Object(overrides));
        if let Some(resume) = &self.resume_id {
            parameters.insert("threadId".into(), json!(resume));
        }
        let method = if self.resume_id.is_none() {
            "thread/start"
        } else {
            "thread/resume"
        };
        let response = self
            .pipe
            .call(method, Value::Object(parameters), None)
            .await?;
        let actual = response
            .get("thread")
            .and_then(|t| t.get("id"))
            .ok_or_else(|| Error::KeyNotFound("thread.id".into()))?
            .as_str()
            .map(str::to_string);
        match actual {
            Some(id)
                if !crate::text::is_blank(&id)
                    && self.resume_id.as_deref().is_none_or(|r| r == id) =>
            {
                self.session_id = Some(id);
                Ok(())
            }
            _ => Err(Error::invalid_operation(
                "Codex 세션 ID가 일치하지 않습니다.",
            )),
        }
    }

    async fn run(
        &mut self,
        prompt: &str,
        attachments: &[AgentAttachment],
    ) -> Result<(String, String)> {
        let session = self.session_id.clone().unwrap_or_default();
        if self.provider.id == "claude" {
            let content = if attachments.is_empty() {
                json!(prompt)
            } else {
                Value::Array(AgentAttachment::claude_input(prompt, attachments).await?)
            };
            self.pipe
                .send(&json!({"type": "user", "session_id": session, "message": {"role": "user", "content": content}, "parent_tool_use_id": null}))
                .await?;
            loop {
                let evt = self.pipe.read().await?;
                let kind = evt.get("type").and_then(Value::as_str);
                if kind == Some("result") {
                    if evt.get("session_id").and_then(Value::as_str) != Some(session.as_str()) {
                        return Err(Error::invalid_operation(
                            "Claude 세션 ID가 일치하지 않습니다.",
                        ));
                    }
                    if evt.get("is_error") == Some(&Value::Bool(true)) {
                        return Err(Error::invalid_operation(
                            "Claude 응답 실패 · 로그인·모델·사용량을 확인하세요.",
                        ));
                    }
                    if let Some(subtype) = evt.get("subtype")
                        && subtype.as_str() != Some("success")
                    {
                        return Err(Error::invalid_operation(
                            "Claude 응답 실패 · 로그인·모델·사용량을 확인하세요.",
                        ));
                    }
                    return Ok((CliAiRunner::parse_json_output(&evt.to_string())?, session));
                }
                if kind == Some("control_request") {
                    let request_id = evt
                        .get("request_id")
                        .ok_or_else(|| Error::KeyNotFound("request_id".into()))?
                        .as_str()
                        .map(str::to_string);
                    self.pipe
                        .send(&json!({"type": "control_response", "response": {"subtype": "error", "request_id": request_id, "error": "Only pre-approved WebSearch and WebFetch tools are available."}}))
                        .await?;
                }
            }
        }
        let mut state = CodexTurn {
            session: session.clone(),
            text: String::new(),
            complete: false,
            failure: None,
        };
        let input = AgentAttachment::codex_input(prompt, attachments);
        let model = if crate::text::is_blank(&self.provider.model) {
            Value::Null
        } else {
            json!(self.provider.model)
        };
        let effort = if self.provider.effort == "default" {
            Value::Null
        } else {
            json!(self.provider.effort)
        };
        self.pipe
            .call(
                "turn/start",
                json!({"threadId": session, "model": model, "effort": effort, "input": input}),
                Some(&mut |evt| state.observe(evt)),
            )
            .await?;
        while !state.complete {
            let evt = self.pipe.read().await?;
            state.observe(&evt)?;
        }
        if let Some(failure) = state.failure {
            return Err(Error::invalid_operation(failure));
        }
        if crate::text::is_blank(&state.text) {
            return Err(Error::invalid_operation("Codex 최종 응답 없음"));
        }
        Ok((state.text, session))
    }
}

struct CodexTurn {
    session: String,
    text: String,
    complete: bool,
    failure: Option<String>,
}

impl CodexTurn {
    fn observe(&mut self, evt: &Value) -> Result<()> {
        let (Some(method), Some(p)) = (evt.get("method"), evt.get("params")) else {
            return Ok(());
        };
        if let Some(thread) = p.get("threadId")
            && thread.as_str() != Some(self.session.as_str())
        {
            return Ok(());
        }
        let missing = |name: &str| Error::KeyNotFound(name.to_string());
        if method.as_str() == Some("item/completed") {
            let item = p.get("item").ok_or_else(|| missing("item"))?;
            if item.get("type").ok_or_else(|| missing("type"))?.as_str() == Some("agentMessage") {
                // 진행 설명(commentary)은 최종 답변이 아니다.
                let phase = item.get("phase");
                if phase.is_none_or(|p| p.is_null() || p.as_str() == Some("final_answer")) {
                    self.text.push_str(
                        item.get("text")
                            .ok_or_else(|| missing("text"))?
                            .as_str()
                            .unwrap_or_default(),
                    );
                    if crate::text::utf16_len(&self.text) > MAX_LINE {
                        return Err(Error::io("CLI 응답이 너무 큽니다."));
                    }
                }
            }
        }
        if method.as_str() == Some("turn/completed") {
            self.complete = true;
            let status = p
                .get("turn")
                .and_then(|t| t.get("status"))
                .ok_or_else(|| missing("status"))?;
            if status.as_str() != Some("completed") {
                self.failure = Some("Codex 응답 실패 · 로그인·모델·사용량을 확인하세요.".into());
            }
        }
        Ok(())
    }
}

pub(crate) struct CliJsonPipe {
    process: ChildProcess,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    errors: Option<JoinHandle<()>>,
    sequence: i64,
}

impl CliJsonPipe {
    pub(crate) fn start(
        exe: &Path,
        cwd: &Path,
        arguments: &[String],
        provider: &str,
    ) -> Result<Self> {
        let mut command = CliCommand::new(exe, cwd);
        provider_storage::configure(&mut command, provider)?;
        command.args.extend(arguments.iter().cloned());
        for key in SESSION_MARKERS {
            command.remove(key);
        }
        let mut process = command.spawn()?;
        let stdin = process
            .take_stdin()
            .ok_or_else(|| Error::invalid_operation("CLI 시작 실패"))?;
        let stdout = BufReader::new(
            process
                .take_stdout()
                .ok_or_else(|| Error::invalid_operation("CLI 시작 실패"))?,
        );
        let errors = process.take_stderr().map(|stderr| {
            tokio::spawn(async move {
                crate::process::drain(stderr).await;
            })
        });
        Ok(Self {
            process,
            stdin,
            stdout,
            errors,
            sequence: 0,
        })
    }

    pub(crate) fn alive(&mut self) -> bool {
        !self.process.has_exited()
    }

    pub(crate) async fn send(&mut self, value: &Value) -> Result<()> {
        let mut line = json::to_string(value, json::Escape::Prompt, false);
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    pub(crate) async fn read(&mut self) -> Result<Value> {
        loop {
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .await
                .map_err(|e| Error::io(e.to_string()))?;
            if read == 0 {
                return Err(Error::io(
                    "CLI 연결이 종료되었습니다. 로그인·모델·CLI 버전을 확인하세요.",
                ));
            }
            let line = line.trim_end_matches(['\n', '\r']);
            if crate::text::utf16_len(line) > MAX_LINE {
                return Err(Error::io("CLI 이벤트가 너무 큽니다."));
            }
            if crate::text::is_blank(line) {
                continue;
            }
            let evt: Value =
                serde_json::from_str(line).map_err(|_| Error::io("CLI 응답 형식을 확인하세요."))?;
            if let (Some(id), Some(_)) = (evt.get("id"), evt.get("method")) {
                let reply = json!({"id": id, "error": {"code": -32601, "message": "Tools and approvals are disabled for chatbot replies."}});
                self.send(&reply).await?;
                continue;
            }
            return Ok(evt);
        }
    }

    pub(crate) async fn call(
        &mut self,
        method: &str,
        parameters: Value,
        mut observe: Option<Observer<'_>>,
    ) -> Result<Value> {
        self.sequence += 1;
        let id = self.sequence;
        self.send(&json!({"id": id, "method": method, "params": parameters}))
            .await?;
        loop {
            let evt = self.read().await?;
            if evt.get("id").and_then(Value::as_i64) == Some(id)
                && evt.get("id").is_some_and(Value::is_number)
            {
                if evt.get("error").is_some() {
                    return Err(Error::invalid_operation(format!(
                        "CLI {method} 실패 · 로그인·모델·세션을 확인하세요."
                    )));
                }
                return evt
                    .get("result")
                    .cloned()
                    .ok_or_else(|| Error::KeyNotFound("result".into()));
            }
            if let Some(observe) = observe.as_mut() {
                observe(&evt)?;
            }
        }
    }
}

impl Drop for CliJsonPipe {
    fn drop(&mut self) {
        self.process.kill_tree();
        if let Some(errors) = self.errors.take() {
            errors.abort();
        }
    }
}
