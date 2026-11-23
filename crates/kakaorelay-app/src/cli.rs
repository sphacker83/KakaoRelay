//! 명령줄 모드. 원본 `App.OnStartup`의 `--send-image`, `--worker`, `--diagnose` 분기.
//! 실패는 원본처럼 종료 코드 2로만 알린다(본문·대상 정보를 출력하지 않는다).

use std::path::Path;
use std::time::Duration;

use kakaorelay_ai::Error;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::settings::AiSettingsStore;
use kakaorelay_core::api_send::{ApiSendCommand, ApiSendService};
use kakaorelay_core::test_send::default_ledger;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio 런타임")
}

/// 저장된 설정의 '내 카카오 ID'. 설정이 없거나 읽지 못하면 빈 값(맥 어댑터가 다른 방법으로 찾는다).
pub fn saved_self_id() -> String {
    AiSettingsStore::new(AiSettingsStore::default_path())
        .load()
        .map(|s| s.self_id)
        .unwrap_or_default()
}

pub fn send_image(request: &str, output: &str) -> i32 {
    let outcome = runtime().block_on(async {
        let text = std::fs::read_to_string(request)?;
        let command = ApiSendCommand::from_json(serde_json::from_str(&text)?)?;
        if !command.is_image() {
            return Err(Error::invalid_data("이미지 요청만 지원합니다."));
        }
        let sender =
            ApiSendService::from_platform(default_ledger(), crate::os::platform(&saved_self_id()));
        let receipt = sender.send(command).await?;
        std::fs::write(output, receipt.to_json())?;
        Ok(receipt.enter_posted && receipt.input_cleared == Some(true))
    });
    match outcome {
        Ok(true) => 0,
        _ => 2,
    }
}

pub fn worker(path: &str) -> i32 {
    let report = crate::os::collect_diagnostics(Path::new(path));
    if report.status == "error" { 2 } else { 0 }
}

pub fn diagnose(path: &str) -> i32 {
    let Ok(executable) = std::env::current_exe() else {
        return 2;
    };
    let report = runtime().block_on(kakaorelay_core::probe::run(
        &executable,
        Path::new(path),
        Duration::from_secs(20),
        &CancellationToken::new(),
    ));
    if matches!(report.status.as_str(), "complete" | "partial") {
        0
    } else {
        2
    }
}
