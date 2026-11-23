//! OS 선택과 OS별 작은 기능(파일·URL 열기, 권한 상태, 진단 수집).

use std::path::Path;
use std::sync::Arc;

use kakaorelay_core::diagnostics::DiagnosticReport;
use kakaorelay_core::platform::Platform;
use serde_json::{Value, json};

#[cfg(not(any(target_os = "macos", windows)))]
compile_error!("KakaoRelay 앱은 macOS와 Windows만 지원합니다.");

/// 대상 OS의 어댑터. 맥은 설정의 '내 카카오 ID'를 DB 키 유도와 발신 확인에 쓴다.
#[cfg(target_os = "macos")]
pub fn platform(self_id: &str) -> Arc<dyn Platform> {
    Arc::new(kakaorelay_macos::MacPlatform::new(
        self_id.trim().parse().ok(),
    ))
}

#[cfg(windows)]
pub fn platform(_self_id: &str) -> Arc<dyn Platform> {
    Arc::new(kakaorelay_windows::WindowsPlatform::new())
}

/// Finder·`open`으로 실행한 맥 앱은 PATH가 `/usr/bin:/bin:/usr/sbin:/sbin`뿐이라 bun·npm·Homebrew로 설치한
/// AI CLI(`codex`는 `#!/usr/bin/env node` 스크립트)를 찾지 못한다(2026-11-23 실측). 로그인 셸의 PATH를 앞에 합친다.
/// 셸이 5초 안에 끝나지 않거나 실패하면 기존 PATH를 그대로 쓴다.
#[cfg(target_os = "macos")]
pub fn import_login_path() {
    use std::io::Read;
    use std::process::{Command, Stdio};
    const MARK: &str = "__KAKAORELAY_PATH__";
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let Ok(mut child) = Command::new(&shell)
        .args(["-ilc", &format!("printf '{MARK}%s{MARK}' \"$PATH\"")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
    let mut output = String::new();
    if child
        .stdout
        .take()
        .is_none_or(|mut out| out.read_to_string(&mut output).is_err())
    {
        return;
    }
    let Some(login) = output.split(MARK).nth(1).filter(|p| !p.is_empty()) else {
        return;
    };
    let current = std::env::var("PATH").unwrap_or_default();
    let mut merged: Vec<&str> = vec![];
    for dir in login.split(':').chain(current.split(':')) {
        if !dir.is_empty() && !merged.contains(&dir) {
            merged.push(dir);
        }
    }
    // SAFETY: main 첫 줄에서 다른 스레드를 만들기 전에 한 번만 호출한다.
    unsafe { std::env::set_var("PATH", merged.join(":")) };
}

#[cfg(windows)]
pub fn import_login_path() {}

/// 원본 `Process.Start(new ProcessStartInfo(path) { UseShellExecute = true })`. 폴더·파일·URL 모두 기본 앱으로 연다.
pub fn open(target: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "macos")]
    command.arg(target);
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("explorer.exe");
        command.arg(target).creation_flags(0x0800_0000);
        command
    };
    command.spawn().map(drop)
}

