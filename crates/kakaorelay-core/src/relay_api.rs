//! 원본: `RelayApi.cs`의 `RelayApi`, `ApiConnection`. 127.0.0.1 임의 포트의 로컬 HTTP 게이트웨이(inventory §3).
//!
//! - 모든 응답에 `Cache-Control: no-store`. `Origin`·`Sec-Fetch-Site`가 있거나 Bearer 토큰이 정확히 같지 않으면
//!   경로와 무관하게 먼저 401이다. 비교는 상수 시간이다.
//! - 오류: `ApiFailure` → `{code, detail}`, 형식 오류 → 400 `{"code":"invalid_request"}`, 그 밖 → 500 고정 본문.
//! - JSON은 camelCase이고 한글을 이스케이프하지 않는다(`PromptJson.Encoder`).
//! - 경로 비교는 ASP.NET 라우팅처럼 대소문자를 무시하고 끝 `/` 하나를 허용한다.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use kakaorelay_ai::ai_service::{AiCommand, AiService};
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::cli_runner::CliAiRunner;
use kakaorelay_ai::json::{self, Escape};
use kakaorelay_ai::persona_images::PersonaImageCatalog;
use kakaorelay_ai::settings::AiSettings;
use kakaorelay_ai::{ApiFailure, Error, LocalRoom, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::task::JoinHandle;

use crate::api_send::{ApiImageCommand, ApiSendCommand, Scan};
use crate::delegations::DelegationJobStore;
use crate::fs::write_atomic;
use crate::ids::valid_request_id;
use crate::platform::ConversationTarget;
use crate::support::BoxFuture;
use crate::test_send::TestSendReceipt;

pub const VERSION: &str = "0.10.0";
/// 원본 Windows 문구. 맥 어댑터는 `Platform::recent_messages_reason`으로 바꿀 수 있다.
pub const DEFAULT_RECENT_MESSAGES_REASON: &str =
    "SQLCipher 4 · 키가 메모리에 로드된 방 · DB/WAL 읽기 전용";
const UNKNOWN_RESULT: &str = "같은 requestId로 결과를 조회하세요. 새 ID로 자동 재전송하지 마세요.";

pub type HistoryFn = Arc<dyn Fn() -> Result<Vec<TestSendReceipt>> + Send + Sync>;
pub type SendHandler =
    Arc<dyn Fn(ApiSendCommand) -> BoxFuture<'static, Result<TestSendReceipt>> + Send + Sync>;
pub type RuntimeFn = Arc<dyn Fn() -> BoxFuture<'static, Value> + Send + Sync>;

pub struct RelayApiConfig {
    pub discovery_path: PathBuf,
    pub rooms: Scan,
    pub history: HistoryFn,
    pub send: SendHandler,
    pub ai: Option<Arc<AiService>>,
    pub runtime: Option<RuntimeFn>,
    /// `capabilities.imageSend.transport`(`Platform::image_transport_name`).
    pub image_transport: String,
    pub recent_messages_reason: String,
    /// 위임 작업 기록 폴더. 없으면 기본 위치다.
    pub delegation_root: Option<PathBuf>,
}

impl RelayApiConfig {
    pub fn new(
        discovery_path: impl Into<PathBuf>,
        rooms: Scan,
        history: HistoryFn,
        send: SendHandler,
    ) -> Self {
        Self {
            discovery_path: discovery_path.into(),
            rooms,
            history,
            send,
            ai: None,
            runtime: None,
            image_transport: "WM_DROPFILES + HWND".into(),
            recent_messages_reason: DEFAULT_RECENT_MESSAGES_REASON.into(),
            delegation_root: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiConnection {
    pub base_url: String,
    pub token: String,
    pub process_id: i32,
    pub version: String,
}

pub fn default_connection_path() -> PathBuf {
    kakaorelay_ai::paths::app_data().join("connection.json")
}

struct ApiState {
    expected_auth: Vec<u8>,
    config: RelayApiConfig,
}

pub struct RelayApi {
    connection: ApiConnection,
    discovery_path: PathBuf,
    shutdown: CancellationToken,
    server: Mutex<Option<JoinHandle<()>>>,
}

impl RelayApi {
    /// 서버를 띄우고 연결 파일(`connection.json`)을 원자적으로 쓴다.
    pub async fn start(config: RelayApiConfig) -> Result<Self> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| Error::invalid_operation(e.to_string()))?;
        let token = json::upper_hex(&bytes);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let discovery_path = config.discovery_path.clone();
        let state = Arc::new(ApiState {
            expected_auth: format!("Bearer {token}").into_bytes(),
            config,
        });
        let router = axum::Router::new().fallback(handle).with_state(state);
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await;
        });
        let connection = ApiConnection {
            base_url: format!("http://{address}"),
            token,
            process_id: i32::try_from(std::process::id()).unwrap_or(0),
            version: VERSION.into(),
        };
        let api = Self {
            connection,
            discovery_path,
            shutdown,
            server: Mutex::new(Some(server)),
        };
        let written = (|| -> Result<()> {
            if let Some(parent) = api.discovery_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_atomic(
                &api.discovery_path,
                json::indented(&api.connection).as_bytes(),
                false,
            )
        })();
        if let Err(error) = written {
            api.stop().await;
            return Err(error);
        }
        Ok(api)
    }

    pub fn connection(&self) -> &ApiConnection {
        &self.connection
    }

    /// 원본 `DisposeAsync`. 연결 파일의 토큰이 이 서버의 것일 때만 지우고, 서버 정지는 2초까지 기다린다.
    pub async fn stop(&self) {
        let Some(server) = self.server.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        remove_discovery(&self.discovery_path, &self.connection.token);
        self.shutdown.cancel();
        let abort = server.abort_handle();
        if tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .is_err()
        {
            abort.abort();
        }
    }
}

