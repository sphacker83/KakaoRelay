//! 창·트레이·앱 수명. 원본: `App.xaml.cs`(시작·종료 순서), `TrayHost.cs`(트레이 메뉴), `MainWindow`(닫기 = 숨기기).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use kakaorelay_core::host::{HostPaths, RelayHost};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, RunEvent, WindowEvent, Wry};

use crate::instance::{self, Start};
use crate::state::AppState;

const TRAY_ID: &str = "main";

/// 트레이 메뉴의 '완전 종료' 항목. 종료 중 문구로 바꾸려고 보관한다.
struct TrayParts {
    exit: MenuItem<Wry>,
    _icon: TrayIcon<Wry>,
}

/// 종료 시작 전 화면이 디바운스 중인 설정을 저장했다고 알리는 신호.
#[derive(Default)]
pub struct ShutdownReady(pub tokio::sync::Notify);

pub fn run(resume_replies: bool, report: Option<String>) -> i32 {
    let guard = match instance::acquire() {
        Ok(Start::Primary(guard)) => guard,
        Ok(Start::AlreadyRunning) => return 0,
        // 잠금 자체를 만들 수 없으면 중복 실행을 막을 수 없으므로 시작하지 않는다.
        Err(_) => return 2,
    };
    let report = report.map(|p| kakaorelay_core::fs::full_path(std::path::Path::new(&p)));
    let state = Arc::new(AppState::new(resume_replies, report));
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state.clone())
        .manage(ShutdownReady::default())
        .invoke_handler(crate::commands::handler())
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                let state = window.state::<Arc<AppState>>();
                if !state.is_shutting_down() {
                    api.prevent_close();
                    hide_main(window.app_handle());
                }
            }
            WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
                let paths: Vec<String> = paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                let _ = window.emit("drop-files", paths);
            }
            _ => {}
        })
        .setup(move |app| {
            build_tray(app.handle())?;
            let handle = app.handle().clone();
            guard.listen(move || show_main(&handle));
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(start_services(handle));
            // 가드는 앱 종료까지 살아 있어야 한다. 해제는 `RunEvent::Exit`에서 한다.
            app.manage(GuardSlot(std::sync::Mutex::new(Some(guard))));
            Ok(())
        })
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(_) => return 2,
    };
    app.run_return(|handle, event| match event {
        RunEvent::ExitRequested { api, .. } => {
            // macOS Cmd+Q 등 시스템 종료 요청도 트레이 '완전 종료'와 같은 순서로 끝낸다.
            if !handle.state::<Arc<AppState>>().is_shutting_down() {
                api.prevent_exit();
                tauri::async_runtime::spawn(exit_gracefully(handle.clone()));
            }
        }
        // 이벤트 루프(주 스레드)에서 단일 인스턴스 가드를 놓는다. Windows mutex는 획득한 스레드에서만 풀린다.
        RunEvent::Exit => {
            if let Some(slot) = handle.try_state::<GuardSlot>() {
                slot.0.lock().unwrap_or_else(|e| e.into_inner()).take();
            }
        }
        _ => {}
    })
}

struct GuardSlot(std::sync::Mutex<Option<instance::Guard>>);

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "열기", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let exit = MenuItem::with_id(app, "exit", "완전 종료", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &separator, &exit])?;
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))?;
    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("KakaoRelay · 실행 중")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "exit" => {
                tauri::async_runtime::spawn(exit_gracefully(app.clone()));
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(TrayParts { exit, _icon: tray });
    Ok(())
}

