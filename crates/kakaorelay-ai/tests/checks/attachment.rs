//! 원본: `AttachmentChecks.cs`. 자동 답장 후보 검사 2개(`AutoReplySession`)는 `kakaorelay-core/tests/checks/auto_reply.rs`에 있다.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};

use async_trait::async_trait;
use base64::Engine;
use kakaorelay_ai::attachment::AgentAttachment;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::chat_models::LocalMessage;
use kakaorelay_ai::cli_runner::CliAiRunner;
use kakaorelay_ai::received_attachments::{
    AttachmentHttp, HttpDownload, ReceivedAttachments, allowed_url,
};
use kakaorelay_ai::settings::AiProviderSettings;
use kakaorelay_ai::{Result, json};
use sha1::Digest;

use crate::support::{Check, fake_exe};

/// 원본 `HttpMessageHandler` 가짜: 항상 같은 본문을 돌려주고 호출 수를 센다.
struct Handler {
    bytes: Vec<u8>,
    calls: AtomicUsize,
    status: AtomicU16,
}

impl Handler {
    fn new(bytes: &[u8]) -> Arc<Self> {
        Arc::new(Self {
            bytes: bytes.to_vec(),
            calls: AtomicUsize::new(0),
            status: AtomicU16::new(200),
        })
    }
}

#[async_trait]
impl AttachmentHttp for Handler {
    async fn get(&self, _url: &str, _limit: usize) -> Result<HttpDownload> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(HttpDownload {
            status: self.status.load(Ordering::SeqCst),
            content_length: Some(self.bytes.len() as u64),
            body: self.bytes.clone(),
        })
    }
}

fn sha1_hex(bytes: &[u8]) -> String {
    json::upper_hex(&sha1::Sha1::digest(bytes))
}

