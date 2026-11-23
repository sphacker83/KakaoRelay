//! 원본: `AsyncWorkQueue.cs`. 호출 시점에 순서를 예약하고, 도착 순서대로 한 번에 하나씩 실행한다.

use std::future::Future;
use std::sync::Mutex;

use tokio::sync::oneshot;

#[derive(Default)]
pub struct WorkQueue {
    tail: Mutex<Option<oneshot::Receiver<()>>>,
}

impl WorkQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 자리는 이 함수를 부르는 순간 잡힌다. 반환된 미래를 버리면 다음 작업이 바로 이어진다.
    pub fn enqueue<T, F: Future<Output = T>>(
        &self,
        operation: F,
    ) -> impl Future<Output = T> + use<T, F> {
        let (finished, next) = oneshot::channel::<()>();
        let previous = self
            .tail
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(next);
        async move {
            // 보낸 쪽이 drop되면(앞 작업 완료·취소) 수신이 끝난다.
            let _finished = finished;
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            operation.await
        }
    }
}
