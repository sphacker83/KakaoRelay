//! 원본: `ImageSender.cs`. 이미지 검증, 전송용 사본 보관, 이미지 영수증 상태 기계.

use std::io::Read;
use std::path::{Path, PathBuf};

use kakaorelay_ai::persona_images::{PersonaAttachment, PersonaImageCatalog};
use kakaorelay_ai::settings::AiSettings;
use kakaorelay_ai::{ApiFailure, Error, Result};
use sha2::{Digest, Sha256};

use crate::fs::{SendLock, full_path, is_fully_qualified};
use crate::platform::ImageTransport;
use crate::support::now;
use crate::test_send::{TestSendReceipt, TestSendRequest, load, receipt_path, save};

/// 원본 `Documents\KakaoRelay\attachments`. 받은 첨부 캐시(`<앱 데이터>/attachments`)와 다른 폴더다.
/// 카카오톡이 업로드하는 동안 페르소나 보관함이 바뀌어도 되도록 사본을 남겨 둔다.
pub fn default_transfer_root() -> PathBuf {
    dirs::document_dir()
        .unwrap_or_else(kakaorelay_ai::paths::user_profile)
        .join("KakaoRelay")
        .join("attachments")
}

/// 원본 `ValidateLocalImage`. 확장자와 파일 머리 바이트가 맞는 로컬 이미지의 정규화 경로를 돌려준다.
pub fn validate_local_image(path: &str) -> Result<PathBuf> {
    let unavailable = || {
        Error::Api(ApiFailure::new(
            400,
            "image_unavailable",
            "PC에 존재하는 로컬 이미지 절대 경로를 지정하세요.",
        ))
    };
    if !is_fully_qualified(path) || path.starts_with("\\\\") || !Path::new(path).is_file() {
        return Err(unavailable());
    }
    let extension = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    let mut header = [0u8; 12];
    let mut file = std::fs::File::open(path)?;
    let mut count = 0;
    while count < header.len() {
        let read = file.read(&mut header[count..])?;
        if read == 0 {
            break;
        }
        count += read;
    }
    let h = &header[..count];
    let valid = match extension.as_str() {
        "png" => h.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]),
        "jpg" | "jpeg" => h.starts_with(&[255, 216, 255]),
        "gif" => h.starts_with(b"GIF87a") || h.starts_with(b"GIF89a"),
        "bmp" => h.starts_with(b"BM"),
        "webp" => count >= 12 && &h[..4] == b"RIFF" && &h[8..12] == b"WEBP",
        _ => false,
    };
    if !valid {
        return Err(Error::Api(ApiFailure::new(
            400,
            "invalid_image",
            "이미지 확장자와 파일 내용을 확인하세요. PNG, JPEG, GIF, BMP, WebP를 지원합니다.",
        )));
    }
    Ok(full_path(Path::new(path)))
}

/// 원본 `ResolvePath`. 등록된 페르소나 이미지의 파일 경로.
pub fn resolve_path(attachment: &PersonaAttachment) -> Result<PathBuf> {
    let store = PersonaImageCatalog::new(AiSettings::image_root(&attachment.persona_id)?)
        .store(&attachment.provider)?;
    if !store
        .load()?
        .images
        .iter()
        .any(|i| i.id == attachment.image_id)
    {
        return Err(Error::Api(ApiFailure::new(
            400,
            "image_unavailable",
            "등록된 페르소나 이미지만 보낼 수 있습니다.",
        )));
    }
    let path = store.image_path(&attachment.image_id)?;
    if !path.is_file() {
        return Err(Error::Api(ApiFailure::new(
            400,
            "image_unavailable",
            "이미지 파일이 없습니다.",
        )));
    }
    Ok(path)
}

/// 이미지 요청의 `message` 값. 파일 내용이 바뀌면 같은 요청 ID로 보낼 수 없다.
pub fn file_identity(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(format!(
        "image:{}",
        kakaorelay_ai::json::upper_hex(&Sha256::digest(&bytes))
    ))
}

/// 원본 `ImageSender.SendOnce`. 블로킹 호출이다.
pub fn send_once(
    request: &TestSendRequest,
    image_path: &Path,
    transport: &mut dyn ImageTransport,
    ledger: &Path,
    transfer_root: Option<&Path>,
) -> Result<TestSendReceipt> {
    request.validate()?;
    let _gate = SendLock::acquire(ledger, "Another send is in progress")?;
    let path = receipt_path(ledger, &request.request_id);
    if path.exists() {
        let previous = load(&path)?;
        if previous.fingerprint != request.fingerprint() {
            return Err(Error::invalid_operation("Image request ID conflict"));
        }
        return Ok(previous);
    }
    let mut receipt = TestSendReceipt {
        request_id: request.request_id.clone(),
        recipient: request.recipient.clone(),
        fingerprint: request.fingerprint(),
        kind: "image".into(),
        ..Default::default()
    };
    save(&path, &receipt)?;
    let mut attempted = false;
    if let Err(error) = dispatch(
        request,
        image_path,
        transport,
        &path,
        transfer_root,
        &mut receipt,
        &mut attempted,
    ) {
        receipt.status = if attempted {
            "needs-review"
        } else {
            "blocked-before-input"
        }
        .into();
        receipt.detail = error.to_string();
    }
    receipt.finished_at = Some(now());
    save(&path, &receipt)?;
    Ok(receipt)
}

fn dispatch(
    request: &TestSendRequest,
    image_path: &Path,
    transport: &mut dyn ImageTransport,
    path: &Path,
    transfer_root: Option<&Path>,
    receipt: &mut TestSendReceipt,
    attempted: &mut bool,
) -> Result<()> {
    if file_identity(image_path)? != request.message {
        return Err(Error::invalid_data("이미지 파일이 변경되었습니다."));
    }
    transport.validate(request)?;
    let staging = transfer_root.map_or_else(default_transfer_root, Path::to_path_buf);
    std::fs::create_dir_all(&staging)?;
    let extension = image_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_default();
    let transfer_path = staging.join(format!("{}{extension}", uuid::Uuid::new_v4().simple()));
    copy_new(image_path, &transfer_path)?;
    if file_identity(&transfer_path)? != request.message {
        return Err(Error::invalid_data("전송용 이미지가 원본과 다릅니다."));
    }
    receipt.status = "dispatching".into();
    save(path, receipt)?;
    *attempted = true;
    transport.attach(&transfer_path)?;
    receipt.attachment_queued = true;
    save(path, receipt)?;
    let closed = transport.confirm_preview()?;
    receipt.enter_posted = true;
    receipt.input_cleared = Some(closed);
    receipt.status = "needs-review".into();
    receipt.detail = if closed {
        "Image preview send was requested once and preview closed. Verify the outgoing image; do not resend."
    } else {
        "Image send was requested but preview closure is uncertain. Do not resend."
    }
    .into();
    Ok(())
}

/// 원본 `File.Copy(.., overwrite: false)`.
fn copy_new(source: &Path, target: &Path) -> Result<()> {
    let mut input = std::fs::File::open(source)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    Ok(())
}
