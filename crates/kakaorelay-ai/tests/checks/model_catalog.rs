//! 원본: `tests/KakaoRelay.ModelCatalog.Tests/Program.cs`. WPF 바인딩 검사 3개는 같은 의미의 설정 동작 검사로 옮겼다.

use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::model_catalog::{self, CliModel, CliModelCatalog};
use kakaorelay_ai::settings::{AiProviderSettings, ORDER};
use kakaorelay_ai::{Error, json};

use crate::support::{Check, fake_exe};

pub async fn run(check: &Check) {
    let none = CancellationToken::new();
    for id in ORDER {
        let catalog = model_catalog::query(
            &AiProviderSettings {
                executable: fake_exe(),
                ..AiProviderSettings::new(id)
            },
            &none,
        )
        .await
        .unwrap();
        check.check(
            catalog.models.len() == if id == "claude" { 1 } else { 2 },
            &format!("{id} metadata protocol and pagination"),
        );
        if id == "grok" {
            check.check(
                catalog.login_required,
                "Grok unauthenticated catalog is labeled",
            );
        }
        if id == "claude" {
            check.check(
                catalog.models[0].id == "alias[1m]",
                "CLI aliases with context suffix remain valid",
            );
        }
    }
    let missing = model_catalog::query(
        &AiProviderSettings {
            executable: "C:/missing-fixture.exe".into(),
            ..AiProviderSettings::new("codex")
        },
        &none,
    )
    .await;
    check.check(
        matches!(missing, Err(Error::InvalidOperation(_))),
        "missing CLI has no static fallback",
    );
    let mut provider = AiProviderSettings {
        model: "saved-model".into(),
        ..AiProviderSettings::default()
    };
    let loaded = CliModelCatalog {
        models: vec![CliModel {
            id: "installed-model".into(),
            label: "Fixture".into(),
            is_default: false,
        }],
        checked_at: chrono::Utc::now(),
        login_required: false,
    };
    provider.set_model_catalog(Some(&loaded), "loaded");
    check.check(
        provider.model == "saved-model"
            && provider
                .model_choices()
                .contains(&"saved-model".to_string()),
        "Catalog refresh preserves saved selection",
    );
    provider.set_model_catalog(None, "failed");
    check.check(
        provider.model == "saved-model"
            && !provider
                .model_choices()
                .contains(&"installed-model".to_string()),
        "failed refresh clears catalog and preserves selection",
    );
    check.check(
        !json::default_pascal(&provider).contains("ModelChoices")
            && !json::default_pascal(&provider).contains("installed"),
        "runtime catalog is not persisted as static settings",
    );
}
