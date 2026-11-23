//! 원본은 .NET 예외 종류로 재시도·폴백 여부를 가른다(`catch ... when (e is IOException or ...)`).
//! 같은 분기를 유지하려고 예외 종류를 변형으로 보존한다.

use crate::chat_models::ApiFailure;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// `ApiFailure`. HTTP 상태와 코드를 가진다.
    #[error("{0}")]
    Api(ApiFailure),
    /// `OperationCanceledException`. 사용자 취소인지 시간 초과인지는 호출자가 취소 토큰으로 구분한다(원본과 같다).
    #[error("작업이 취소되었습니다.")]
    Cancelled,
    /// `TimeoutException`.
    #[error("{0}")]
    Timeout(String),
    /// `InvalidOperationException`.
    #[error("{0}")]
    InvalidOperation(String),
    /// `IOException`(하위 `FileNotFoundException` 포함).
    #[error("{0}")]
    Io(String),
    /// `DirectoryNotFoundException`. `IOException`의 하위 형식이라 `is_io`에 포함된다.
    #[error("{0}")]
    DirectoryNotFound(String),
    /// `InvalidDataException`.
    #[error("{0}")]
    InvalidData(String),
    /// `JsonException`.
    #[error("{0}")]
    Json(String),
    /// `ArgumentException`(하위 `ArgumentOutOfRangeException` 포함).
    #[error("{0}")]
    Argument(String),
    /// `UnauthorizedAccessException`.
    #[error("{0}")]
    Unauthorized(String),
    /// `NotSupportedException`.
    #[error("{0}")]
    NotSupported(String),
    /// `FormatException`.
    #[error("{0}")]
    Format(String),
    /// `KeyNotFoundException`·`InvalidOperationException(Sequence contains ...)` 등 컬렉션 조회 실패.
    #[error("{0}")]
    KeyNotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn invalid_operation(message: impl Into<String>) -> Self {
        Self::InvalidOperation(message.into())
    }
    pub fn io(message: impl Into<String>) -> Self {
        Self::Io(message.into())
    }
    pub fn invalid_data(message: impl Into<String>) -> Self {
        Self::InvalidData(message.into())
    }
    pub fn argument(message: impl Into<String>) -> Self {
        Self::Argument(message.into())
    }
    pub fn json(message: impl Into<String>) -> Self {
        Self::Json(message.into())
    }
    /// `e is IOException`. .NET 계층에서 `DirectoryNotFoundException`도 `IOException`이다.
    pub fn is_io(&self) -> bool {
        matches!(self, Self::Io(_) | Self::DirectoryNotFound(_))
    }
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

impl From<ApiFailure> for Error {
    fn from(value: ApiFailure) -> Self {
        Self::Api(value)
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        match value.kind() {
            std::io::ErrorKind::PermissionDenied => Self::Unauthorized(value.to_string()),
            _ => Self::Io(value.to_string()),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value.to_string())
    }
}
