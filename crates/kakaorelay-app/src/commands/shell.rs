//! 앱 공통: 첫 화면 정보, 외부 열기, 권한 상태, 파일 대화상자.

use std::path::Path;

use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use super::{App, CmdResult, dialog_result, file_path};
use crate::app::ShutdownReady;

const API_DOCS: &str = include_str!("../../../../docs/API.md");
const OPENAPI: &str = include_str!("../../../../docs/openapi.json");

#[tauri::command(async)]
pub fn bootstrap(state: App<'_>) -> Value {
    json!({
        "version": crate::os::version_text(),
        "platform": if cfg!(target_os = "macos") { "macos" } else { "windows" },
        "apiStatus": state.api_status.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        "hostReady": state.host.get().is_some(),
        "settings": kakaorelay_ai::json::value(&state.settings()),
        "settingsError": state.settings_error,
        "report": state.report.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|p| p.to_string_lossy().into_owned()),
        "apiDocs": API_DOCS,
        "openApi": OPENAPI,
        "permissions": crate::os::permissions(),
    })
}

#[tauri::command(async)]
pub fn permissions() -> Value {
    crate::os::permissions()
}

/// 기본 앱으로 연다. 화면이 넘길 수 있는 대상은 있는 파일·폴더와 옵시디언·시스템 설정 URL로 한정한다.
#[tauri::command]
pub fn open_target(target: String) -> CmdResult<()> {
    let allowed = target.starts_with("obsidian://open?path=")
        || target.starts_with("x-apple.systempreferences:")
        || Path::new(&target).exists();
    if !allowed {
        return Err("파일을 열 수 없습니다.".into());
    }
    crate::os::open(&target).map_err(|_| "파일을 열 수 없습니다.".to_owned())
}

#[tauri::command]
pub fn shutdown_ready(app: AppHandle) {
    app.state::<ShutdownReady>().0.notify_one();
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle, title: String) -> CmdResult<Option<String>> {
    let picked = dialog_result(|tx| {
        app.dialog()
            .file()
            .set_title(title)
            .pick_folder(move |path| {
                let _ = tx.send(path.and_then(file_path));
            });
    })
    .await
    .flatten();
    Ok(picked.map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
pub async fn pick_files(app: AppHandle) -> CmdResult<Vec<String>> {
    let picked = dialog_result(|tx| {
        app.dialog()
            .file()
            .set_title("지식 자료 추가")
            .add_filter("지식 자료", &["md", "txt", "pdf", "docx"])
            .pick_files(move |paths| {
                let _ = tx.send(paths.unwrap_or_default());
            });
    })
    .await
    .unwrap_or_default();
    Ok(picked
        .into_iter()
        .filter_map(file_path)
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

/// 개발 빌드에서만 화면 스크립트 오류를 표준 오류로 남긴다(본문·방 이름은 넘기지 않는다).
#[tauri::command]
pub fn ui_log(text: String) {
    if cfg!(debug_assertions) {
        eprintln!("[ui] {text}");
    }
}
