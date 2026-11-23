//! AI 챗봇 탭: 설정, 방 목록, 자동답장, 대화분석, 프로바이더. 원본: `AiPanel.xaml.cs`, Portable 가져오기.

use std::sync::{Arc, Mutex};

use kakaorelay_ai::ai_service::{AiCommand, AiService};
use kakaorelay_ai::cli_runner::CliAiRunner;
use kakaorelay_ai::json::{self, Escape};
use kakaorelay_ai::settings::{AiProviderSettings, AiSettings, room_key};
use kakaorelay_ai::{ChatContext, LocalRoom};
use kakaorelay_core::command_room::CommandRoomStore;
use kakaorelay_core::host::RelayHost;
use kakaorelay_core::platform::ConversationTarget;
use kakaorelay_core::readiness::open_rooms;
use serde_json::{Value, json};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use super::{App, CmdResult, dialog_result, emit, fail, file_path, room_json};
use crate::state::AppState;

pub const IMPORT_PROFILE: &str = "import";

/// 화면이 마지막으로 받은 로컬 방과 열린 창 서명(원본 `Rooms`, `openWindowSignature`).
#[derive(Default)]
pub struct RoomCache {
    rooms: Vec<LocalRoom>,
    signature: Option<String>,
}

static CACHE: Mutex<RoomCache> = Mutex::new(RoomCache {
    rooms: vec![],
    signature: None,
});

fn cache() -> std::sync::MutexGuard<'static, RoomCache> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn cached_rooms() -> Vec<LocalRoom> {
    cache().rooms.clone()
}

fn update_cached_room(room: &LocalRoom) {
    if let Some(slot) = cache()
        .rooms
        .iter_mut()
        .find(|r| r.profile == room.profile && r.id == room.id)
    {
        *slot = room.clone();
    }
}

/// 원본 `WindowSignature`.
fn signature(windows: &[ConversationTarget]) -> String {
    let mut items: Vec<String> = windows
        .iter()
        .map(|w| format!("{}:{}:{}:{}", w.process_id, w.handle, w.ambiguous, w.title))
        .collect();
    items.sort();
    items.join("\n")
}

/// 원본 `BotBadge`·`BotStatuses`·`LastError`.
pub fn bot_snapshot(host: &RelayHost) -> Value {
    let runtime = host.runtime();
    let count = runtime.count();
    json!({
        "running": count > 0,
        "count": count,
        "badge": if count > 0 { format!("● 자동답장 {count}개 방") } else { "○ 자동답장 꺼짐".into() },
        "statuses": runtime.statuses(),
        "lastError": runtime.last_error(),
    })
}

pub fn settings_json(state: &AppState) -> Value {
    json::value(&state.settings())
}

fn service<'a>(state: &'a AppState, profile: &str) -> &'a Arc<AiService> {
    if profile == IMPORT_PROFILE {
        &state.import_ai
    } else {
        &state.ai
    }
}

#[tauri::command(async)]
pub fn get_settings(state: App<'_>) -> Value {
    settings_json(&state)
}

/// 원본 `AiPanel.Save`. 저장한 설정(검증·정규화 결과)을 돌려준다.
#[tauri::command(async)]
pub fn save_settings(state: App<'_>, settings: Value) -> CmdResult<Value> {
    let next = AiSettings::from_value(settings).map_err(fail)?;
    state.save_settings(next).map_err(fail)?;
    Ok(settings_json(&state))
}

async fn scan(state: &AppState) -> CmdResult<Vec<ConversationTarget>> {
    let platform = state.platform.clone();
    tokio::task::spawn_blocking(move || platform.rooms().scan())
        .await
        .map_err(fail)?
        .map_err(fail)
}

async fn local_rooms(state: &AppState) -> CmdResult<Vec<LocalRoom>> {
    let rooms = state.ai.chats().rooms().await.map_err(|e| e.detail)?;
    if let Some(host) = state.host.get() {
        host.set_known_rooms(rooms.clone());
    }
    cache().rooms = rooms.clone();
    Ok(rooms)
}

async fn rooms_payload(
    state: &AppState,
    rooms: &[LocalRoom],
    windows: &[ConversationTarget],
) -> Value {
    cache().signature = Some(signature(windows));
    let mut all: Vec<Value> = rooms.iter().map(room_json).collect();
    // 가져온 대화는 대화분석에서만 쓴다. 발송 대상이 아니므로 열린 방 계산에 넣지 않는다.
    all.extend(imported_rooms(state).await.iter().map(room_json));
    json!({
        "rooms": all,
        "open": open_rooms(rooms, windows).iter().map(room_json).collect::<Vec<_>>(),
    })
}

