//! 원본: `RelayApi.cs`의 `ApiSendCommand`, `ApiImageCommand`, `ApiSendService`.
//! KakaoTalk에 손대기 전에 요청 ID와 내용을 예약 파일로 묶는다. 영수증은 방이 닫혀도 남는다.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kakaorelay_ai::json;
use kakaorelay_ai::persona_images::{PersonaAttachment, PersonaImageCatalog};
use kakaorelay_ai::summary_forward_settings::is_guid_n;
use kakaorelay_ai::text::is_blank;
use kakaorelay_ai::{ApiFailure, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::fs::{create_new, full_path, is_fully_qualified};
use crate::ids::valid_request_id;
use crate::image_send;
use crate::platform::{
    ConversationTarget, ImageTransport, Platform, TextTransport, TransportError,
};
use crate::support::now;
use crate::test_send::{self, TestSendReceipt, TestSendRequest, receipt_path};
use crate::work_queue::WorkQueue;

/// API·자동답장·정리감시·제어방이 공유하는 발송 명령. 기본 지연은 1000ms다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiSendCommand {
    pub request_id: String,
    pub recipient: String,
    pub message: String,
    pub send_delay_ms: i32,
    pub attachment: Option<PersonaAttachment>,
    pub image_path: Option<String>,
}

impl ApiSendCommand {
    pub fn new(
        request_id: impl Into<String>,
        recipient: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            recipient: recipient.into(),
            message: message.into(),
            send_delay_ms: 1000,
            attachment: None,
            image_path: None,
        }
    }

    pub fn is_image(&self) -> bool {
        self.attachment.is_some() || self.image_path.is_some()
    }

    pub fn validate(&self) -> Result<()> {
        if !(0..=30000).contains(&self.send_delay_ms) {
            return Err(failure(
                400,
                "invalid_send_delay",
                "sendDelayMs는 0~30000 밀리초 정수입니다.",
            ));
        }
        if !valid_request_id(&self.request_id)
            || is_blank(&self.recipient)
            || (!self.is_image() && is_blank(&self.message))
            || self.message.contains('\0')
        {
            return Err(failure(
                400,
                "invalid_request",
                "requestId, recipient, message를 확인하세요.",
            ));
        }
        if let Some(path) = &self.image_path
            && (self.attachment.is_some()
                || !self.message.is_empty()
                || !is_fully_qualified(path)
                || path.contains('\0'))
        {
            return Err(failure(
                400,
                "invalid_request",
                "imagePath는 로컬 이미지의 절대 경로이며 attachment와 함께 사용할 수 없습니다.",
            ));
        }
        if let Some(attachment) = &self.attachment
            && (!self.message.is_empty()
                || !valid_request_id(&attachment.persona_id)
                || !PersonaImageCatalog::PROVIDERS.contains(&attachment.provider.as_str())
                || !is_guid_n(&attachment.image_id))
        {
            return Err(failure(
                400,
                "invalid_request",
                "이미지 요청은 빈 message와 등록된 attachment 식별자를 사용하세요. 텍스트는 별도 요청으로 보냅니다.",
            ));
        }
        Ok(())
    }

    /// 골든 `apiFingerprint.*`. 이미지 경로는 정규화한 전체 경로로 묶는다.
    pub fn fingerprint(&self) -> String {
        let value = match (&self.image_path, &self.attachment) {
            (Some(path), _) => json!({
                "Recipient": self.recipient,
                "ImagePath": full_path(Path::new(path)).to_string_lossy(),
            }),
            (None, None) => json!({"Recipient": self.recipient, "Message": self.message}),
            (None, Some(a)) => json!({
                "Recipient": self.recipient,
                "Message": self.message,
                "Attachment": {"PersonaId": a.persona_id, "Provider": a.provider, "ImageId": a.image_id},
            }),
        };
        json::hash(&json::default(&value))
    }

    /// HTTP 본문(`JsonSerializerDefaults.Web`: 이름 대소문자 무시, 숫자 문자열 허용)에서 읽는다.
    pub fn from_json(value: Value) -> Result<Self> {
        let raw: RawCommand = read_web(value)?;
        let mut command = Self::new(
            raw.request_id.unwrap_or_default(),
            raw.recipient.unwrap_or_default(),
            // 원본은 `Message is null`을 `invalid_request`로 거부한다. NUL 문자는 같은 검사에서 같은 응답으로 거부된다.
            raw.message.unwrap_or_else(|| "\0".into()),
        );
        if let Some(delay) = raw.send_delay_ms {
            command.send_delay_ms = delay;
        }
        command.attachment = raw.attachment;
        command.image_path = raw.image_path;
        Ok(command)
    }
}

