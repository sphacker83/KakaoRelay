//! 원본: `ReplyRuntime.cs`. 방마다 독립 작업과 취소를 가진다. 한 방을 바꿔도 다른 방은 다시 시작하지 않는다.

use std::sync::{Arc, Mutex};

use kakaorelay_ai::ai_service::AiService;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::settings::room_key;
use kakaorelay_ai::{Error, LocalRoom, Result};

use crate::api_send::{ApiSendCommand, Scan};
use crate::auto_reply::AutoReplySession;
use crate::ids::is_positive_id;
use crate::readiness::prepare;
use crate::support::{BoxFuture, StatusEvent};
use crate::test_send::TestSendReceipt;

/// 취소 가능한 공용 발송(원본 `Func<ApiSendCommand, CancellationToken, Task<TestSendReceipt>>`).
pub type CancellableSend = Arc<
    dyn Fn(ApiSendCommand, CancellationToken) -> BoxFuture<'static, Result<TestSendReceipt>>
        + Send
        + Sync,
>;

struct Entry {
    key: String,
    room: LocalRoom,
    stop: CancellationToken,
    done: CancellationToken,
    status: Arc<Mutex<String>>,
}

struct Inner {
    ai: Arc<AiService>,
    send: CancellableSend,
    scan: Scan,
    entries: Mutex<Vec<Entry>>,
    last_error: Mutex<String>,
    changed: StatusEvent,
}

#[derive(Clone)]
pub struct ReplyRuntime {
    inner: Arc<Inner>,
}

impl ReplyRuntime {
    pub fn new(ai: Arc<AiService>, send: CancellableSend, scan: Scan) -> Self {
        Self {
            inner: Arc::new(Inner {
                ai,
                send,
                scan,
                entries: Mutex::new(vec![]),
                last_error: Mutex::new(String::new()),
                changed: StatusEvent::default(),
            }),
        }
    }

    /// 방 목록·상태·마지막 오류가 바뀔 때 알린다(값은 빈 문자열).
    pub fn on_changed(&self, handler: impl Fn() + Send + Sync + 'static) {
        self.inner.changed.subscribe(move |_| handler());
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.inner.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn count(&self) -> usize {
        self.entries().len()
    }

    pub fn statuses(&self) -> Vec<String> {
        self.entries()
            .iter()
            .map(|e| e.status.lock().unwrap_or_else(|p| p.into_inner()).clone())
            .collect()
    }

    pub fn running(&self, room: &LocalRoom) -> bool {
        let key = room_key(&room.profile, &room.id);
        self.entries().iter().any(|e| e.key == key)
    }

    pub fn last_error(&self) -> String {
        self.inner
            .last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub async fn start(&self, rooms: &[LocalRoom], cancel: &CancellationToken) -> Result<()> {
        let selected: Vec<LocalRoom> = rooms.iter().filter(|r| !self.running(r)).cloned().collect();
        if selected.is_empty() {
            return Ok(());
        }
        let settings = self.inner.ai.settings().load()?;
        if !is_positive_id(&settings.self_id) {
            return Err(Error::invalid_operation("카카오 ID를 먼저 설정하세요."));
        }
        let (_, ready) = prepare(
            self.inner.ai.chats().as_ref(),
            &selected,
            &self.inner.scan,
            cancel,
        )
        .await?;
        for room in ready {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let key = room_key(&room.profile, &room.id);
            let entry = Entry {
                key: key.clone(),
                room: room.clone(),
                stop: CancellationToken::new(),
                done: CancellationToken::new(),
                status: Arc::new(Mutex::new("시작 준비".into())),
            };
            let (stop, done, status) =
                (entry.stop.clone(), entry.done.clone(), entry.status.clone());
            {
                let mut entries = self.entries();
                if entries.iter().any(|e| e.key == key) {
                    continue;
                }
                entries.push(entry);
            }
            let inner = self.inner.clone();
            let self_id = settings.self_id.clone();
            tokio::spawn(async move {
                Inner::run(inner, key, room, self_id, stop, done, status).await;
            });
        }
        self.inner.changed.emit("");
        Ok(())
    }

    /// 지정한 방(없으면 전체)을 멈추고 세션이 끝날 때까지 기다린다.
    pub async fn stop(&self, room: Option<&LocalRoom>) {
        let waits: Vec<CancellationToken> = self
            .entries()
            .iter()
            .filter(|e| room.is_none_or(|r| e.room.profile == r.profile && e.room.id == r.id))
            .map(|e| {
                e.stop.cancel();
                e.done.clone()
            })
            .collect();
        for done in waits {
            done.cancelled().await;
        }
    }

    pub async fn drain(&self) {
        let waits: Vec<CancellationToken> = self.entries().iter().map(|e| e.done.clone()).collect();
        for done in waits {
            done.cancelled().await;
        }
    }
}

impl Inner {
    async fn run(
        inner: Arc<Inner>,
        key: String,
        room: LocalRoom,
        self_id: String,
        stop: CancellationToken,
        done: CancellationToken,
        status: Arc<Mutex<String>>,
    ) {
        let send = inner.send.clone();
        let token = stop.clone();
        let session = Arc::new(AutoReplySession::new(
            inner.ai.clone(),
            Arc::new(move |command| send(command, token.clone())),
            None,
            None,
        ));
        let changed = inner.changed.clone();
        session.status.subscribe(move |s| {
            *status.lock().unwrap_or_else(|e| e.into_inner()) = s.to_string();
            changed.emit("");
        });
        match session.run(&room, &self_id, &stop).await {
            Err(Error::Cancelled) if stop.is_cancelled() => {}
            Err(error) => {
                *inner.last_error.lock().unwrap_or_else(|e| e.into_inner()) =
                    format!("{} · {error}", room.title);
            }
            Ok(()) => {}
        }
        inner
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|e| e.key != key);
        done.cancel();
        inner.changed.emit("");
    }
}
