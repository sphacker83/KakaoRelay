//! 메시지·API 탭. 원본: `MainWindow.xaml.cs`(방 새로고침, 대상 확인, 수동 발송, 이력, 대화 확인 완료, 진단).

use std::path::{Path, PathBuf};
use std::time::Duration;

use kakaorelay_core::diagnostics::{DiagnosticReport, report_store};
use kakaorelay_core::platform::ConversationTarget;
use kakaorelay_core::test_send::{
    TestSendReceipt, TestSendRequest, read_history, record_observed, send_once,
};
use serde_json::{Value, json};

use super::{App, CmdResult, fail};

/// 원본 `MainWindow.Friendly`. 어댑터의 영문 사유를 사용자 안내로 바꾼다.
pub fn friendly(message: &str) -> String {
    let lower = message.to_lowercase();
    let text = if lower.contains("draft") {
        "카카오톡 입력 내용이 예상과 다릅니다. 기존 초안은 덮어쓰지 않았습니다."
    } else if message.contains("ambiguous") {
        "받는 대화방을 하나로 확인할 수 없습니다. 대화방 목록을 새로고침하세요."
    } else if ["identity", "changed", "exists", "editor"]
        .iter()
        .any(|w| message.contains(w))
    {
        "대화방 또는 입력창 상태가 바뀌었습니다. 목록을 새로고침하세요."
    } else if message.contains("desktop") {
        "PC가 잠겨 있거나 현재 화면을 사용할 수 없습니다."
    } else if message.contains("modifier") {
        "Shift, Ctrl 또는 Alt 키가 눌려 있어 전송을 중단했습니다."
    } else if message.contains("progress") {
        "다른 작업을 처리 중입니다. 잠시 후 다시 확인하세요."
    } else {
        "작업을 완료하지 못했습니다. 발송 이력과 대화방 상태를 확인하세요."
    };
    text.into()
}

/// 원본 `MainWindow.DescribeReceipt`.
pub fn describe(receipt: &TestSendReceipt) -> String {
    match receipt.status.as_str() {
        "observed-in-chat" => "새 발신 말풍선 확인이 기록되었습니다. 상대방 수신·읽음 여부는 별도입니다.".into(),
        "blocked-before-input" => friendly(&receipt.detail),
        "needs-review" if receipt.kind == "image" => if receipt.enter_posted {
            "이미지 전송을 요청했습니다. 대화의 새 첨부 이미지를 확인하세요. 자동 재전송하지 않습니다."
        } else {
            "이미지 첨부 결과 확인이 필요합니다. 대화를 확인하고 중복 전송하지 마세요."
        }
        .into(),
        "needs-review" if receipt.enter_posted => format!(
            "{}새 발신 말풍선을 확인한 뒤 ‘대화에서 확인 완료’를 누르세요. 자동 재전송하지 않습니다.",
            if receipt.input_cleared == Some(true) {
                "입력창이 비워졌습니다. "
            } else {
                "입력창 비움을 확인하지 못했습니다. "
            }
        ),
        "continued-after-visual-review" => "이전 시험에서 남은 초안을 별도 요청으로 이어 처리한 기록입니다.".into(),
        _ => "완료되지 않은 작업 기록입니다. 대화를 먼저 확인하고 중복 전송하지 마세요.".into(),
    }
}

fn observable(receipt: &TestSendReceipt) -> bool {
    receipt.status == "needs-review" && receipt.enter_posted
}

fn receipt_json(receipt: &TestSendReceipt) -> Value {
    json!({
        "requestId": receipt.request_id,
        "recipient": receipt.recipient,
        "fingerprint": receipt.fingerprint,
        "status": receipt.status,
        "displayTime": receipt.display_time(),
        "displayStatus": receipt.display_status(),
        "describe": describe(receipt),
        "observable": observable(receipt),
    })
}

fn target_json(target: &ConversationTarget) -> Value {
    json!({
        "title": target.title,
        "processId": target.process_id,
        "handle": target.handle,
        "ambiguous": target.ambiguous,
        "selectable": target.selectable(),
        "hint": target.hint(),
    })
}

