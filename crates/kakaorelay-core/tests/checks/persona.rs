//! 원본: `PersonaChecks.cs`의 Core 의존 검사 중 이미지 발송 원장 3개와 API 멱등 지문 1개.
//! 호출어 후보 검사 1개는 `auto_reply.rs`에 있다.

use std::path::Path;

use kakaorelay_ai::persona_images::PersonaAttachment;
use kakaorelay_core::api_send::ApiSendCommand;
use kakaorelay_core::image_send::{file_identity, send_once};
use kakaorelay_core::test_send::TestSendRequest;

use crate::support::{Check, FakeImage, PNG, in_minutes};

pub fn run(root: &Path, check: &Check) {
    let ledger = root.join("image-sends");
    let image_path = root.join("persona.png");
    std::fs::write(&image_path, PNG).unwrap();
    let mut request = TestSendRequest::new(
        "image-once",
        "fixture-room",
        file_identity(&image_path).unwrap(),
        1,
        "0x1",
        in_minutes(5),
    );
    request.verified_empty_placeholder = true;
    let transfers = root.join("transfers");
    let mut transport = FakeImage::default();
    let first = send_once(
        &request,
        &image_path,
        &mut transport,
        &ledger,
        Some(&transfers),
    )
    .unwrap();
    let duplicate = send_once(
        &request,
        &image_path,
        &mut transport,
        &ledger,
        Some(&transfers),
    )
    .unwrap();
    check.check(
        first.kind == "image"
            && first.attachment_queued
            && first.enter_posted
            && transport.attachments == 1
            && duplicate.request_id == first.request_id,
        "Duplicate image requests return the saved receipt without attaching again",
    );
    let attached = transport.attached.clone().unwrap();
    check.check(
        attached != image_path && std::fs::read(&attached).unwrap() == PNG,
        "Attachment uses an independent byte-identical file that survives library edits",
    );
    let mut failed = FakeImage {
        fail: true,
        ..Default::default()
    };
    let failed_request = TestSendRequest {
        request_id: "image-uncertain".into(),
        ..request.clone()
    };
    let uncertain = send_once(
        &failed_request,
        &image_path,
        &mut failed,
        &ledger,
        Some(&transfers),
    )
    .unwrap();
    send_once(
        &failed_request,
        &image_path,
        &mut failed,
        &ledger,
        Some(&transfers),
    )
    .unwrap();
    check.check(
        uncertain.status == "needs-review" && failed.attachments == 1,
        "Uncertain attachment is never replayed even when no confirmation was posted",
    );
    let mut with_attachment = ApiSendCommand::new("same", "room", "");
    with_attachment.attachment = Some(PersonaAttachment {
        persona_id: "default".into(),
        provider: "astra".into(),
        image_id: uuid::Uuid::new_v4().simple().to_string(),
    });
    check.check(
        ApiSendCommand::new("same", "room", "").fingerprint() != with_attachment.fingerprint(),
        "Attachment identity participates in API idempotency",
    );
}
