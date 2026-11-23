//! 실제 CLI를 호출하는 수동 검사(원본 `--live`, `--ai-smoke`). 자동 테스트에서는 실행하지 않는다.
//! 실행: `cargo test -p kakaorelay-ai --test live -- --ignored --nocapture`

use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::cli_runner::{AiRunner, CliAiRunner};
use kakaorelay_ai::model_catalog;
use kakaorelay_ai::settings::{AiProviderSettings, ORDER};

/// 원본 `KakaoRelay.ModelCatalog.Tests --live`: 설치된 CLI마다 실제 모델 목록을 출력한다.
#[tokio::test]
#[ignore = "실제 CLI와 로그인 상태가 필요하다"]
async fn live_model_catalog() {
    for id in ORDER {
        let catalog = model_catalog::query(&AiProviderSettings::new(id), &CancellationToken::new())
            .await
            .unwrap();
        let models: Vec<&str> = catalog.models.iter().map(|m| m.id.as_str()).collect();
        println!(
            "{}",
            serde_json::json!({"provider": id, "models": models, "LoginRequired": catalog.login_required})
        );
    }
}

/// 원본 `KakaoRelay.Tests --ai-smoke`: 실제 Codex CLI로 짧은 응답을 받는다.
#[tokio::test]
#[ignore = "실제 Codex CLI와 로그인 상태가 필요하다"]
async fn ai_smoke() {
    let runner = CliAiRunner::new();
    let answer = runner
        .run(
            &AiProviderSettings::new("codex"),
            "외부 도구를 사용하지 말고 정확히 '연결 확인'만 답하세요.",
            90,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    println!("{answer}");
}