impl Drop for RelayApi {
    fn drop(&mut self) {
        if self
            .server
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .is_some()
        {
            remove_discovery(&self.discovery_path, &self.connection.token);
            self.shutdown.cancel();
        }
    }
}

fn remove_discovery(path: &Path, token: &str) {
    let owned = kakaorelay_ai::text::read_text(path)
        .ok()
        .and_then(|t| {
            serde_json::from_str::<Option<ApiConnection>>(&t)
                .ok()
                .flatten()
        })
        .is_some_and(|c| c.token == token);
    if owned {
        let _ = std::fs::remove_file(path);
    }
}

/// 상수 시간 비교. 길이가 다르면 바로 거부한다(원본도 길이를 먼저 본다).
fn same_secret(supplied: &[u8], expected: &[u8]) -> bool {
    supplied.len() == expected.len()
        && supplied
            .iter()
            .zip(expected)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn respond(status: StatusCode, body: Option<String>) -> Response {
    let mut builder = Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if body.is_some() {
        builder = builder.header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
    }
    builder
        .body(body.map_or_else(Body::empty, Body::from))
        .unwrap_or_default()
}

fn json_response(status: StatusCode, value: &Value) -> Response {
    respond(status, Some(json::to_string(value, Escape::Prompt, false)))
}

fn error_response(error: Error) -> Response {
    match error {
        Error::Api(failure) => json_response(
            StatusCode::from_u16(failure.status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            &json!({"code": failure.code, "detail": failure.detail}),
        ),
        Error::Json(_) | Error::Argument(_) => {
            json_response(StatusCode::BAD_REQUEST, &json!({"code": "invalid_request"}))
        }
        _ => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({"code": "unknown_result", "detail": UNKNOWN_RESULT}),
        ),
    }
}

async fn handle(State(state): State<Arc<ApiState>>, request: Request) -> Response {
    let headers = request.headers();
    let supplied = headers
        .get_all(header::AUTHORIZATION)
        .iter()
        .map(|v| v.as_bytes())
        .collect::<Vec<_>>()
        .join(&b","[..]);
    if headers.contains_key(header::ORIGIN)
        || headers.contains_key("sec-fetch-site")
        || !same_secret(&supplied, &state.expected_auth)
    {
        return json_response(StatusCode::UNAUTHORIZED, &json!({"code": "unauthorized"}));
    }
    match route(&state, request).await {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

/// 경로 조각을 퍼센트 디코딩한다. 잘못된 인코딩은 원문을 유지한다.
fn percent_decode(text: &str, plus_as_space: bool) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(high), Some(low)) => {
                    out.push((high * 16 + low) as u8);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' if plus_as_space => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query(request: &Request) -> Vec<(String, String)> {
    request
        .uri()
        .query()
        .unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(k, true), percent_decode(v, true))
        })
        .collect()
}

