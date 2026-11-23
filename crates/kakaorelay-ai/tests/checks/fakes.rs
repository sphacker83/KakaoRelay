//! 검사용 가짜 대화 리더와 AI 실행기(원본 검사 파일마다 있던 `Reader`/`Runner` 대응).

use std::sync::Mutex;

use async_trait::async_trait;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::chat_models::{ApiFailure, ChatContext, ChatReader, LocalMessage, LocalRoom};
use kakaorelay_ai::cli_runner::AiRunner;
use kakaorelay_ai::settings::AiProviderSettings;
use kakaorelay_ai::{Error, Result};

pub fn message(id: i64, text: &str, author: &str) -> LocalMessage {
    LocalMessage::new(
        id.to_string(),
        author,
        author,
        chrono::Utc::now().fixed_offset(),
        1,
        text,
        false,
    )
}

pub struct Reader {
    pub room: Mutex<LocalRoom>,
    pub messages: Mutex<Vec<LocalMessage>>,
    pub require_discovery: bool,
    pub discovered: Mutex<bool>,
}

impl Reader {
    pub fn new(room: LocalRoom, require_discovery: bool) -> Self {
        Self {
            room: Mutex::new(room),
            messages: Mutex::new(vec![]),
            require_discovery,
            discovered: Mutex::new(false),
        }
    }
    pub fn push(&self, message: LocalMessage) {
        self.messages.lock().unwrap().push(message);
    }
}

#[async_trait]
impl ChatReader for Reader {
    async fn rooms(&self) -> std::result::Result<Vec<LocalRoom>, ApiFailure> {
        *self.discovered.lock().unwrap() = true;
        Ok(vec![self.room.lock().unwrap().clone()])
    }
    async fn read(
        &self,
        _profile: &str,
        _room_id: &str,
        limit: usize,
    ) -> std::result::Result<ChatContext, ApiFailure> {
        if self.require_discovery && !*self.discovered.lock().unwrap() {
            return Err(ApiFailure::new(
                404,
                "room_not_loaded",
                "먼저 방을 불러오세요.",
            ));
        }
        let messages = self.messages.lock().unwrap();
        let tail = messages[messages.len().saturating_sub(limit)..].to_vec();
        Ok(ChatContext {
            room: self.room.lock().unwrap().clone(),
            messages: tail,
            read_at: chrono::Utc::now().fixed_offset(),
        })
    }
}

type Reply = Box<dyn Fn(usize, &str) -> Result<String> + Send + Sync>;

/// 호출한 프로바이더와 마지막 프롬프트를 기록하고, `reply(호출 번호, 프로바이더)`로 응답한다.
pub struct Runner {
    pub calls: Mutex<Vec<String>>,
    pub prompt: Mutex<String>,
    reply: Mutex<Reply>,
}

impl Runner {
    pub fn new(reply: &str) -> Self {
        let reply = reply.to_string();
        Self::with(move |_, _| Ok(reply.clone()))
    }
    pub fn with(reply: impl Fn(usize, &str) -> Result<String> + Send + Sync + 'static) -> Self {
        Self {
            calls: Mutex::new(vec![]),
            prompt: Mutex::new(String::new()),
            reply: Mutex::new(Box::new(reply)),
        }
    }
    pub fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
    pub fn ids(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    pub fn prompt(&self) -> String {
        self.prompt.lock().unwrap().clone()
    }
    pub fn set(&self, reply: impl Fn(usize, &str) -> Result<String> + Send + Sync + 'static) {
        *self.reply.lock().unwrap() = Box::new(reply);
    }
    pub fn set_text(&self, text: String) {
        self.set(move |_, _| Ok(text.clone()));
    }
}

#[async_trait]
impl AiRunner for Runner {
    async fn run(
        &self,
        provider: &AiProviderSettings,
        prompt: &str,
        _timeout: i32,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let calls = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(provider.id.clone());
            calls.len()
        };
        *self.prompt.lock().unwrap() = prompt.to_string();
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        (self.reply.lock().unwrap())(calls, &provider.id)
    }
}