pub async fn run(root: &Path, check: &Check) {
    let none = CancellationToken::new();
    let bytes = "회의 결정: 금요일까지 제출".as_bytes().to_vec();
    let handler = Handler::new(&bytes);
    let reader = ReceivedAttachments::new(handler.clone(), Some(root.join("attachments")));
    let message = |url: &str, hash: Option<String>| {
        LocalMessage {
        attachment_metadata: Some(
            serde_json::json!({"name": "회의.txt", "size": bytes.len(), "cs": hash.unwrap_or_else(|| sha1_hex(&bytes)), "url": url}).to_string(),
        ),
        ..LocalMessage::new("1", "human", "작성자", chrono::Utc::now().fixed_offset(), 18, "회의.txt", false)
    }
    };
    let m = message("https://talk.kakaocdn.net/test", None);
    let result = reader.read(m.clone(), &none).await.unwrap();
    check.check(
        result.text.contains("금요일까지 제출") && handler.calls.load(Ordering::SeqCst) == 1,
        "Attachment text is downloaded, verified and extracted",
    );
    reader.read(m.clone(), &none).await.unwrap();
    check.check(
        handler.calls.load(Ordering::SeqCst) == 1,
        "Verified attachment cache avoids repeat downloads",
    );
    check.check(
        !serde_json::to_string(&m).unwrap().contains("kakaocdn"),
        "Attachment URLs never serialize into prompts or API messages",
    );
    check.check(
        !allowed_url("http://talk.kakaocdn.net/a")
            && !allowed_url("https://kakaocdn.net.evil.test/a")
            && !allowed_url("https://127.0.0.1/a"),
        "Attachment downloads reject non-HTTPS and unrelated hosts",
    );
    let bad = reader
        .read(
            message("https://talk.kakaocdn.net/test", Some("0".repeat(40))),
            &none,
        )
        .await
        .unwrap();
    check.check(
        bad.text.contains("검증에 실패"),
        "Mismatched attachment checksum never enters the prompt",
    );
    let untouched = reader
        .read(
            LocalMessage {
                deleted: true,
                ..m.clone()
            },
            &none,
        )
        .await
        .unwrap();
    check.check(
        untouched.text == m.text,
        "Deleted attachments are never downloaded or extracted",
    );
    handler.status.store(302, Ordering::SeqCst);
    let redirect = reader
        .read(
            message("https://talk.kakaocdn.net/test", Some("1".repeat(40))),
            &none,
        )
        .await
        .unwrap();
    check.check(
        redirect.text.contains("첨부 읽기 실패"),
        "Redirect response is not accepted as an attachment",
    );
    handler.status.store(200, Ordering::SeqCst);
    let photo_metadata = |size: usize| {
        serde_json::json!({"s": size, "cs": "", "url": "https://talk.kakaocdn.net/photo"})
            .to_string()
    };
    let photo = LocalMessage {
        kind: 2,
        text: "사진".into(),
        attachment_metadata: Some(photo_metadata(bytes.len())),
        ..m.clone()
    };
    let invalid = reader.read(photo.clone(), &none).await.unwrap();
    check.check(
        invalid.attachment.is_none() && invalid.text.contains("첨부 읽기 실패"),
        "Invalid image bytes are not attached",
    );
    let image_bytes = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aN1EAAAAASUVORK5CYII=").unwrap();
    let photo_reader =
        ReceivedAttachments::new(Handler::new(&image_bytes), Some(root.join("photos")));
    let photo = LocalMessage {
        attachment_metadata: Some(photo_metadata(image_bytes.len())),
        ..photo
    };
    let original = photo_reader.read(photo, &none).await.unwrap();
    let image = original.attachment.clone().unwrap();
    check.check(
        image.media_type == "image/png" && std::fs::read(&image.path).unwrap() == image_bytes,
        "Original image is preserved byte for byte without OCR",
    );
    check.check(
        !serde_json::to_string(&original)
            .unwrap()
            .contains(&image.path),
        "Private attachment paths stay out of serialized messages",
    );
    check.check(
        serde_json::to_string(&AgentAttachment::codex_input(
            "test",
            std::slice::from_ref(&image),
        ))
        .unwrap()
        .contains("localImage"),
        "Codex receives native image input",
    );
    let claude = AgentAttachment::claude_input("test", std::slice::from_ref(&image))
        .await
        .unwrap();
    check.check(
        claude[2]["source"]["data"].as_str()
            == Some(
                base64::engine::general_purpose::STANDARD
                    .encode(&image_bytes)
                    .as_str(),
            ),
        "Claude receives original image bytes",
    );
    let pdf_bytes = b"%PDF-1.7\nfixture".to_vec();
    let pdf_message = LocalMessage {
        attachment_metadata: Some(serde_json::json!({"name": "scan.pdf", "size": pdf_bytes.len(), "cs": sha1_hex(&pdf_bytes), "url": "https://talk.kakaocdn.net/scan"}).to_string()),
        ..m.clone()
    };
    let pdf = ReceivedAttachments::new(Handler::new(&pdf_bytes), Some(root.join("pdf")))
        .read(pdf_message, &none)
        .await
        .unwrap();
    let pdf_file = pdf.attachment.clone().unwrap();
    check.check(
        pdf_file.media_type == "application/pdf"
            && std::fs::read(&pdf_file.path).unwrap() == pdf_bytes,
        "PDF original is passed without text extraction",
    );
    let pdf_input = json::prompt(&AgentAttachment::codex_input(
        "test",
        std::slice::from_ref(&pdf_file),
    ));
    check.check(
        pdf_input.contains("PDF 원본 경로") && !pdf_input.contains("localImage"),
        "Codex receives PDF original path for agent tools, not an image or forced fallback",
    );
    let codex = AiProviderSettings::new("codex");
    check.check(
        CliAiRunner::resident_arguments(&codex, None, "test", true)
            .unwrap()
            .contains(&"features.shell_tool=true".to_string())
            && CliAiRunner::resident_arguments(&codex, None, "test", false)
                .unwrap()
                .contains(&"features.shell_tool=false".to_string()),
        "Local file tools are enabled only for PDF sessions",
    );
    {
        let cli = CliAiRunner::new();
        let fake = AiProviderSettings {
            executable: fake_exe(),
            model: "fixture".into(),
            ..AiProviderSettings::new("codex")
        };
        let folder = root.join("pdf-session");
        let files = std::slice::from_ref(&pdf_file);
        let first = cli
            .run_conversation(&fake, "read PDF", 10, &none, &folder, None, files)
            .await
            .unwrap();
        let next = cli
            .run_conversation(
                &fake,
                "ordinary followup",
                10,
                &none,
                &folder,
                Some(&first.1),
                files,
            )
            .await
            .unwrap();
        check.check(
            first.1 == next.1,
            "PDF and ordinary follow-up reuse Luna session with file tools",
        );
        cli.dispose();
    }
    check.check(
        serde_json::to_string(
            &AgentAttachment::claude_input("test", std::slice::from_ref(&pdf_file))
                .await
                .unwrap(),
        )
        .unwrap()
        .contains("document"),
        "Claude receives native PDF document block",
    );
}