/// 가져온 대화방 목록(메모리 리더).
async fn imported_rooms(state: &AppState) -> Vec<LocalRoom> {
    state.import_ai.chats().rooms().await.unwrap_or_default()
}

/// 원본 `Rooms_Click`: 로컬 방을 다시 읽고 열린 창과 맞춘다.
#[tauri::command]
pub async fn ai_rooms(state: App<'_>) -> CmdResult<Value> {
    let rooms = local_rooms(&state).await?;
    let windows = scan(&state).await?;
    Ok(rooms_payload(&state, &rooms, &windows).await)
}

/// 원본 `RefreshOpenRoomsAsync`(2초 주기). 새로 연 방이 로컬 DB보다 먼저 보이면 30초마다 다시 조회한다.
/// `discover_allowed`는 마지막 재조회 뒤 30초가 지났는지다(화면이 시각을 관리한다).
#[tauri::command]
pub async fn poll_open_rooms(state: App<'_>, discover_allowed: bool) -> CmdResult<Value> {
    let mut windows = scan(&state).await?;
    let (mut rooms, previous) = {
        let cache = cache();
        (cache.rooms.clone(), cache.signature.clone())
    };
    let missing = windows
        .iter()
        .any(|w| !rooms.iter().any(|r| r.title == w.title));
    let discover = (rooms.is_empty() || missing) && discover_allowed;
    if discover {
        rooms = local_rooms(&state).await?;
        windows = scan(&state).await?;
    }
    if !discover && previous.as_deref() == Some(signature(&windows).as_str()) {
        return Ok(json!({ "changed": false, "discovered": false }));
    }
    let mut payload = rooms_payload(&state, &rooms, &windows).await;
    payload["changed"] = json!(true);
    payload["discovered"] = json!(discover);
    Ok(payload)
}

#[tauri::command(async)]
pub fn bot_state(state: App<'_>) -> Value {
    match state.host.get() {
        Some(host) => bot_snapshot(host),
        None => {
            json!({ "running": false, "count": 0, "badge": "○ 자동답장 꺼짐", "statuses": [], "lastError": "" })
        }
    }
}

/// 원본 `StartBot_Click`. 제어방으로 쓰는 방은 제외한다.
#[tauri::command]
pub async fn start_bots(state: App<'_>, keys: Vec<String>) -> CmdResult<String> {
    let host = state.host().map_err(fail)?.clone();
    let operation = state.ops.begin("ai").map_err(fail)?;
    let control = CommandRoomStore::new(None).load().map_err(fail)?;
    let rooms = cached_rooms();
    let selected: Vec<LocalRoom> = keys
        .iter()
        .filter_map(|key| rooms.iter().find(|r| &room_key(&r.profile, &r.id) == key))
        .filter(|r| !control.owns(r))
        .cloned()
        .collect();
    let outcome = if selected.is_empty() {
        Err("자동답장할 방을 선택하세요. 제어방은 제외됩니다.".to_owned())
    } else {
        host.runtime()
            .start(&selected, &operation.token)
            .await
            .map(|()| "자동답장 시작".to_owned())
            .map_err(fail)
    };
    drop(operation);
    outcome
}

#[tauri::command]
pub async fn stop_bots(state: App<'_>) -> CmdResult<()> {
    if let Some(host) = state.host.get() {
        host.runtime().stop(None).await;
    }
    Ok(())
}

async fn find_room(state: &AppState, profile: &str, id: &str) -> CmdResult<LocalRoom> {
    let local = cached_rooms();
    let imported = imported_rooms(state).await;
    local
        .into_iter()
        .chain(imported)
        .find(|r| r.profile == profile && r.id == id)
        .ok_or_else(|| "대화방을 다시 선택하세요.".to_owned())
}