/// 맥 권한 세 가지와 설정 화면 URL. Windows는 권한 안내가 없다.
#[cfg(target_os = "macos")]
pub fn permissions() -> Value {
    use kakaorelay_macos::{DataAccess, permission_status};
    let status = permission_status();
    let pane = "x-apple.systempreferences:com.apple.preference.security?";
    json!({
        "supported": true,
        "items": [
            {
                "name": "전체 디스크 접근",
                "granted": status.data_access == DataAccess::Allowed,
                "detail": match status.data_access {
                    DataAccess::Allowed => "허용됨 · 카카오톡 대화를 읽을 수 있습니다.",
                    DataAccess::Denied => "필요 · 카카오톡 대화 읽기에 필요합니다.",
                    DataAccess::Missing => "카카오톡 데이터 없음 · 카카오톡에 로그인했는지 확인하세요.",
                },
                "url": format!("{pane}Privacy_AllFiles"),
            },
            {
                "name": "손쉬운 사용",
                "granted": status.accessibility,
                "detail": if status.accessibility { "허용됨" } else { "필요 · 열린 대화방 확인과 발송에 필요합니다." },
                "url": format!("{pane}Privacy_Accessibility"),
            },
            {
                "name": "화면 기록",
                "granted": status.screen_recording,
                "detail": if status.screen_recording { "허용됨" } else { "필요 · 발송 대상 창 확인에 필요합니다(발송 필수)." },
                "url": format!("{pane}Privacy_ScreenCapture"),
            },
            {
                // macOS 15+는 다른 앱 컨테이너를 처음 읽을 때 별도 대화상자로 묻는다. 상태를 따로 조회하는 공개 API가 없어
                // 카카오톡 데이터 읽기 결과로 판단한다. 시스템 설정에 별도 항목이 없으므로 거절했다면 전체 디스크 접근으로 허용한다.
                "name": "다른 앱 데이터",
                "granted": status.data_access == DataAccess::Allowed,
                "detail": match status.data_access {
                    DataAccess::Allowed => "허용됨",
                    _ => "필요 · '다른 앱의 데이터에 접근' 확인 창에서 허용하세요. 거절했다면 전체 디스크 접근을 허용하세요.",
                },
                "url": format!("{pane}Privacy_AllFiles"),
            },
        ],
        "problems": status.problems(),
    })
}

#[cfg(windows)]
pub fn permissions() -> Value {
    json!({ "supported": false, "items": [], "problems": [] })
}

/// 원본 `DiagnosticCollector.Collect`. 원본이 저장하던 시점마다 보고서를 저장한다.
#[cfg(windows)]
pub fn collect_diagnostics(path: &Path) -> DiagnosticReport {
    use kakaorelay_core::diagnostics::report_store;
    let mut report = DiagnosticReport::default();
    let apply = |report: &mut DiagnosticReport,
                 p: &kakaorelay_windows::diagnostics::DiagnosticProgress| {
        report.session_id = p.session_id;
        report.status = p.status.clone();
        report.stage = p.stage.clone();
        report.foreground_window_before = p.foreground_window_before.clone();
        report.foreground_window_after = p.foreground_window_after.clone();
        report.processes = p.processes.clone();
        report.windows = p.windows.clone();
        report.warnings = p.warnings.clone();
    };
    let progress = kakaorelay_windows::diagnostics::collect(&mut |p| {
        apply(&mut report, p);
        let _ = report_store::save(path, &report);
    });
    apply(&mut report, &progress);
    report.finished_at = Some(chrono::Local::now().fixed_offset());
    let _ = report_store::save(path, &report);
    report
}

/// 맥 진단: 열린 대화창 목록과 권한 상태만 기록한다. 맥 카카오톡은 입력창을 AX에 노출하지 않아 컨트롤 트리는 없다.
#[cfg(target_os = "macos")]
pub fn collect_diagnostics(path: &Path) -> DiagnosticReport {
    use kakaorelay_core::diagnostics::{WindowSnapshot, report_store};
    use kakaorelay_core::platform::RoomCatalog;
    let mut report = DiagnosticReport {
        stage: "windows".into(),
        ..Default::default()
    };
    let _ = report_store::save(path, &report);
    let status = kakaorelay_macos::permission_status();
    report
        .warnings
        .extend(status.problems().into_iter().map(String::from));
    match kakaorelay_macos::MacRoomCatalog.scan() {
        Ok(targets) => {
            report.windows = targets
                .into_iter()
                .map(|t| WindowSnapshot {
                    handle: t.handle,
                    process_id: t.process_id,
                    title: t.title,
                    class_name: "KakaoTalk".into(),
                    visible: true,
                    enabled: true,
                    automation_status: if t.ambiguous {
                        "ambiguous".into()
                    } else {
                        "window-menu".into()
                    },
                    ..Default::default()
                })
                .collect();
            report.status = if status.problems().is_empty() {
                "complete"
            } else {
                "partial"
            }
            .into();
        }
        Err(error) => {
            report.status = "error".into();
            report.warnings.push(format!("진단 오류: {error}"));
        }
    }
    report.stage = "finished".into();
    report.finished_at = Some(chrono::Local::now().fixed_offset());
    let _ = report_store::save(path, &report);
    report
}

pub fn version_text() -> String {
    format!("v{} · KakaoRelay", env!("CARGO_PKG_VERSION"))
}
