//! 원본: `ChatToolChecks.cs`. 자동 답장 발송 순서 검사 7개(`AutoReplySession`·`ImageSender`)는 `kakaorelay-core/tests/checks/chat_tool.rs`에 있다.
//! "Claude 이미지 요청" 검사의 `ImageSender.ValidateLocalImage`는 Core 소관이라 파일 존재와 PNG 서명 확인으로 대신한다.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use kakaorelay_ai::ai_service::{AiCommand, AiService};
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::chat_image::{
    ChatImageGenerator, ChatImageRequest, CliChatImageGenerator, Progress,
};
use kakaorelay_ai::cli_runner::CliAiRunner;
use kakaorelay_ai::image_cli::ImageCliRunner;
use kakaorelay_ai::imported_reader::ImportedChatReader;
use kakaorelay_ai::settings::{AiProviderSettings, AiSettings, AiSettingsStore};
use kakaorelay_ai::{Error, Result};

use crate::fakes::Runner;
use crate::image_cli::png;
use crate::support::Check;

const DIRECTIVE: &str = "[[generate_image:\"달 위의 한복 고양이\"]]";

#[derive(Default)]
struct Images {
    calls: Mutex<usize>,
    prompt: Mutex<String>,
    fail: Mutex<bool>,
}

#[async_trait]
impl ChatImageGenerator for Images {
    async fn generate(
        &self,
        _settings: &AiSettings,
        _reply: &str,
        prompt: &str,
        _cancel: &CancellationToken,
        _progress: Progress<'_>,
    ) -> Result<PathBuf> {
        *self.calls.lock().unwrap() += 1;
        *self.prompt.lock().unwrap() = prompt.into();
        if *self.fail.lock().unwrap() {
            return Err(Error::invalid_data("fixture failure"));
        }
        Ok(std::env::current_dir().unwrap().join("generated.png"))
    }
}

#[derive(Default)]
struct ImageCli {
    calls: Mutex<Vec<String>>,
    fail_all: Mutex<bool>,
}

#[async_trait]
impl ImageCliRunner for ImageCli {
    async fn run(
        &self,
        provider: &AiProviderSettings,
        folder: &Path,
        _prompt: &str,
        _reference: Option<&Path>,
        _cancel: &CancellationToken,
    ) -> Result<String> {
        self.calls.lock().unwrap().push(provider.id.clone());
        if provider.id == "codex" || *self.fail_all.lock().unwrap() {
            return Err(Error::invalid_data("fixture unavailable"));
        }
        let path = folder.join("generated.png");
        std::fs::write(&path, png())?;
        Ok(serde_json::json!({"image_path": path.to_string_lossy()}).to_string())
    }
}