/// 원본 `Read_Click`. 한국 시간으로 보여 주고 아직 AI에 전달하지 않는다.
#[tauri::command]
pub async fn read_conversation(state: App<'_>, profile: String, id: String) -> CmdResult<Value> {
    let operation = state.ops.begin("ai").map_err(fail)?;
    let limit = usize::try_from(state.settings().context_messages).unwrap_or(0);
    let reader = service(&state, &profile).chats().clone();
    let outcome = tokio::select! {
        read = reader.read(&profile, &id, limit) => read.map_err(|e| e.detail),
        () = operation.token.cancelled() => Err("중지했습니다.".into()),
    };
    drop(operation);
    let context = outcome?;
    update_cached_room(&context.room);
    let kst = chrono::FixedOffset::east_opt(9 * 3600).expect("오프셋");
    let transcript: Vec<String> = context
        .messages
        .iter()
        .map(|m| {
            format!(
                "{} · {} ({})\n{}",
                m.time.with_timezone(&kst).format("%m-%d %H:%M"),
                m.author,
                m.author_id,
                if m.deleted {
                    "[삭제된 메시지]"
                } else {
                    m.text.as_str()
                }
            )
        })
        .collect();
    Ok(json!({
        "room": room_json(&context.room),
        "transcript": transcript.join("\n\n"),
        "hint": format!("{}개 메시지 · 한국 시간 · 첨부 미디어 제외", context.messages.len()),
    }))
}

/// 원본 `Generate_Click`. 진행 상황은 `ai-progress` 이벤트로 보낸다.
#[tauri::command]
pub async fn generate(
    app: AppHandle,
    state: App<'_>,
    profile: String,
    id: String,
    mode: String,
    question: String,
) -> CmdResult<Value> {
    let operation = state.ops.begin("ai").map_err(fail)?;
    let mut command = AiCommand::new(&profile, &id, &mode);
    command.instruction = question;
    let progress_app = app.clone();
    let progress = move |text: &str| emit(&progress_app, "ai-progress", text.to_owned());
    let outcome = service(&state, &profile)
        .generate(
            &command,
            &operation.token,
            Some(&progress),
            false,
            false,
            None,
        )
        .await;
    drop(operation);
    let result = outcome.map_err(|e| {
        if e.is_cancelled() {
            "중지했습니다.".into()
        } else {
            e.to_string()
        }
    })?;
    update_cached_room(&result.room);
    let attempts: Vec<String> = result
        .attempts
        .iter()
        .map(|a| format!("{} {}", a.provider, a.status))
        .collect();
    let mut status = format!(
        "{} · {} · effort {} · {}개 문맥 · {}",
        result.provider,
        result.model,
        result.effort,
        result.message_count,
        attempts.join(" → ")
    );
    if let Some(path) = &result.image_path {
        status.push_str(&format!(" · 생성 이미지 저장: {path}"));
    }
    if let Some(error) = &result.image_error {
        status.push_str(&format!(" · 이미지 생성 실패: {error}"));
    }
    Ok(json!({
        "text": result.text,
        "room": room_json(&result.room),
        "personaId": result.persona_id,
        "mode": mode,
        "status": status,
    }))
}

#[tauri::command]
pub fn cancel_ai(state: App<'_>) {
    state.ops.cancel("ai");
}

/// 원본 `Export_Click`. 웹 기본값(camelCase)·들여쓰기·PromptJson 이스케이프로 저장한다.
#[tauri::command]
pub async fn export_conversation(
    app: AppHandle,
    state: App<'_>,
    profile: String,
    id: String,
) -> CmdResult<Option<String>> {
    let picked = dialog_result(|tx| {
        app.dialog()
            .file()
            .add_filter("KakaoRelay 대화 JSON", &["json"])
            .set_file_name("kakao-conversation.json")
            .save_file(move |path| {
                let _ = tx.send(path.and_then(file_path));
            });
    })
    .await
    .flatten();
    let Some(path) = picked else { return Ok(None) };
    let operation = state.ops.begin("ai").map_err(fail)?;
    let limit = usize::try_from(state.settings().context_messages).unwrap_or(0);
    let reader = service(&state, &profile).chats().clone();
    let context: ChatContext = tokio::select! {
        read = reader.read(&profile, &id, limit) => read.map_err(|e| e.detail)?,
        () = operation.token.cancelled() => return Err("중지했습니다.".into()),
    };
    let text = json::to_string(&json::value(&context), Escape::Prompt, true);
    tokio::fs::write(&path, text).await.map_err(fail)?;
    Ok(Some(
        "선택한 문맥을 JSON으로 저장했습니다. macOS·Linux 버전에서 가져올 수 있습니다.".into(),
    ))
}

