//! 서비스 사이에서 쓰는 작은 공용 형식: 시계, 상태 알림, 비동기 콜백, 어댑터 오류 변환.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset, Local};
use kakaorelay_ai::{ApiFailure, Error};

use crate::platform::TransportError;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 원본 `TimeProvider`. `now`는 현지 시각(오프셋 포함)이다. 테스트는 시간대와 시각을 고정한다.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<FixedOffset>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<FixedOffset> {
        Local::now().fixed_offset()
    }
}

pub(crate) fn now() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}

/// 원본 `event Action<string>`. 구독자가 여럿일 수 있고 알림 순서는 등록 순서다.
type StatusHandler = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default, Clone)]
pub struct StatusEvent {
    handlers: Arc<Mutex<Vec<StatusHandler>>>,
}

impl StatusEvent {
    pub fn subscribe(&self, handler: impl Fn(&str) + Send + Sync + 'static) {
        self.handlers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::new(handler));
    }

    pub fn emit(&self, text: &str) {
        let handlers = self
            .handlers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for handler in handlers {
            handler(text);
        }
    }
}

/// 어댑터 오류를 원본 예외 분기로 옮긴다. 권한 부족은 API가 409 `{code, detail}`로 응답한다.
impl From<TransportError> for Error {
    fn from(error: TransportError) -> Self {
        match error {
            TransportError::InputTimeout(_) => Error::Timeout(error.to_string()),
            TransportError::PermissionDenied { code, detail } => {
                Error::Api(ApiFailure::new(409, code, detail))
            }
            TransportError::Failed(message) => Error::InvalidOperation(message),
        }
    }
}

/// 취소 가능한 대화 읽기. 어댑터 읽기는 취소 토큰을 받지 않으므로 바깥에서 기다림을 끊는다.
pub async fn chat_read(
    reader: &dyn kakaorelay_ai::ChatReader,
    profile: &str,
    room_id: &str,
    limit: usize,
    cancel: &kakaorelay_ai::cancel::CancellationToken,
) -> kakaorelay_ai::Result<kakaorelay_ai::ChatContext> {
    kakaorelay_ai::cancel::run(
        async { Ok(reader.read(profile, room_id, limit).await?) },
        cancel,
        None,
    )
    .await
}

pub async fn chat_rooms(
    reader: &dyn kakaorelay_ai::ChatReader,
    cancel: &kakaorelay_ai::cancel::CancellationToken,
) -> kakaorelay_ai::Result<Vec<kakaorelay_ai::LocalRoom>> {
    kakaorelay_ai::cancel::run(async { Ok(reader.rooms().await?) }, cancel, None).await
}

/// 블로킹 창 목록 조회(원본 `Task.Run(scan)`).
pub async fn scan(
    windows: &crate::api_send::Scan,
    cancel: &kakaorelay_ai::cancel::CancellationToken,
) -> kakaorelay_ai::Result<Vec<crate::platform::ConversationTarget>> {
    let windows = windows.clone();
    let work = async move {
        tokio::task::spawn_blocking(move || windows())
            .await
            .map_err(|e| Error::invalid_operation(e.to_string()))?
            .map_err(Error::from)
    };
    kakaorelay_ai::cancel::run(work, cancel, None).await
}

/// .NET 기본 문자열 비교(현재 문화권)의 근사: 대소문자 무시 비교 후 서수 비교.
pub fn culture_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    kakaorelay_ai::text::ordinal_ignore_case_cmp(a, b).then_with(|| a.cmp(b))
}

/// 원본은 예외 형식 이름만 기록한다(본문·경로를 남기지 않는다).
pub fn error_kind(error: &Error) -> String {
    match error {
        Error::Api(_) => "ApiFailure",
        Error::Cancelled => "OperationCanceledException",
        Error::Timeout(_) => "TimeoutException",
        Error::InvalidOperation(_) => "InvalidOperationException",
        Error::Io(_) => "IOException",
        Error::DirectoryNotFound(_) => "DirectoryNotFoundException",
        Error::InvalidData(_) => "InvalidDataException",
        Error::Json(_) => "JsonException",
        Error::Argument(_) => "ArgumentException",
        Error::Unauthorized(_) => "UnauthorizedAccessException",
        Error::NotSupported(_) => "NotSupportedException",
        Error::Format(_) => "FormatException",
        Error::KeyNotFound(_) => "KeyNotFoundException",
    }
    .into()
}