fn preflight(target: &ConversationTarget, message: &str) -> TestSendRequest {
    let mut request = TestSendRequest::new(
        format!("app-{}", uuid::Uuid::new_v4().simple()),
        target.title.clone(),
        message,
        target.process_id,
        target.handle.clone(),
        chrono::Local::now().fixed_offset() + chrono::Duration::minutes(5),
    );
    request.room_id = target.room_id.clone();
    request
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> CmdResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(fail)
}

/// 원본 `RefreshRoomsAsync`. 작업 중 표시(수동 작업)로 API 발송과 겹치지 않는다.
#[tauri::command]
pub async fn scan_rooms(state: App<'_>) -> CmdResult<Vec<Value>> {
    let platform = state.platform.clone();
    let rooms = state
        .exclusive(blocking(move || platform.rooms().scan()))
        .await?
        .map_err(|_| "대화방을 읽지 못했습니다. ↻를 눌러 다시 확인하세요.".to_owned())?;
    let json = rooms.iter().map(target_json).collect();
    state
        .manual
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .targets = rooms;
    Ok(json)
}

/// 원본 `CheckTargetAsync`. 맥은 입력창을 읽을 수 없어 초안 판정은 발송 직전 보호(Cmd+A·Cmd+C)로 대신한다.
#[tauri::command]
pub async fn check_target(state: App<'_>, target: ConversationTarget) -> CmdResult<Value> {
    let target = resolve(&state, target);
    if !target.selectable() {
        return Ok(
            json!({ "ready": false, "hint": friendly("Recipient window is missing or ambiguous") }),
        );
    }
    let platform = state.platform.clone();
    let checked = state
        .exclusive(blocking(move || {
            let mut probe = platform.text_transport();
            let request = preflight(&target, "preflight");
            probe.validate_target(&request)?;
            probe.read_draft()
        }))
        .await?;
    Ok(match checked {
        Ok(draft) => {
            let ready = draft.is_none_or(|d| d.is_empty() || d == "메시지 입력");
            json!({
                "ready": ready,
                "hint": if ready { "보낼 대화방을 확인했습니다." } else { "카카오톡에 작성 중인 초안이 있습니다. 먼저 해당 초안을 정리하세요." },
            })
        }
        Err(error) => json!({ "ready": false, "hint": friendly(&error.to_string()) }),
    })
}

/// 화면이 돌려준 대상을 마지막 목록의 원본 값으로 바꾼다(화면에 보내지 않은 필드 보존).
fn resolve(state: &crate::state::AppState, target: ConversationTarget) -> ConversationTarget {
    let manual = state.manual.lock().unwrap_or_else(|e| e.into_inner());
    manual
        .targets
        .iter()
        .find(|t| {
            t.title == target.title
                && t.process_id == target.process_id
                && t.handle == target.handle
        })
        .cloned()
        .unwrap_or(target)
}

/// 본문이나 대상이 바뀌면 같은 요청 ID 유지를 끝낸다(원본 `compose.Reset`).
#[tauri::command]
pub fn compose_reset(state: App<'_>) {
    state
        .manual
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .compose
        .reset();
}

/// 원본 `Send_Click`. 한 번 시작한 요청은 확인될 때까지 같은 요청 ID로 유지한다.
#[tauri::command]
pub async fn send_manual(
    state: App<'_>,
    target: ConversationTarget,
    body: String,
) -> CmdResult<Value> {
    if state.is_shutting_down() {
        return Err("앱이 종료 중입니다.".into());
    }
    let target = resolve(&state, target);
    let request = {
        let mut manual = state.manual.lock().unwrap_or_else(|e| e.into_inner());
        match manual.compose.request() {
            Some(request) => request.clone(),
            None => manual
                .compose
                .begin(Some(&target), &body, true)
                .map_err(|e| friendly(&e.to_string()))?,
        }
    };
    let (platform, ledger) = (state.platform.clone(), state.ledger.clone());
    let sent = state
        .exclusive(blocking(move || {
            let mut transport = platform.text_transport();
            send_once(&request, transport.as_mut(), &ledger)
        }))
        .await?;
    match sent {
        Ok(receipt) => {
            let mut manual = state.manual.lock().unwrap_or_else(|e| e.into_inner());
            let cleared = manual.compose.complete(&receipt);
            manual.current = Some(receipt.clone());
            Ok(json!({
                "status": if receipt.enter_posted { "전송 요청 처리 · 대화 확인 필요" } else { "전송 중단" },
                "detail": describe(&receipt),
                "cleared": cleared,
                "receipt": receipt_json(&receipt),
            }))
        }
        Err(error) => Ok(json!({
            "status": "처리 결과 확인 필요",
            "detail": format!("{} 자동으로 다시 보내지 않습니다.", friendly(&error.to_string())),
            "cleared": false,
            "receipt": null,
        })),
    }
}