pub async fn run(root: &Path, check: &Check) {
    let none = CancellationToken::new();
    let reader = Arc::new(ImportedChatReader::new());
    let room = reader.import("image-request.txt", "--------------- 2026년 9월 29일 ---------------\n[테스트] [오후 1:00] 달 위의 한복 고양이 그려줘").unwrap();
    let store = Arc::new(AiSettingsStore::new(root.join("chat-image-settings.json")));
    let runner = Arc::new(Runner::new(&format!(
        "달 위의 한복 고양이를 수채화로 그려볼게요.\n{DIRECTIVE}\n[[emotion:happy]]"
    )));
    let images = Arc::new(Images::default());
    let service = AiService::new(
        reader.clone(),
        store.clone(),
        runner.clone(),
        Some(images.clone()),
        None,
    );
    let reply = |target: Option<&str>| AiCommand {
        target_message_id: target.map(Into::into),
        ..AiCommand::new(&room.profile, &room.id, "reply")
    };
    let result = service
        .generate(&reply(Some("123")), &none, None, false, false, None)
        .await
        .unwrap();
    check.check(
        result.image_path.is_some() && result.attachment.is_none() && result.emotion == "neutral" && !result.text.contains("[[") && *images.prompt.lock().unwrap() == "달 위의 한복 고양이",
        "Reply image directive invokes generation and returns a generated attachment without exposing control markers",
    );
    let prompt = runner.prompt();
    check.check(
        prompt.contains("웹검색") && prompt.contains("출처 URL") && prompt.contains("123"),
        "Chat instructions permit sourced web answers and identify the current target message",
    );
    runner.set_text("평범한 답변\n[[emotion:neutral]]".into());
    let ordinary = service
        .generate(&reply(None), &none, None, false, false, None)
        .await
        .unwrap();
    check.check(
        *images.calls.lock().unwrap() == 1 && ordinary.image_path.is_none(),
        "Ordinary replies never invoke the image generator",
    );
    runner.set_text(format!("만들었어요.\n{DIRECTIVE}\n[[emotion:neutral]]"));
    *images.fail.lock().unwrap() = true;
    let failed = service
        .generate(&reply(None), &none, None, false, false, None)
        .await
        .unwrap();
    check.check(
        failed.image_path.is_none() && failed.attachment.is_none() && failed.image_error.is_some() && failed.text.contains("생성하지 못"),
        "Failed generation replaces success claims with a truthful failure reply and no emotion image",
    );
    for malformed in [
        "[[generate_image:/secret.png]]".to_string(),
        format!("{DIRECTIVE}\n{DIRECTIVE}"),
        "[[generate_image:\"\"]]".into(),
        format!("{DIRECTIVE}\nextra"),
    ] {
        check.check(
            matches!(
                ChatImageRequest::parse(&malformed),
                Err(Error::Json(_) | Error::InvalidData(_))
            ),
            "Malformed image directives cannot become file attachments",
        );
    }
    let settings = store.load().unwrap();
    let cli = Arc::new(ImageCli::default());
    let generator = CliChatImageGenerator::new(
        Arc::new(|b: &[u8]| Ok(b.to_vec())),
        Some(cli.clone()),
        Some(root.join("generated-replies")),
    );
    let image_path = generator
        .generate(&settings, "claude", "고양이", &none, None)
        .await
        .unwrap();
    check.check(
        *cli.calls.lock().unwrap() == ["codex", "grok"] && std::fs::read(&image_path).is_ok_and(|b| b.starts_with(&[137, 80, 78, 71])),
        "Claude image requests use Codex then Grok fallback and retain validated output after temporary job cleanup",
    );
    cli.calls.lock().unwrap().clear();
    *cli.fail_all.lock().unwrap() = true;
    let exhausted = generator
        .generate(&settings, "grok", "고양이", &none, None)
        .await;
    check.check(
        matches!(exhausted, Err(Error::InvalidOperation(_)))
            && *cli.calls.lock().unwrap() == ["grok", "codex", "grok", "codex"],
        "Image fallback stops after two rounds",
    );
    *images.fail.lock().unwrap() = false;
    let before_plan = *images.calls.lock().unwrap();
    runner.set_text(format!(
        "달 위의 한복 고양이를 수채화로 그려볼게요.\n{DIRECTIVE}\n[[emotion:neutral]]"
    ));
    let plan = service
        .generate(&reply(None), &none, None, true, false, None)
        .await
        .unwrap();
    check.check(
        *images.calls.lock().unwrap() == before_plan
            && plan.image_path.is_none()
            && plan.image_prompt.as_deref() == Some("달 위의 한복 고양이")
            && plan.text.contains("그려볼게요"),
        "Automatic reply preparation returns the drawing plan without starting image generation",
    );
    for provider in ["codex", "grok", "claude"] {
        let args = CliAiRunner::arguments(&AiProviderSettings::new(provider), root).unwrap();
        let has = |x: &str| args.iter().any(|a| a == x);
        let ok = match provider {
            "codex" => has("web_search=\"live\""),
            "grok" => {
                has("web_search,web_fetch")
                    && !has("--disable-web-search")
                    && args[args.iter().position(|a| a == "--max-turns").unwrap() + 1] == "8"
            }
            _ => has("WebSearch,WebFetch") && has("--allowedTools"),
        };
        check.check(ok, &format!("{provider} CLI enables web tools"));
    }
}