fn query_value<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

async fn body_json(request: Request) -> Result<Value> {
    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|e| Error::argument(e.to_string()))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn not_found() -> Response {
    respond(StatusCode::NOT_FOUND, None)
}

fn method_not_allowed(allowed: &str) -> Response {
    let mut response = respond(StatusCode::METHOD_NOT_ALLOWED, None);
    if let Ok(value) = HeaderValue::from_str(allowed) {
        response.headers_mut().insert(header::ALLOW, value);
    }
    response
}

fn ai(state: &ApiState) -> Result<&Arc<AiService>> {
    state
        .config
        .ai
        .as_ref()
        .ok_or_else(|| Error::invalid_operation("AI 서비스가 없습니다."))
}

/// C# `LocalRoom`은 계산 속성 `display`도 직렬화한다(inventory §3-6 #4).
pub fn room_json(room: &LocalRoom) -> Value {
    let mut value = json::value(room);
    if let Value::Object(map) = &mut value {
        map.insert("display".into(), Value::String(room.display()));
    }
    value
}

fn with_room_display(mut value: Value, room: &LocalRoom) -> Value {
    if let Value::Object(map) = &mut value {
        map.insert("room".into(), room_json(room));
    }
    value
}

/// C# `ConversationTarget` 레코드는 계산 속성 `hint`, `selectable`도 직렬화한다.
pub fn target_json(target: &ConversationTarget) -> Value {
    json!({
        "title": target.title,
        "processId": target.process_id,
        "handle": target.handle,
        "ambiguous": target.ambiguous,
        "hint": target.hint(),
        "selectable": target.selectable(),
    })
}

fn capabilities(state: &ApiState) -> Value {
    let has_ai = state.config.ai.is_some();
    let mut endpoints = vec![
        "GET /v1/rooms",
        "POST /v1/send",
        "POST /v1/send-image",
        "GET /v1/requests",
        "GET /v1/requests/{requestId}",
    ];
    if has_ai {
        endpoints.extend([
            "GET /v1/local/rooms",
            "GET /v1/local/messages",
            "GET /v1/ai/settings",
            "PUT /v1/ai/settings",
            "GET /v1/ai/providers",
            "POST /v1/ai/generate",
            "GET /v1/personas/{personaId}/images",
        ]);
    }
    json!({
        "send": true,
        "idempotency": "requestId",
        "requiresOpenRoom": true,
        "maxMessageCharacters": null,
        "textSend": {"defaultSendDelayMs": 1000, "maxSendDelayMs": 30000, "verifyInsertedText": false, "continueAfterInputTimeout": true},
        "imageSend": {"supported": true, "endpoint": "/v1/send-image", "transport": state.config.image_transport, "computerUse": false},
        "recentMessages": {"supported": has_ai, "reason": state.config.recent_messages_reason},
        "aiChat": has_ai,
        "deliveryVerification": "manual",
        "endpoints": endpoints,
    })
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| Error::invalid_operation(e.to_string()))?
}

fn receipt_json(receipt: &TestSendReceipt) -> Value {
    json::value(receipt)
}

