//! 검사용 가짜 대화 리더·실행기·시계와 발송 기록기(원본 검사 파일마다 있던 `Reader`/`Runner`/`Clock` 대응).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, TimeZone};
use kakaorelay_ai::ai_service::{AiResult, AiService};
use kakaorelay_ai::chat_image::ChatImageGenerator;
use kakaorelay_ai::cli_runner::AiRunner;
use kakaorelay_ai::settings::AiSettingsStore;
use kakaorelay_ai::{ApiFailure, ChatContext, ChatReader, LocalMessage, LocalRoom, Result};
use kakaorelay_core::api_send::ApiSendCommand;
use kakaorelay_core::auto_reply::CommandSend;
use kakaorelay_core::support::{BoxFuture, Clock};
use kakaorelay_core::test_send::TestSendReceipt;

pub fn message(id: &str, author: &str, text: &str) -> LocalMessage {
    LocalMessage::new(
        id,
        author,
        author,
        chrono::Utc::now().fixed_offset(),
        1,
        text,
        false,
    )
}

pub fn typed(id: &str, author: &str, text: &str, kind: i32, deleted: bool) -> LocalMessage {
    LocalMessage::new(
        id,
        author,
        author,
        chrono::Utc::now().fixed_offset(),
        kind,
        text,
        deleted,
    )
}

pub fn ai(
    reader: Arc<dyn ChatReader>,
    store: Arc<AiSettingsStore>,
    runner: Arc<dyn AiRunner>,
) -> Arc<AiService> {
    Arc::new(AiService::new(reader, store, runner, None, None))
}

pub fn ai_with_images(
    reader: Arc<dyn ChatReader>,
    store: Arc<AiSettingsStore>,
    runner: Arc<dyn AiRunner>,
    images: Arc<dyn ChatImageGenerator>,
) -> Arc<AiService> {
    Arc::new(AiService::new(reader, store, runner, Some(images), None))
}

pub fn confirmed() -> TestSendReceipt {
    receipt(true, Some(true))
}

pub fn receipt(enter_posted: bool, input_cleared: Option<bool>) -> TestSendReceipt {
    TestSendReceipt {
        enter_posted,
        input_cleared,
        ..Default::default()
    }
}

/// 보낸 명령을 기록하고 정해진 영수증을 돌려주는 발송. `hook`은 기록 직후 호출된다.
pub fn recorder(
    sent: Arc<Mutex<Vec<ApiSendCommand>>>,
    hook: impl Fn(&ApiSendCommand, usize) -> BoxFuture<'static, Result<TestSendReceipt>>
    + Send
    + Sync
    + 'static,
) -> CommandSend {
    Arc::new(move |command| {
        let count = {
            let mut list = sent.lock().unwrap();
            list.push(command.clone());
            list.len()
        };
        hook(&command, count)
    })
}

pub fn ready(receipt: TestSendReceipt) -> BoxFuture<'static, Result<TestSendReceipt>> {
    Box::pin(async move { Ok(receipt) })
}

/// 조건이 참이 될 때까지 최대 5초 기다린다(원본은 동기 실행 순서로 보장하던 지점).
pub async fn until(condition: impl Fn() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 원본 `TimeProvider` 시험용. 시간대는 고정 오프셋이다.
pub struct FixedClock(pub Mutex<DateTime<FixedOffset>>);

impl FixedClock {
    pub fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Arc<Self> {
        Arc::new(Self(Mutex::new(
            FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(y, mo, d, h, mi, 0)
                .unwrap(),
        )))
    }
    pub fn advance(&self, duration: chrono::Duration) {
        let mut now = self.0.lock().unwrap();
        *now += duration;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<FixedOffset> {
        *self.0.lock().unwrap()
    }
}

/// 원본 `new AiResult(text, provider, model, effort, mode, room, count, attempts)`.
pub fn result(text: &str, room: &LocalRoom) -> AiResult {
    AiResult {
        text: text.into(),
        provider: "codex".into(),
        model: "fixture".into(),
        effort: "low".into(),
        mode: "reply".into(),
        room: room.clone(),
        message_count: 1,
        attempts: vec![],
        persona_id: "default".into(),
        emotion: "neutral".into(),
        attachment: None,
        image_path: None,
        image_error: None,
        image_prompt: None,
        delegation: None,
        knowledge_sources: vec![],
    }
}

/// 고정 방과 메시지 목록을 돌려주는 리더. `read`는 마지막 `limit`개를 준다.
pub struct ListReader {
    pub rooms: Mutex<Vec<LocalRoom>>,
    pub messages: Mutex<Vec<LocalMessage>>,
}

impl ListReader {
    pub fn new(rooms: Vec<LocalRoom>) -> Arc<Self> {
        Arc::new(Self {
            rooms: Mutex::new(rooms),
            messages: Mutex::new(vec![]),
        })
    }
}

#[async_trait]
impl ChatReader for ListReader {
    async fn rooms(&self) -> std::result::Result<Vec<LocalRoom>, ApiFailure> {
        Ok(self.rooms.lock().unwrap().clone())
    }
    async fn read(
        &self,
        _: &str,
        room_id: &str,
        limit: usize,
    ) -> std::result::Result<ChatContext, ApiFailure> {
        let room = self
            .rooms
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.id == room_id)
            .cloned()
            .ok_or_else(|| ApiFailure::new(404, "room_not_loaded", "없음"))?;
        let messages = self.messages.lock().unwrap();
        let start = messages.len().saturating_sub(limit);
        Ok(ChatContext {
            room,
            messages: messages[start..].to_vec(),
            read_at: chrono::Utc::now().fixed_offset(),
        })
    }
}
