//! 원본: `Program.cs`의 진단 검사 12개와 `--worker` 모드.

use std::path::Path;
use std::time::{Duration, Instant};

use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_core::diagnostics::{
    AutomationControlSnapshot, DiagnosticReport, ProcessSnapshot, WindowSnapshot, report_store,
};
use kakaorelay_core::probe;

use crate::support::Check;

fn process(id: i32, version: &str) -> ProcessSnapshot {
    ProcessSnapshot {
        id,
        session_id: 1,
        version: version.into(),
    }
}

/// 원본 worker: 부분 보고서를 쓰고, 이름에 따라 멈추거나(hang-) 실패하거나(crash-) 완료한다.
pub fn worker(path: &str) -> i32 {
    let mut partial = DiagnosticReport {
        stage: "uia:test".into(),
        processes: vec![process(1234, "test")],
        windows: vec![WindowSnapshot {
            handle: "0x1234".into(),
            automation_status: "running".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    report_store::save(Path::new(path), &partial).unwrap();
    if path.contains("hang-") {
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
    if path.contains("crash-") {
        return 7;
    }
    partial.status = "complete".into();
    partial.windows[0].automation_status = "complete".into();
    report_store::save(Path::new(path), &partial).unwrap();
    0
}

pub async fn run(root: &Path, check: &Check) {
    let mut exposed = DiagnosticReport {
        status: "complete".into(),
        processes: vec![process(1, "test")],
        windows: vec![WindowSnapshot {
            controls: vec![AutomationControlSnapshot {
                control_type: "Edit".into(),
                value_available: true,
                value_read_only: Some(false),
                invoke_available: true,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    check.check(
        exposed.input_candidate_count() == 1 && exposed.summary().contains("검증"),
        "Exposed controls remain unverified, not send-ready",
    );
    check.check(
        !exposed.background_input_tested
            && !exposed.sending_tested
            && exposed.read_only
            && !exposed.message_contents_collected,
        "Read-only diagnosis never claims an input or send test",
    );
    exposed.windows[0].controls[0].password = true;
    check.check(
        exposed.input_candidate_count() == 0,
        "Password controls are excluded from input candidates",
    );
    let path = root.join("report.json");
    report_store::save(&path, &exposed).unwrap();
    let restored = report_store::load(&path).unwrap().unwrap();
    check.check(
        restored.windows.len() == 1 && !root.join("report.json.tmp").exists(),
        "Atomic JSON report survives a round trip",
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let control = &parsed["windows"][0]["controls"][0];
    check.check(
        control.get("name").is_none()
            && control.get("value").is_none()
            && control.get("text").is_none(),
        "Control schema contains no message text fields",
    );
    let executable = std::env::current_exe().unwrap();
    let none = CancellationToken::new();
    let completed = probe::run(
        &executable,
        &root.join("complete.json"),
        Duration::from_secs(5),
        &none,
    )
    .await;
    check.check(
        completed.status == "complete",
        "Coordinator receives a successful worker result",
    );
    let timer = Instant::now();
    let timed_out = probe::run(
        &executable,
        &root.join("hang-timeout.json"),
        Duration::from_secs(3),
        &none,
    )
    .await;
    check.check(
        timed_out.status == "timeout"
            && timed_out.windows.len() == 1
            && timed_out.windows[0].automation_status == "interrupted"
            && timer.elapsed() < Duration::from_secs(8),
        "Hung worker is terminated while partial evidence is preserved",
    );
    let cancellation = CancellationToken::new();
    let canceller = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        canceller.cancel();
    });
    let cancelled = probe::run(
        &executable,
        &root.join("hang-cancel.json"),
        Duration::from_secs(15),
        &cancellation,
    )
    .await;
    check.check(
        cancelled.status == "cancelled",
        "User cancellation differs from timeout",
    );
    let crashed = probe::run(
        &executable,
        &root.join("crash-worker.json"),
        Duration::from_secs(5),
        &none,
    )
    .await;
    check.check(
        crashed.status == "error" && crashed.windows.len() == 1,
        "Worker failure preserves partial data and is not reported as success",
    );
    let stale = root.join("stale.json");
    report_store::save(
        &stale,
        &DiagnosticReport {
            status: "complete".into(),
            processes: vec![process(7777, "old")],
            ..Default::default()
        },
    )
    .unwrap();
    let missing = probe::run(
        &root.join("missing.exe"),
        &stale,
        Duration::from_secs(1),
        &none,
    )
    .await;
    check.check(
        missing.status == "error" && missing.processes.is_empty(),
        "Previous success is not reused when a new worker cannot start",
    );
    let leftovers = std::fs::read_dir(root)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".worker.json")
        })
        .count();
    check.check(leftovers == 0, "Worker scratch files are cleaned up");
}