async fn route(state: &Arc<ApiState>, request: Request) -> Result<Response> {
    let path = request.uri().path().to_string();
    let trimmed = path.strip_prefix('/').unwrap_or(&path);
    let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
    let segments: Vec<String> = trimmed
        .split('/')
        .map(|s| percent_decode(s, false))
        .collect();
    let lower: Vec<String> = segments.iter().map(|s| s.to_lowercase()).collect();
    let lower: Vec<&str> = lower.iter().map(String::as_str).collect();
    let method = request.method().clone();
    let has_ai = state.config.ai.is_some();
    let get = method == Method::GET;
    let post = method == Method::POST;
    let ok = StatusCode::OK;
    match lower.as_slice() {
        ["v1", "health"] if get => Ok(json_response(
            ok,
            &json!({"status": "ready", "version": VERSION, "application": "KakaoRelay"}),
        )),
        ["v1", "ai", "runtime"] if state.config.runtime.is_some() => {
            if !get {
                return Ok(method_not_allowed("GET"));
            }
            let runtime = state.config.runtime.clone().expect("위에서 확인함");
            Ok(json_response(ok, &runtime().await))
        }
        ["v1", "capabilities"] if get => Ok(json_response(ok, &capabilities(state))),
        ["v1", "rooms"] if get => {
            let rooms = state.config.rooms.clone();
            let targets = blocking(move || Ok(rooms()?)).await?;
            Ok(json_response(
                ok,
                &Value::Array(targets.iter().map(target_json).collect()),
            ))
        }
        ["v1", "send-image"] if post => {
            let command = ApiImageCommand::from_json(body_json(request).await?)?.to_send()?;
            let receipt = (state.config.send)(command).await?;
            Ok(json_response(ok, &receipt_json(&receipt)))
        }
        ["v1", "personas", _, "images"] if has_ai => {
            if !get {
                return Ok(method_not_allowed("GET"));
            }
            let persona_id = segments[2].clone();
            let ai = ai(state)?;
            if !valid_request_id(&persona_id)
                || !ai
                    .settings()
                    .load()?
                    .personas
                    .iter()
                    .any(|p| p.id == persona_id)
            {
                return Err(Error::Api(ApiFailure::new(
                    404,
                    "persona_unavailable",
                    "페르소나가 없습니다.",
                )));
            }
            let catalog = PersonaImageCatalog::new(AiSettings::image_root(&persona_id)?);
            let mut versions = Map::new();
            for provider in PersonaImageCatalog::PROVIDERS {
                versions.insert(
                    provider.into(),
                    json::value(&catalog.store(provider)?.load()?),
                );
            }
            Ok(json_response(
                ok,
                &json!({"personaId": persona_id, "appliedProvider": catalog.applied_provider()?, "versions": versions}),
            ))
        }
        ["v1", "local", "rooms"] if has_ai => {
            if !get {
                return Ok(method_not_allowed("GET"));
            }
            let rooms = ai(state)?.chats().rooms().await?;
            Ok(json_response(
                ok,
                &Value::Array(rooms.iter().map(room_json).collect()),
            ))
        }
        ["v1", "local", "messages"] if has_ai => {
            if !get {
                return Ok(method_not_allowed("GET"));
            }
            let pairs = query(&request);
            let (Some(profile), Some(room_id)) = (
                query_value(&pairs, "profile"),
                query_value(&pairs, "roomId"),
            ) else {
                return Err(Error::argument("profile과 roomId가 필요합니다."));
            };
            let limit = match query_value(&pairs, "limit") {
                None | Some("") => 100,
                Some(text) => text
                    .trim()
                    .parse::<i64>()
                    .map_err(|e| Error::argument(e.to_string()))?,
            };
            let context = ai(state)?
                .chats()
                .read(profile, room_id, usize::try_from(limit).unwrap_or(0))
                .await?;
            Ok(json_response(
                ok,
                &with_room_display(json::value(&context), &context.room),
            ))
        }
        ["v1", "ai", "settings"] if has_ai => {
            let ai = ai(state)?;
            if get {
                Ok(json_response(ok, &json::value(&ai.settings().load()?)))
            } else if method == Method::PUT {
                let mut settings = AiSettings::from_value(body_json(request).await?)?;
                ai.settings().save(&mut settings)?;
                Ok(respond(ok, None))
            } else {
                Ok(method_not_allowed("GET, PUT"))
            }
        }
        ["v1", "ai", "providers"] if has_ai => {
            if !get {
                return Ok(method_not_allowed("GET"));
            }
            Ok(json_response(
                ok,
                &json::value(&CliAiRunner::status(&ai(state)?.settings().load()?)),
            ))
        }
        ["v1", "ai", "delegations"] if has_ai => {
            if !get {
                return Ok(method_not_allowed("GET"));
            }
            let jobs = DelegationJobStore::new(state.config.delegation_root.clone()).list();
            Ok(json_response(ok, &json::value(&jobs)))
        }
        ["v1", "ai", "generate"] if has_ai => {
            if !post {
                return Ok(method_not_allowed("POST"));
            }
            let command = ai_command(body_json(request).await?)?;
            let cancel = CancellationToken::new();
            let _cancel_on_drop = cancel.clone().drop_guard();
            let result = ai(state)?
                .generate(&command, &cancel, None, false, false, None)
                .await?;
            let room = result.room.clone();
            Ok(json_response(
                ok,
                &with_room_display(json::value(&result), &room),
            ))
        }
        ["v1", "requests"] if get => {
            let history = state.config.history.clone();
            let receipts = blocking(move || history()).await?;
            Ok(json_response(
                ok,
                &Value::Array(receipts.iter().map(receipt_json).collect()),
            ))
        }
        ["v1", "requests", _] if get => {
            let id = segments[2].clone();
            if !valid_request_id(&id) {
                return Ok(json_response(
                    StatusCode::BAD_REQUEST,
                    &json!({"code": "invalid_request_id"}),
                ));
            }
            let history = state.config.history.clone();
            let receipts = blocking(move || history()).await?;
            match receipts.iter().find(|r| r.request_id == id) {
                Some(receipt) => Ok(json_response(ok, &receipt_json(receipt))),
                // 영수증이 없다는 사실이 새 ID로 재시도해도 안전하다는 증거는 아니다.
                None => Ok(json_response(
                    StatusCode::NOT_FOUND,
                    &json!({"code": "receipt_unavailable", "detail": "같은 요청 ID를 유지하세요."}),
                )),
            }
        }
        ["v1", "send"] if post => {
            let value = body_json(request).await?;
            if value.is_null() {
                return Err(Error::Api(ApiFailure::new(
                    400,
                    "invalid_request",
                    "JSON 본문이 필요합니다.",
                )));
            }
            let command = ApiSendCommand::from_json(value)?;
            command.validate()?;
            let receipt = (state.config.send)(command).await?;
            Ok(json_response(ok, &receipt_json(&receipt)))
        }
        ["v1", "health"]
        | ["v1", "capabilities"]
        | ["v1", "rooms"]
        | ["v1", "requests"]
        | ["v1", "requests", _] => Ok(method_not_allowed("GET")),
        ["v1", "send-image"] | ["v1", "send"] => Ok(method_not_allowed("POST")),
        _ => Ok(not_found()),
    }
}

/// `POST /v1/ai/generate` 본문. 원본 기본값은 `mode = "analyze"`, `instruction = ""`다.
fn ai_command(mut value: Value) -> Result<AiCommand> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Raw {
        profile: Option<String>,
        room_id: Option<String>,
        mode: Option<String>,
        instruction: Option<String>,
        target_message_id: Option<String>,
    }
    if !value.is_object() {
        return Err(Error::json("JSON 객체가 필요합니다."));
    }
    json::normalize_keys(
        &mut value,
        &[
            "profile",
            "roomId",
            "mode",
            "instruction",
            "targetMessageId",
        ],
        &[],
    );
    let raw: Raw = serde_json::from_value(value)?;
    let mut command = AiCommand::new(
        &raw.profile.unwrap_or_default(),
        &raw.room_id.unwrap_or_default(),
        &raw.mode.unwrap_or_else(|| "analyze".into()),
    );
    command.instruction = raw.instruction.unwrap_or_default();
    command.target_message_id = raw.target_message_id;
    Ok(command)
}
