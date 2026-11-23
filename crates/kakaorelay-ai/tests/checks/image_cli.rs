//! 원본: `ImageCliChecks.cs`.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;
use base64::Engine;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::image_cli::{
    GPT_MODEL, GROK_MODEL, ImageCliRunner, NativeImageCliRunner, PersonaImageCliGenerator,
};
use kakaorelay_ai::persona_images::PersonaImageGenerator;
use kakaorelay_ai::settings::{AiProviderSettings, AiSettings, AiSettingsStore};
use kakaorelay_ai::{Error, Result};

use crate::support::{Check, fake_exe};

pub fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aG1sAAAAASUVORK5CYII=").unwrap()
}

#[derive(Default)]
struct State {
    reference: Option<Vec<u8>>,
    prompt: String,
    folder: std::path::PathBuf,
    return_reference: bool,
    fail: bool,
}

#[derive(Default)]
struct Runner(Mutex<State>);

#[async_trait]
impl ImageCliRunner for Runner {
    async fn run(
        &self,
        _provider: &AiProviderSettings,
        folder: &Path,
        prompt: &str,
        reference: Option<&Path>,
        _cancel: &CancellationToken,
    ) -> Result<String> {
        let mut state = self.0.lock().unwrap();
        state.prompt = prompt.into();
        state.folder = folder.into();
        state.reference = reference.map(|r| std::fs::read(r).unwrap());
        if state.fail {
            return Ok("{\"error\":\"no image tool\"}".into());
        }
        let output = folder.join("new.png");
        std::fs::write(&output, png()).unwrap();
        let path = if state.return_reference {
            reference.map(|r| r.to_path_buf())
        } else {
            Some(output)
        };
        Ok(
            serde_json::json!({"image_path": path.map(|p| p.to_string_lossy().into_owned())})
                .to_string(),
        )
    }
}

pub async fn run(root: &Path, check: &Check) {
    let none = CancellationToken::new();
    let mut settings = AiSettings::default();
    settings.image_gpt_model = "custom-gpt".into();
    settings.image_grok_model = "custom-grok".into();
    let store = AiSettingsStore::new(root.join("image-cli-settings.json"));
    store.save(&mut settings).unwrap();
    let loaded = store.load().unwrap();
    check.check(
        loaded.image_gpt_model == "custom-gpt" && loaded.image_grok_model == "custom-grok",
        "Image CLI model settings survive restart",
    );
    check.check(
        NativeImageCliRunner::extract_image_result(
            "I'll edit the image.\n{\"image_path\":\"/generated/result.png\"}\nDone.",
        )
        .unwrap()
        .contains("/generated/result.png"),
        "Image CLI extracts the final result despite progress commentary",
    );
    for id in ["codex", "grok"] {
        let provider = AiProviderSettings {
            model: if id == "codex" { GPT_MODEL } else { GROK_MODEL }.into(),
            executable: fake_exe(),
            ..AiProviderSettings::new(id)
        };
        let runner = Arc::new(Runner::default());
        let generator = PersonaImageCliGenerator::new(
            provider.clone(),
            Arc::new(|b: &[u8]| Ok(b.to_vec())),
            Some(runner.clone()),
        );
        let output = generator
            .generate("한국어 style & $(literal)", Some(&png()), &none)
            .await
            .unwrap();
        {
            let state = runner.0.lock().unwrap();
            check.check(
                output == png()
                    && state.reference.as_deref() == Some(&png()[..])
                    && !state.folder.exists(),
                &format!(
                    "{id} image edit uses exact reference bytes and cleans the request directory"
                ),
            );
            check.check(
                state.prompt.contains("Python/API fallback")
                    && state.prompt.contains("Do not use API keys")
                    && state.prompt.contains(if id == "codex" {
                        "imagegen skill"
                    } else {
                        "image_gen / image_edit"
                    }),
                &format!("{id} image job requires the native image tool and forbids API fallback"),
            );
        }
        let info =
            NativeImageCliRunner::command(&provider, root, Some(&root.join("reference.png")))
                .unwrap();
        check.check(
            !info.has_env("OPENAI_API_KEY")
                && !info.has_env("XAI_API_KEY")
                && !info.has_env("GROK_API_KEY"),
            &format!("{id} image CLI hides its window and excludes API-key environment variables"),
        );
        let has = |x: &str| info.args.iter().any(|a| a == x);
        check.check(
            if id == "codex" {
                has("--image") && has("features.shell_tool=false") && has("image_generation")
            } else {
                has("image_gen,image_edit") && has("dontAsk")
            },
            &format!("{id} native tool and reference options are separate CLI arguments"),
        );
        runner.0.lock().unwrap().return_reference = true;
        let reused = generator.generate("edit", Some(&png()), &none).await;
        check.check(
            matches!(reused, Err(Error::InvalidData(_))),
            &format!("{id} cannot reuse the unchanged reference as a generated image"),
        );
        {
            let mut state = runner.0.lock().unwrap();
            state.return_reference = false;
            state.fail = true;
        }
        let missing = generator.generate("generate", None, &none).await;
        check.check(
            matches!(missing, Err(Error::InvalidData(_))),
            &format!("{id} missing native tool result fails without API fallback"),
        );
    }
    let outside = root.join("outside.png");
    std::fs::write(&outside, png()).unwrap();
    let rejected = PersonaImageCliGenerator::validate_output(
        &outside.to_string_lossy(),
        &root.join("job"),
        None,
        SystemTime::now(),
    );
    check.check(
        matches!(rejected, Err(Error::InvalidData(_))),
        "CLI cannot import arbitrary files outside generated-image roots",
    );
    let native = NativeImageCliRunner;
    let fixture = AiProviderSettings {
        model: "fixture".into(),
        executable: fake_exe(),
        ..AiProviderSettings::new("codex")
    };
    let literal = "{\"image_path\":\"한글 & $(literal)\"}";
    let reply = native
        .run(&fixture, root, literal, None, &none)
        .await
        .unwrap();
    check.check(
        reply == literal,
        "Native image CLI sends Unicode and metacharacters through stdin literally",
    );
    let stop = CancellationToken::new();
    let stopper = stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        stopper.cancel();
    });
    check.check(
        native.run(&fixture, root, "HANG", None, &stop).await == Err(Error::Cancelled),
        "Cancelling image generation terminates its CLI process",
    );
}
