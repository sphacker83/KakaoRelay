//! 웹뷰가 부르는 Tauri command. 화면은 HTTP API가 아니라 이 command로 core·ai를 호출한다.
//! 오류는 원본 화면처럼 예외 메시지 문자열로 돌려준다.

pub mod ai;
pub mod control;
pub mod forward;
pub mod images;
pub mod knowledge;
pub mod messages;
pub mod shell;

use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;

use kakaorelay_ai::LocalRoom;
use serde_json::{Value, json};
use tauri::{AppHandle, Runtime, State};

use crate::state::AppState;

pub type App<'a> = State<'a, Arc<AppState>>;
pub type CmdResult<T> = Result<T, String>;

pub fn fail(error: impl Display) -> String {
    error.to_string()
}

pub fn handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        shell::bootstrap,
        shell::open_target,
        shell::shutdown_ready,
        shell::permissions,
        shell::pick_folder,
        shell::pick_files,
        shell::ui_log,
        messages::scan_rooms,
        messages::check_target,
        messages::compose_reset,
        messages::send_manual,
        messages::history,
        messages::observe,
        messages::open_history_folder,
        messages::diagnose,
        messages::load_report,
        messages::open_report,
        ai::get_settings,
        ai::greeting_rooms,
        ai::set_greeting,
        ai::save_settings,
        ai::ai_rooms,
        ai::poll_open_rooms,
        ai::start_bots,
        ai::stop_bots,
        ai::bot_state,
        ai::read_conversation,
        ai::generate,
        ai::cancel_ai,
        ai::export_conversation,
        ai::import_conversation,
        ai::save_summary_note,
        ai::detect_providers,
        ai::query_models,
        images::images_load,
        images::images_generate,
        images::images_transparent,
        images::images_manage,
        images::images_open_folder,
        images::images_cancel,
        knowledge::notes_list,
        knowledge::note_preview,
        knowledge::notes_import,
        knowledge::notes_search,
        knowledge::note_write,
        knowledge::knowledge_save_connection,
        knowledge::create_vault,
        knowledge::knowledge_cancel,
        knowledge::auto_rooms,
        knowledge::auto_apply,
        knowledge::auto_run,
        knowledge::auto_review_folder,
        knowledge::auto_statuses,
        forward::forward_rooms,
        forward::forward_apply,
        forward::forward_delete,
        control::control_load,
        control::control_rooms,
        control::control_apply,
    ]
}

/// 화면에 보내는 방. `key`는 설정의 방 키(`AiSettings.RoomKey`).
pub fn room_json(room: &LocalRoom) -> Value {
    json!({
        "profile": room.profile,
        "id": room.id,
        "title": room.title,
        "readable": room.readable,
        "directMemberId": room.direct_member_id,
        "display": room.display(),
        "key": kakaorelay_ai::settings::room_key(&room.profile, &room.id),
    })
}

/// 파일 대화상자 결과를 받는다. 대화상자는 콜백형 API를 써서 비동기 command 스레드를 막지 않는다.
pub async fn dialog_result<T: Send + 'static>(
    start: impl FnOnce(tokio::sync::oneshot::Sender<T>),
) -> Option<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    start(tx);
    rx.await.ok()
}

pub fn file_path(path: tauri_plugin_dialog::FilePath) -> Option<PathBuf> {
    path.into_path().ok()
}

pub fn emit<R: Runtime>(app: &AppHandle<R>, event: &str, payload: impl serde::Serialize + Clone) {
    use tauri::Emitter;
    let _ = app.emit(event, payload);
}
