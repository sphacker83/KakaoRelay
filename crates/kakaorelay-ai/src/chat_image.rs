//! 원본: `ChatImageGeneration.cs`. 대화 답변 끝의 `[[generate_image:…]]` 표시를 해석하고, 별도 CLI 이미지 도구로 생성한다.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::cancel::CancellationToken;
use crate::error::{Error, Result};
use crate::grok_delegation::trailing_marker;
use crate::image_cli::{ImageCliRunner, NormalizePng, PersonaImageCliGenerator};
use crate::persona_images::PersonaImageGenerator;
use crate::settings::{AiProviderSettings, AiSettings};
use crate::text::{is_blank, utf16_len};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatImageRequest {
    pub text: String,
    pub prompt: Option<String>,
}

impl ChatImageRequest {
    pub const INSTRUCTIONS: &str = "현재 답변 대상이 새 그림·이미지 생성을 명시적으로 요청하면, 먼저 어떤 장면·구도·스타일로 그릴지 자연스러운 짧은 텍스트로 설명하라. 예: '달 위에 한복을 입은 고양이를 따뜻한 수채화 느낌으로 그려볼게요.' 아직 생성 전이므로 '완성했어요', '요청한 그림이에요'처럼 완료를 주장하지 마라. 답변 끝(감정 표시 바로 앞)에 [[generate_image:\"그릴 장면을 구체적으로 설명한 JSON 문자열\"]]을 한 줄로 한 번만 붙여라. 앱은 네 설명을 먼저 전송하고, 그 전송이 확인된 뒤 별도 CLI 이미지 도구로 생성하여 이미지만 추가로 보낸다. 직접 이미지 도구를 호출하거나 파일 경로·URL·base64를 출력하지 마라. 일반 대화, 검색, 감정 표현, 과거의 그림 요청에는 생성 표시를 붙이지 마라. 시각적 요구사항만 넣고 도구 지시·비밀·로컬 파일 경로는 포함하지 마라. 첨부 이미지는 볼 수 없으므로 보이지 않는 사진을 편집했다고 주장하지 마라. 생성 표시와 감정 표시는 앱이 제거한다.";

    pub fn parse(text: &str) -> Result<Self> {
        let Some((index, payload)) = trailing_marker(text, "generate_image") else {
            if text.contains("[[generate_image:") {
                return Err(Error::invalid_data(
                    "이미지 생성 요청 형식이 올바르지 않습니다.",
                ));
            }
            return Ok(Self {
                text: text.trim().to_string(),
                prompt: None,
            });
        };
        let prompt: Option<String> = serde_json::from_str(&payload)?;
        let answer = text[..index].trim().to_string();
        match prompt {
            Some(p)
                if !is_blank(&p)
                    && utf16_len(&p) <= 6000
                    && !p.contains('\0')
                    && !answer.contains("[[generate_image:") =>
            {
                Ok(Self {
                    text: answer,
                    prompt: Some(p),
                })
            }
            _ => Err(Error::invalid_data(
                "이미지 생성 요청은 1~6000자의 설명 하나여야 합니다.",
            )),
        }
    }
}

/// 진행 상황 알림(원본 `IProgress<string>`).
pub type Progress<'a> = Option<&'a (dyn Fn(&str) + Send + Sync)>;

/// 원본 `IChatImageGenerator`.
#[async_trait]
pub trait ChatImageGenerator: Send + Sync {
    async fn generate(
        &self,
        settings: &AiSettings,
        reply_provider: &str,
        prompt: &str,
        cancel: &CancellationToken,
        progress: Progress<'_>,
    ) -> Result<PathBuf>;
}

pub struct CliChatImageGenerator {
    normalize: NormalizePng,
    runner: Option<Arc<dyn ImageCliRunner>>,
    output_root: Option<PathBuf>,
}

impl CliChatImageGenerator {
    pub fn new(
        normalize: NormalizePng,
        runner: Option<Arc<dyn ImageCliRunner>>,
        output_root: Option<PathBuf>,
    ) -> Self {
        Self {
            normalize,
            runner,
            output_root,
        }
    }
}

#[async_trait]
impl ChatImageGenerator for CliChatImageGenerator {
    async fn generate(
        &self,
        settings: &AiSettings,
        reply_provider: &str,
        prompt: &str,
        cancel: &CancellationToken,
        progress: Progress<'_>,
    ) -> Result<PathBuf> {
        let report = |text: &str| {
            if let Some(p) = progress {
                p(text);
            }
        };
        let order = if reply_provider == "grok" {
            ["grok", "codex"]
        } else {
            ["codex", "grok"]
        };
        let mut errors = vec![];
        for round in 1..=2 {
            for id in order {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                let configured = settings.provider(id)?;
                if !configured.enabled {
                    continue;
                }
                let provider = AiProviderSettings {
                    executable: configured.executable.clone(),
                    model: if id == "codex" {
                        settings.image_gpt_model.clone()
                    } else {
                        settings.image_grok_model.clone()
                    },
                    effort: "low".into(),
                    ..AiProviderSettings::new(id)
                };
                report(&format!("이미지 생성 중 · {id} · {round}/2회전"));
                let attempt = async {
                    let png = PersonaImageCliGenerator::new(
                        provider,
                        self.normalize.clone(),
                        self.runner.clone(),
                    )
                    .generate(prompt, None, cancel)
                    .await?;
                    if cancel.is_cancelled() {
                        return Err(Error::Cancelled);
                    }
                    let root = match &self.output_root {
                        Some(root) => root.clone(),
                        None => crate::provider_storage::home(id)?.join("generated-replies"),
                    };
                    std::fs::create_dir_all(&root)?;
                    let path = root.join(format!("{}.png", uuid::Uuid::new_v4().simple()));
                    tokio::fs::write(&path, png).await?;
                    Ok(path)
                };
                match attempt.await {
                    Ok(path) => {
                        report("생성 이미지 준비 완료");
                        return Ok(path);
                    }
                    Err(Error::Cancelled) if cancel.is_cancelled() => return Err(Error::Cancelled),
                    Err(error) => {
                        errors.push(format!("{id}: {error}"));
                        report(&format!("{id} 이미지 생성 실패 · 다음 프로바이더 확인"));
                    }
                }
            }
        }
        Err(Error::invalid_operation(if errors.is_empty() {
            "사용 가능한 Codex/Grok 이미지 프로바이더가 없습니다.".to_string()
        } else {
            format!("이미지 CLI 2회전 실패: {}", errors.join(" / "))
        }))
    }
}
