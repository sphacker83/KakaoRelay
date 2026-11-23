//! 원본: `ProbeRunner.cs`. 진단을 별도 worker 프로세스(`<앱> --worker <경로>`)로 돌리고, 시간 초과·취소 시 그 프로세스 트리만 끝낸다.
//! 카카오톡 프로세스는 건드리지 않는다. 결과는 항상 보고서로 돌려준다(실패도 상태로 기록한다).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use kakaorelay_ai::Error;
use kakaorelay_ai::cancel::CancellationToken;

use crate::diagnostics::{DiagnosticReport, report_store};
use crate::fs::full_path;
use crate::support::error_kind;
use kakaorelay_ai::settings::path_with_suffix;

fn load_partial(path: &Path) -> DiagnosticReport {
    match report_store::load(path) {
        Ok(report) => report.unwrap_or_default(),
        Err(_) => DiagnosticReport {
            warnings: vec!["부분 진단 파일을 읽을 수 없습니다.".into()],
            ..Default::default()
        },
    }
}

fn finish(path: &Path, mut report: DiagnosticReport) -> DiagnosticReport {
    report
        .finished_at
        .get_or_insert_with(|| chrono::Local::now().fixed_offset());
    for window in report
        .windows
        .iter_mut()
        .filter(|w| w.automation_status == "running")
    {
        window.automation_status = "interrupted".into();
    }
    // 원본은 저장 실패를 호출자에게 던진다. 여기서는 보고서를 돌려주고 경고로 남긴다.
    if let Err(error) = report_store::save(path, &report) {
        report
            .warnings
            .push(format!("진단 보고서 저장 실패: {}", error_kind(&error)));
    }
    report
}

/// 원본 `Process.Kill(entireProcessTree: true)`. worker는 자체 프로세스 그룹(유닉스)으로 띄운다.
async fn kill_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        #[cfg(windows)]
        let status = tokio::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        #[cfg(unix)]
        let status = tokio::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        if status.is_ok_and(|s| s.success()) {
            let _ = child.wait().await;
            return;
        }
    }
    let _ = child.kill().await;
}

pub async fn run(
    executable: &Path,
    output_path: &Path,
    timeout: Duration,
    cancel: &CancellationToken,
) -> DiagnosticReport {
    let output_path = full_path(output_path);
    // 실행마다 다른 worker 파일을 써서 이전 보고서를 이번 결과로 착각하지 않는다.
    let worker_path: PathBuf = path_with_suffix(
        &output_path,
        &format!(".{}.worker.json", uuid::Uuid::new_v4().simple()),
    );
    let report = execute(executable, &output_path, &worker_path, timeout, cancel).await;
    for temporary in [worker_path.clone(), path_with_suffix(&worker_path, ".tmp")] {
        let _ = std::fs::remove_file(temporary);
    }
    report
}

async fn execute(
    executable: &Path,
    output_path: &Path,
    worker_path: &Path,
    timeout: Duration,
    cancel: &CancellationToken,
) -> DiagnosticReport {
    let failed = |error: Error| {
        let mut report = load_partial(worker_path);
        report.status = if matches!(error, Error::Cancelled) {
            "cancelled"
        } else {
            "error"
        }
        .into();
        report
            .warnings
            .push(format!("진단 실행 오류: {}", error_kind(&error)));
        finish(output_path, report)
    };
    if let Some(parent) = output_path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return failed(error.into());
    }
    if cancel.is_cancelled() {
        return failed(Error::Cancelled);
    }
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("--worker")
        .arg(worker_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return failed(error.into()),
    };
    let exited = tokio::select! {
        status = child.wait() => Some(status),
        () = cancel.cancelled() => None,
        () = tokio::time::sleep(timeout) => None,
    };
    let status = match exited {
        Some(Ok(status)) => status,
        Some(Err(error)) => return failed(error.into()),
        None => {
            kill_tree(&mut child).await;
            let mut report = load_partial(worker_path);
            report.status = if cancel.is_cancelled() {
                "cancelled"
            } else {
                "timeout"
            }
            .into();
            report.warnings.push(
                "응답이 없는 진단 작업만 종료했습니다. 카카오톡은 종료하지 않았습니다.".into(),
            );
            return finish(output_path, report);
        }
    };
    let mut report = load_partial(worker_path);
    if !status.success() || report.status == "running" {
        report.status = "error".into();
        // 원본 `Process.ExitCode`. 신호로 끝난 유닉스 프로세스는 코드가 없어 -1로 적는다.
        report.warnings.push(format!(
            "진단 프로세스 종료 코드: {}",
            status.code().unwrap_or(-1)
        ));
    }
    finish(output_path, report)
}