#[tauri::command(async)]
pub fn history(state: App<'_>) -> CmdResult<Vec<Value>> {
    read_history(&state.ledger)
        .map(|items| items.iter().map(receipt_json).collect())
        .map_err(|_| "기록을 읽을 수 없습니다. 이력 새로고침을 눌러주세요.".to_owned())
}

/// 원본 `Observe`. 새 발신 말풍선을 눈으로 확인한 경우에만 기록한다.
#[tauri::command(async)]
pub fn observe(state: App<'_>, request_id: String, fingerprint: String) -> CmdResult<()> {
    record_observed(&request_id, &state.ledger, Some(&fingerprint))
        .map_err(|e| friendly(&e.to_string()))?;
    let mut manual = state.manual.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(current) = manual
        .current
        .as_mut()
        .filter(|c| c.request_id == request_id)
    {
        current.status = "observed-in-chat".into();
    }
    Ok(())
}

#[tauri::command]
pub fn open_history_folder(state: App<'_>) -> CmdResult<()> {
    std::fs::create_dir_all(&state.ledger).map_err(fail)?;
    crate::os::open(&state.ledger.to_string_lossy())
        .map_err(|_| "파일을 열 수 없습니다.".to_owned())
}

fn report_json(path: &Path, report: &DiagnosticReport) -> Value {
    let windows: Vec<Value> = report
        .windows
        .iter()
        .map(|w| {
            json!({
                "displayTitle": w.display_title(),
                "displayState": w.display_state(),
                "visible": w.visible,
                "controls": w.controls.iter().map(|c| json!({ "className": c.class_name, "patterns": c.patterns() })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "path": path.to_string_lossy(),
        "title": if matches!(report.status.as_str(), "complete" | "partial") { "진단 완료" } else { "진단 중단 · 부분 결과" },
        "summary": report.summary(),
        "windows": windows,
    })
}

/// 원본 `Diagnose_Click`. 진단은 별도 worker 프로세스로 돌려 응답 없는 자동화가 앱을 멈추지 않게 한다.
#[tauri::command]
pub async fn diagnose(state: App<'_>) -> CmdResult<Value> {
    if state.is_shutting_down() {
        return Err("앱이 종료 중입니다.".into());
    }
    let operation = state.ops.begin("diagnose").map_err(fail)?;
    let path = kakaorelay_ai::paths::app_data()
        .join("diagnostics")
        .join(format!("diagnostic-{}.json", uuid::Uuid::new_v4().simple()));
    let executable = std::env::current_exe().map_err(|e| friendly(&e.to_string()))?;
    let report = state
        .exclusive(kakaorelay_core::probe::run(
            &executable,
            &path,
            Duration::from_secs(20),
            &operation.token,
        ))
        .await;
    *state.report.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.clone());
    Ok(report_json(&path, &report))
}

/// 원본 `LoadReport`(`--report <경로>`).
#[tauri::command(async)]
pub fn load_report(path: String) -> CmdResult<Value> {
    let path = PathBuf::from(path);
    match report_store::load(&path) {
        Ok(Some(report)) => Ok(report_json(&path, &report)),
        _ => Err("결과를 열 수 없습니다".into()),
    }
}

#[tauri::command]
pub fn open_report(state: App<'_>) -> CmdResult<()> {
    let path = state
        .report
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    match path.filter(|p| p.exists()) {
        Some(path) => crate::os::open(&path.to_string_lossy())
            .map_err(|_| "파일을 열 수 없습니다.".to_owned()),
        None => Err("파일을 열 수 없습니다.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_maps_adapter_reasons() {
        assert!(friendly("Draft changed").contains("초안"));
        assert!(friendly("Recipient window is missing or ambiguous").contains("하나로"));
        assert!(friendly("Input desktop is locked").contains("잠겨"));
        assert!(friendly("anything").contains("발송 이력"));
    }
}
