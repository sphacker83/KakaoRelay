//! .NET `CancellationTokenSource.CreateLinkedTokenSource` + `CancelAfter` 대응.
//! 사용자 취소와 시간 초과 모두 `Error::Cancelled`(원본 `OperationCanceledException`)로 끝난다.
//! 작업 미래(future)는 버려지며, 그 안의 자식 프로세스는 `Drop`에서 트리째 종료된다.

use std::future::Future;
use std::time::Duration;

pub use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};

pub async fn run<T>(
    work: impl Future<Output = Result<T>>,
    cancel: &CancellationToken,
    timeout: Option<Duration>,
) -> Result<T> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let sleep = async {
        match timeout {
            Some(t) => tokio::time::sleep(t).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        result = work => result,
        _ = cancel.cancelled() => Err(Error::Cancelled),
        _ = sleep => Err(Error::Cancelled),
    }
}

pub fn seconds(value: i32) -> Duration {
    Duration::from_secs(u64::try_from(value).unwrap_or(0))
}
