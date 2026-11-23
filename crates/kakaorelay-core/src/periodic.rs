//! 원본 화면의 `DispatcherTimer` 주기 실행(정리감시 15초, 제어방 2초, 자동 정리 1분).
//! 시작 즉시 한 번 돌고, 이전 실행이 끝나지 않았으면 그 주기는 건너뛴다. 중지는 진행 중인 실행도 취소한다.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kakaorelay_ai::cancel::CancellationToken;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

pub struct Periodic {
    stop: CancellationToken,
    current: Arc<Mutex<Option<CancellationToken>>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Periodic {
    pub fn start<F, Fut>(interval: Duration, tick: F) -> Self
    where
        F: Fn(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let stop = CancellationToken::new();
        let current = Arc::new(Mutex::new(None::<CancellationToken>));
        let (loop_stop, loop_current) = (stop.clone(), current.clone());
        let task = tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = loop_stop.cancelled() => return,
                    _ = timer.tick() => {}
                }
                let token = loop_stop.child_token();
                *loop_current.lock().unwrap_or_else(|e| e.into_inner()) = Some(token.clone());
                tick(token).await;
                *loop_current.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
        });
        Self {
            stop,
            current,
            task: Mutex::new(Some(task)),
        }
    }

    /// 진행 중인 실행만 취소한다(원본 `ReloadSettings`의 `runCancellation.Cancel`). 다음 주기는 그대로 돈다.
    pub fn cancel_current(&self) {
        if let Some(token) = self
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            token.cancel();
        }
    }

    /// 새 실행을 막고 진행 중인 실행을 취소한다.
    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// 진행 중인 실행이 끝날 때까지 기다린다. `stop` 뒤에 부른다.
    pub async fn drain(&self) {
        let task = self.task.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

impl Drop for Periodic {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
