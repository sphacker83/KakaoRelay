//! 원본 러너(`tests/KakaoRelay.Tests/Program.cs`)의 `Check(condition, description)` 대응과 공용 가짜 객체.

use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, FixedOffset};
use kakaorelay_core::platform::{ImageTransport, SendTarget, TextTransport, TransportError};

pub struct Check {
    count: AtomicUsize,
}

impl Check {
    pub fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
        }
    }
    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
    #[track_caller]
    pub fn check(&self, condition: bool, description: &str) {
        if !condition {
            panic!("FAIL {description}");
        }
        self.count.fetch_add(1, Ordering::SeqCst);
        println!("PASS {description}");
    }
}

pub const PNG: [u8; 68] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0,
    0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 31, 0, 3, 3, 2, 0,
    239, 154, 27, 91, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

pub fn now() -> DateTime<FixedOffset> {
    chrono::Local::now().fixed_offset()
}

pub fn in_minutes(minutes: i64) -> DateTime<FixedOffset> {
    now() + chrono::Duration::minutes(minutes)
}

/// 원본 테스트의 `FakeTransport`. Enter 이후 확인은 원본의 입력창 비움 검사(`IsEmptyDraft(ReadDraft())`)를 한 번 한다.
#[derive(Default)]
pub struct FakeText {
    pub draft: String,
    pub reject_identity: bool,
    pub throw_after_posting: bool,
    pub keep_draft_after_posting: bool,
    pub writes: usize,
    pub posts: usize,
}

impl FakeText {
    pub fn with_draft(draft: &str) -> Self {
        Self {
            draft: draft.into(),
            ..Default::default()
        }
    }
}

impl TextTransport for FakeText {
    fn foreground_window(&self) -> String {
        "0x999".into()
    }
    fn validate_target(&mut self, _: &dyn SendTarget) -> Result<(), TransportError> {
        if self.reject_identity {
            return Err(TransportError::Failed("Wrong target".into()));
        }
        Ok(())
    }
    fn read_draft(&mut self) -> Result<Option<String>, TransportError> {
        Ok(Some(self.draft.clone()))
    }
    fn write_draft(&mut self, message: &str) -> Result<(), TransportError> {
        self.writes += 1;
        self.draft = message.into();
        Ok(())
    }
    fn post_enter(&mut self) -> Result<(), TransportError> {
        self.posts += 1;
        if !self.keep_draft_after_posting {
            self.draft.clear();
        }
        if self.throw_after_posting {
            return Err(TransportError::Failed("Unknown outcome".into()));
        }
        Ok(())
    }
    fn confirm_after_enter(&mut self, target: &dyn SendTarget) -> Result<bool, TransportError> {
        Ok(target.is_empty_draft(&self.draft))
    }
}

/// 원본 `ApiChecks.FakeImageTransport`/`PersonaChecks.Transport`.
#[derive(Default)]
pub struct FakeImage {
    pub attachments: usize,
    pub fail: bool,
    pub attached: Option<std::path::PathBuf>,
}

impl ImageTransport for FakeImage {
    fn validate(&mut self, _: &dyn SendTarget) -> Result<(), TransportError> {
        Ok(())
    }
    fn attach(&mut self, path: &std::path::Path) -> Result<(), TransportError> {
        if !path.exists() {
            return Err(TransportError::Failed("Missing staged file".into()));
        }
        self.attachments += 1;
        self.attached = Some(path.to_path_buf());
        if self.fail {
            return Err(TransportError::Failed("uncertain attachment".into()));
        }
        Ok(())
    }
    fn confirm_preview(&mut self) -> Result<bool, TransportError> {
        Ok(true)
    }
}

/// 발송 어댑터 팩토리가 매번 새 상자를 만들어도 같은 가짜 상태를 보도록 감싼다(원본 `() => fake` 대응).
pub struct Shared<T>(pub std::sync::Arc<std::sync::Mutex<T>>);

impl<T: TextTransport> TextTransport for Shared<T> {
    fn foreground_window(&self) -> String {
        self.0.lock().unwrap().foreground_window()
    }
    fn validate_target(&mut self, t: &dyn SendTarget) -> Result<(), TransportError> {
        self.0.lock().unwrap().validate_target(t)
    }
    fn read_draft(&mut self) -> Result<Option<String>, TransportError> {
        self.0.lock().unwrap().read_draft()
    }
    fn write_draft(&mut self, m: &str) -> Result<(), TransportError> {
        self.0.lock().unwrap().write_draft(m)
    }
    fn post_enter(&mut self) -> Result<(), TransportError> {
        self.0.lock().unwrap().post_enter()
    }
    fn confirm_after_enter(&mut self, t: &dyn SendTarget) -> Result<bool, TransportError> {
        self.0.lock().unwrap().confirm_after_enter(t)
    }
}

impl<T: ImageTransport> ImageTransport for Shared<T> {
    fn validate(&mut self, t: &dyn SendTarget) -> Result<(), TransportError> {
        self.0.lock().unwrap().validate(t)
    }
    fn attach(&mut self, p: &std::path::Path) -> Result<(), TransportError> {
        self.0.lock().unwrap().attach(p)
    }
    fn confirm_preview(&mut self) -> Result<bool, TransportError> {
        self.0.lock().unwrap().confirm_preview()
    }
}

pub fn shared<T>(value: T) -> std::sync::Arc<std::sync::Mutex<T>> {
    std::sync::Arc::new(std::sync::Mutex::new(value))
}

/// 로컬 HTTP 시험 클라이언트. 응답은 (상태 코드, 본문)이다.
pub struct Http {
    pub base: String,
    pub token: Option<String>,
    pub origin: bool,
    client: reqwest::Client,
}

impl Http {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.into(),
            token: None,
            origin: false,
            client: reqwest::Client::new(),
        }
    }
    async fn send(&self, builder: reqwest::RequestBuilder) -> (u16, String) {
        let mut builder = builder;
        if let Some(token) = &self.token {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        if self.origin {
            builder = builder.header("Origin", "https://example.org");
        }
        let response = builder.send().await.expect("HTTP 요청");
        let status = response.status().as_u16();
        (status, response.text().await.unwrap_or_default())
    }
    pub async fn get(&self, path: &str) -> (u16, String) {
        self.send(self.client.get(format!("{}{path}", self.base)))
            .await
    }
    pub async fn post(&self, path: &str, body: &str) -> (u16, String) {
        self.send(
            self.client
                .post(format!("{}{path}", self.base))
                .header("Content-Type", "application/json; charset=utf-8")
                .body(body.to_string()),
        )
        .await
    }
    pub async fn put(&self, path: &str, body: &str) -> (u16, String) {
        self.send(
            self.client
                .put(format!("{}{path}", self.base))
                .header("Content-Type", "application/json; charset=utf-8")
                .body(body.to_string()),
        )
        .await
    }
}

pub fn parse(body: &str) -> serde_json::Value {
    serde_json::from_str(body).expect("JSON 응답")
}