/// Portable `POST /api/import`: 카카오톡 한국어 TXT 또는 KakaoRelay JSON을 가져온다. 파일은 앱 메모리에서만 읽는다.
#[tauri::command]
pub async fn import_conversation(app: AppHandle, state: App<'_>) -> CmdResult<Option<Value>> {
    let picked = dialog_result(|tx| {
        app.dialog()
            .file()
            .set_title("대화 파일 가져오기")
            .add_filter("카카오톡 TXT · KakaoRelay JSON", &["txt", "json"])
            .pick_file(move |path| {
                let _ = tx.send(path.and_then(file_path));
            });
    })
    .await
    .flatten();
    let Some(path) = picked else { return Ok(None) };
    let size = tokio::fs::metadata(&path).await.map_err(fail)?.len();
    if size > 10_000_000 {
        return Err("10MB 이하 파일을 선택하세요.".into());
    }
    let content = tokio::fs::read_to_string(&path).await.map_err(fail)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let room = state.imported.import(&name, &content).map_err(fail)?;
    Ok(Some(room_json(&room)))
}

/// 원본 `SaveKnowledgeNote_Click`. 분석 결과를 그 방의 페르소나 보관함에 저장한다.
#[tauri::command]
pub async fn save_summary_note(
    state: App<'_>,
    profile: String,
    id: String,
    persona_id: String,
    body: String,
) -> CmdResult<String> {
    let room = find_room(&state, &profile, &id).await?;
    let settings = state.settings();
    let knowledge = settings
        .personas
        .iter()
        .find(|p| p.id == persona_id)
        .map(|p| p.knowledge.clone())
        .ok_or_else(|| "페르소나를 찾지 못했습니다.".to_owned())?;
    let operation = state.ops.begin("ai").map_err(fail)?;
    let path = kakaorelay_ai::obsidian::save_note(
        &knowledge,
        &format!("{} 대화 요약", room.title),
        &body,
        Some(&room),
        &operation.token,
    )
    .await
    .map_err(fail)?;
    Ok(format!("요약 노트 저장: {}", path.display()))
}

/// 원본 `Detect`의 설치 상태 줄.
#[tauri::command(async)]
pub fn detect_providers(settings: Value) -> CmdResult<String> {
    let settings = AiSettings::from_value(settings).map_err(fail)?;
    Ok(CliAiRunner::status(&settings)
        .iter()
        .map(|s| format!("{}: {}", s.provider, s.detail))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// 원본 `Detect`의 프로바이더별 모델 조회. 저장된 모델 선택은 바꾸지 않는다.
#[tauri::command]
pub async fn query_models(id: String, executable: String, model: String) -> Value {
    let provider = AiProviderSettings {
        executable,
        ..AiProviderSettings::new(&id)
    };
    let token = kakaorelay_ai::cancel::CancellationToken::new();
    match kakaorelay_ai::model_catalog::query(&provider, &token).await {
        Ok(catalog) => json!({
            "status": format!(
                "CLI 제공 {}개 · {}{}",
                catalog.models.len(),
                catalog.checked_at.with_timezone(&chrono::Local).format("%H:%M:%S"),
                if catalog.login_required { " · CLI 로그인 필요" } else { " · 실행 권한은 실제 응답 시 확인" }
            ),
            "models": catalog.models.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
            "choices": kakaorelay_ai::model_catalog::choices(&catalog.models, &model),
        }),
        Err(error) => json!({
            "status": format!("목록 조회 실패 · {error}"),
            "models": [],
            "choices": kakaorelay_ai::model_catalog::choices(&[], &model),
        }),
    }
}

fn greeting_store() -> kakaorelay_core::greetings::GreetingStore {
    kakaorelay_core::greetings::GreetingStore::new(
        kakaorelay_core::greetings::GreetingStore::default_path(),
    )
}

/// 자동인사를 켠 방의 키 목록(원본에 없는 기능).
#[tauri::command(async)]
pub fn greeting_rooms() -> CmdResult<Vec<String>> {
    greeting_store().load().map(|s| s.rooms).map_err(fail)
}

/// 한 방의 자동인사를 켜거나 끈다. 돌고 있는 자동답장 세션은 다음 폴링부터 반영한다.
#[tauri::command(async)]
pub fn set_greeting(key: String, enabled: bool) -> CmdResult<Vec<String>> {
    greeting_store()
        .set(&key, enabled)
        .map(|s| s.rooms)
        .map_err(fail)
}