/// 원본 `ShowMainWindow`. 맥은 창이 보일 때만 Dock에 표시한다(원본: 작업 표시줄에 창 표시).
pub fn show_main(app: &AppHandle) {
    if app.state::<Arc<AppState>>().is_shutting_down() {
        return;
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn hide_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

pub fn emit(app: &AppHandle, event: &str, payload: impl serde::Serialize + Clone) {
    let _ = app.emit(event, payload);
}

/// 원본 `OnStartup`의 서비스 조립. 실패하면 화면은 열어 두고 API 상태로만 알린다.
async fn start_services(app: AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let started = async {
        kakaorelay_ai::provider_storage::initialize()?;
        RelayHost::start(
            state.platform.clone(),
            state.ai.clone(),
            HostPaths::default(),
        )
        .await
    }
    .await;
    let host = match started {
        Ok(host) => host,
        Err(_) => {
            state.set_api_status("AI API 시작 실패 · 앱을 다시 실행하세요.");
            emit(
                &app,
                "api-status",
                "AI API 시작 실패 · 앱을 다시 실행하세요.",
            );
            return;
        }
    };
    subscribe(&app, &host);
    // 원본 `ExecuteCommandAsync`: AI 화면·이미지 작업 중에는 제어방 명령을 거절한다.
    let weak = Arc::downgrade(&host);
    state.ops.on_change(move |ops| {
        if let Some(host) = weak.upgrade() {
            host.set_settings_busy(ops.busy("ai") || ops.busy("images"));
        }
    });
    let _ = state.host.set(host.clone());
    state.set_api_status("AI API 연결됨 · 로컬 게이트웨이 실행 중");
    emit(
        &app,
        "api-status",
        "AI API 연결됨 · 로컬 게이트웨이 실행 중",
    );
    emit(&app, "host-ready", ());
    fill_self_id(&state).await;
    if state.resume_replies {
        let token = kakaorelay_ai::cancel::CancellationToken::new();
        let text = match host.resume_selected_replies(&token).await {
            Ok(()) => "저장된 자동답장 방 복원 완료".to_owned(),
            Err(error) => format!("자동답장 복원 실패: {error}"),
        };
        emit(&app, "ai-status", text);
    }
}

/// 설정의 '카카오 ID'가 비어 있으면 어댑터가 찾은 ID로 채운다(맥은 DB 키 유도에 쓴 ID).
/// 저장은 `RelayHost`를 거치므로 `settings-changed`로 화면에도 반영된다. 사용자가 입력한 값은 덮어쓰지 않는다.
async fn fill_self_id(state: &Arc<AppState>) {
    if !state.settings().self_id.trim().is_empty() {
        return;
    }
    let platform = state.platform.clone();
    let Ok(Some(id)) = tokio::task::spawn_blocking(move || platform.detected_self_id()).await
    else {
        return;
    };
    let mut next = state.settings();
    if next.self_id.trim().is_empty() {
        next.self_id = id;
        // 실패해도 화면의 기존 안내(카카오 ID 직접 입력)가 그대로 남으므로 따로 알리지 않는다.
        let _ = state.save_settings(next);
    }
}

fn subscribe(app: &AppHandle, host: &Arc<RelayHost>) {
    {
        let (app, weak) = (app.clone(), Arc::downgrade(host));
        host.runtime().on_changed(move || {
            if let Some(host) = weak.upgrade() {
                emit(&app, "bots", crate::commands::ai::bot_snapshot(&host));
            }
        });
    }
    let forward = app.clone();
    host.forwarder()
        .status
        .subscribe(move |text| emit(&forward, "forward-status", text.to_owned()));
    let commands = app.clone();
    host.commands()
        .on_status(move |text| emit(&commands, "command-status", text.to_owned()));
    let settings = app.clone();
    host.settings_changed
        .subscribe(move |_| emit(&settings, "settings-changed", ()));
    let knowledge = app.clone();
    host.knowledge_errors
        .subscribe(move |text| emit(&knowledge, "knowledge-status", text.to_owned()));
    let knowledge = app.clone();
    host.ai()
        .automatic_knowledge
        .on_status(move |text| emit(&knowledge, "knowledge-status", text.to_owned()));
    let sent = app.clone();
    host.coordinator()
        .sent
        .subscribe(move |text| emit(&sent, "api-sent", text.to_owned()));
}

/// 원본 `ExitFromTrayAsync`: 새 작업 차단 → 화면 작업 중지·설정 저장 → 봇·전달·제어방 드레인 → 진행 중 발송 대기 → API 정지.
pub async fn exit_gracefully(app: AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if state.shutting_down.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some("KakaoRelay · 작업 완료 후 종료"));
    }
    if let Some(parts) = app.try_state::<TrayParts>() {
        let _ = parts.exit.set_enabled(false);
        let _ = parts.exit.set_text("종료 중…");
    }
    state.set_api_status("작업 완료 후 종료 중");
    emit(&app, "prepare-shutdown", ());
    // 화면이 디바운스 중인 설정을 저장할 시간을 준다. 화면이 응답하지 않아도 종료는 계속한다.
    let ready = app.state::<ShutdownReady>();
    let _ = tokio::time::timeout(Duration::from_secs(2), ready.0.notified()).await;
    state.ops.cancel_all();
    if let Some(host) = state.host.get() {
        host.shutdown().await;
    }
    // 수동 발송·창 확인이 최종 영수증을 저장할 때까지 기다린다.
    drop(state.native.lock().await);
    state.runner.dispose();
    app.exit(0);
}