/// `POST /v1/send-image` 본문. `imagePath`와 `attachment` 중 하나만 쓴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiImageCommand {
    pub request_id: String,
    pub recipient: String,
    pub image_path: Option<String>,
    pub attachment: Option<PersonaAttachment>,
}

impl ApiImageCommand {
    pub fn to_send(&self) -> Result<ApiSendCommand> {
        let mut command = ApiSendCommand::new(&self.request_id, &self.recipient, "");
        command.image_path = self.image_path.clone();
        command.attachment = self.attachment.clone();
        command.validate()?;
        Ok(command)
    }

    pub fn from_json(value: Value) -> Result<Self> {
        let raw: RawCommand = read_web(value)?;
        Ok(Self {
            request_id: raw.request_id.unwrap_or_default(),
            recipient: raw.recipient.unwrap_or_default(),
            image_path: raw.image_path,
            attachment: raw.attachment,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCommand {
    request_id: Option<String>,
    recipient: Option<String>,
    message: Option<String>,
    #[serde(default, deserialize_with = "web_int_option")]
    send_delay_ms: Option<i32>,
    attachment: Option<PersonaAttachment>,
    image_path: Option<String>,
}

const KNOWN: [&str; 9] = [
    "requestId",
    "recipient",
    "message",
    "sendDelayMs",
    "attachment",
    "imagePath",
    "personaId",
    "provider",
    "imageId",
];

fn read_web<T: serde::de::DeserializeOwned>(mut value: Value) -> Result<T> {
    if !value.is_object() {
        return Err(Error::json("JSON 객체가 필요합니다."));
    }
    json::normalize_keys(&mut value, &KNOWN, &[]);
    Ok(serde_json::from_value(value)?)
}

/// 원본 `int` 속성. `null`은 형식 오류다(값을 생략하면 기본값을 쓴다).
fn web_int_option<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<i32>, D::Error> {
    json::web_int::deserialize(d).map(Some)
}

fn failure(status: u16, code: &str, detail: &str) -> Error {
    Error::Api(ApiFailure::new(status, code, detail))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reservation {
    fingerprint: String,
    engine_fingerprint: String,
}

pub type Scan =
    Arc<dyn Fn() -> std::result::Result<Vec<ConversationTarget>, TransportError> + Send + Sync>;
pub type TextFactory = Arc<dyn Fn() -> Box<dyn TextTransport> + Send + Sync>;
pub type ImageFactory = Arc<dyn Fn() -> Box<dyn ImageTransport> + Send + Sync>;

struct Inner {
    ledger: PathBuf,
    scan: Scan,
    text: TextFactory,
    image: Option<ImageFactory>,
    transfer_root: Option<PathBuf>,
}

/// 원본 `ApiSendService`. 발송은 한 번에 하나씩 도착 순서대로 처리하고, 블로킹 어댑터 호출은 `spawn_blocking`에서 한다.
pub struct ApiSendService {
    inner: Arc<Inner>,
    queue: WorkQueue,
}

impl ApiSendService {
    pub fn new(
        ledger: impl Into<PathBuf>,
        scan: Scan,
        text: TextFactory,
        image: Option<ImageFactory>,
        transfer_root: Option<PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                ledger: ledger.into(),
                scan,
                text,
                image,
                transfer_root,
            }),
            queue: WorkQueue::new(),
        }
    }

    pub fn from_platform(ledger: impl Into<PathBuf>, platform: Arc<dyn Platform>) -> Self {
        let (scan_platform, text_platform, image_platform) =
            (platform.clone(), platform.clone(), platform);
        Self::new(
            ledger,
            Arc::new(move || scan_platform.rooms().scan()),
            Arc::new(move || text_platform.text_transport()),
            Some(Arc::new(move || image_platform.image_transport())),
            None,
        )
    }

    /// 검증은 즉시 하고 순서는 호출 시점에 예약한다.
    pub fn send(
        &self,
        command: ApiSendCommand,
    ) -> impl Future<Output = Result<TestSendReceipt>> + Send + 'static {
        let validated = command.validate();
        let inner = self.inner.clone();
        self.queue.enqueue(async move {
            validated?;
            tokio::task::spawn_blocking(move || inner.send_blocking(&command))
                .await
                .map_err(|e| Error::invalid_operation(e.to_string()))?
        })
    }
}

impl Inner {
    fn send_blocking(&self, command: &ApiSendCommand) -> Result<TestSendReceipt> {
        let folder = self.ledger.join("api");
        std::fs::create_dir_all(&folder)?;
        let reservation_path = folder.join(format!("{}.json", command.request_id));
        let receipt_file = receipt_path(&self.ledger, &command.request_id);
        if reservation_path.exists() {
            let previous: Option<Reservation> =
                serde_json::from_str(&kakaorelay_ai::text::read_text(&reservation_path)?)?;
            let previous = previous.ok_or_else(|| {
                failure(
                    409,
                    "uncertain",
                    "기록을 확인할 수 없습니다. 재전송하지 마세요.",
                )
            })?;
            if previous.fingerprint != command.fingerprint() {
                return Err(failure(
                    409,
                    "request_id_conflict",
                    "같은 요청 ID에 다른 본문이나 대화방을 사용할 수 없습니다.",
                ));
            }
            if !receipt_file.exists() {
                return Err(failure(
                    409,
                    "uncertain",
                    "요청이 예약된 뒤 중단되었습니다. 대화를 확인하고 자동 재전송하지 마세요.",
                ));
            }
            let receipt = test_send::load(&receipt_file)?;
            if receipt.fingerprint != previous.engine_fingerprint {
                return Err(failure(
                    409,
                    "uncertain",
                    "발송 기록이 일치하지 않습니다. 대화를 확인하세요.",
                ));
            }
            return Ok(receipt);
        }
        if receipt_file.exists() {
            return Err(failure(
                409,
                "request_id_conflict",
                "이미 사용 중인 요청 ID입니다.",
            ));
        }
        let matches: Vec<ConversationTarget> = (self.scan)()?
            .into_iter()
            .filter(|r| r.title == command.recipient && r.selectable())
            .collect();
        let [room] = matches.as_slice() else {
            return Err(failure(
                409,
                "room_unavailable",
                "해당 이름의 열린 대화방을 하나로 확인할 수 없습니다.",
            ));
        };
        if command.is_image() && self.image.is_none() {
            return Err(failure(
                409,
                "image_unavailable",
                "이미지 전송 어댑터가 없습니다.",
            ));
        }
        let image_path = match (&command.image_path, &command.attachment) {
            (Some(path), _) => Some(image_send::validate_local_image(path)?),
            (None, Some(attachment)) => Some(image_send::resolve_path(attachment)?),
            (None, None) => None,
        };
        let message = match &image_path {
            Some(path) => image_send::file_identity(path)?,
            None => command.message.clone(),
        };
        let mut request = TestSendRequest::new(
            &command.request_id,
            &room.title,
            message,
            room.process_id,
            &room.handle,
            now() + chrono::Duration::minutes(5),
        );
        request.verified_empty_placeholder = true;
        request.room_id = room.room_id.clone();
        request.send_delay_ms = image_path.is_none().then_some(command.send_delay_ms);
        request.validate()?;
        let reservation = Reservation {
            fingerprint: command.fingerprint(),
            engine_fingerprint: request.fingerprint(),
        };
        create_new(&reservation_path, json::indented(&reservation).as_bytes())?;
        match image_path {
            None => test_send::send_once(&request, (self.text)().as_mut(), &self.ledger),
            Some(path) => image_send::send_once(
                &request,
                &path,
                self.image.as_ref().expect("위에서 확인함")().as_mut(),
                &self.ledger,
                self.transfer_root.as_deref(),
            ),
        }
    }
}
