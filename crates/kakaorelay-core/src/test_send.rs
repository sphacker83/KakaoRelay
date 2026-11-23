//! 원본: `TestSend.cs`(`TestSendRequest`, `TestSendReceipt`, `TestSender`). 텍스트 발송 상태 기계와 원장.
//!
//! 순서: 원장 저장 → 대상 확인 → 초안 검사 → `inputting` 저장 → 입력 → 지연 또는 되읽기 → 대상 재확인
//! → `dispatching` 저장 → Enter → `needs-review` 저장 → 확인. 되돌릴 수 없는 Enter 직전에 반드시 저장한다.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Local};
use kakaorelay_ai::json;
use kakaorelay_ai::text::is_blank;
use kakaorelay_ai::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::fs::{SendLock, write_atomic};
use crate::ids::valid_request_id;
use crate::platform::{SendTarget, TextTransport, TransportError, same_text};
use crate::support::now;

/// 입력창을 읽을 수 없는 OS에서 되읽기 대신 쓰는 대기(사용자 결정: 지연이 없으면 1000ms).
pub const UNREADABLE_DRAFT_DELAY_MS: i32 = 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct TestSendRequest {
    pub request_id: String,
    pub recipient: String,
    pub message: String,
    pub expected_process_id: i32,
    pub expected_window_handle: String,
    pub expires_at: DateTime<FixedOffset>,
    pub verified_empty_placeholder: bool,
    pub continuation_of: Option<String>,
    /// API 텍스트 발송은 입력 내용을 되읽지 않고 이 시간만큼 기다린다. `None`은 수동 발송 흐름이다.
    pub send_delay_ms: Option<i32>,
    /// 로컬 DB 방 ID. 원본에는 없고 맥 어댑터의 DB 확인에 쓴다. 지문에 포함하지 않는다.
    pub room_id: Option<String>,
}

impl TestSendRequest {
    pub fn new(
        request_id: impl Into<String>,
        recipient: impl Into<String>,
        message: impl Into<String>,
        expected_process_id: i32,
        expected_window_handle: impl Into<String>,
        expires_at: DateTime<FixedOffset>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            recipient: recipient.into(),
            message: message.into(),
            expected_process_id,
            expected_window_handle: expected_window_handle.into(),
            expires_at,
            verified_empty_placeholder: false,
            continuation_of: None,
            send_delay_ms: None,
            room_id: None,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if !valid_request_id(&self.request_id) {
            return Err(Error::argument("Invalid request ID"));
        }
        if is_blank(&self.recipient) || is_blank(&self.message) || self.message.contains('\0') {
            return Err(Error::argument("Invalid recipient or message"));
        }
        if self.expected_process_id <= 0 || !valid_handle(&self.expected_window_handle) {
            return Err(Error::argument("Missing observed window identity"));
        }
        let current = now();
        if self.expires_at <= current || self.expires_at > current + chrono::Duration::minutes(15) {
            return Err(Error::argument(
                "Test request must expire within 15 minutes",
            ));
        }
        if let Some(previous) = &self.continuation_of
            && (!valid_request_id(previous) || *previous == self.request_id)
        {
            return Err(Error::argument("Invalid continuation ID"));
        }
        if self
            .send_delay_ms
            .is_some_and(|d| !(0..=30000).contains(&d))
        {
            return Err(Error::argument(
                "Send delay must be between 0 and 30000 milliseconds",
            ));
        }
        Ok(())
    }

    /// 골든 `testSendFingerprint`. 프로세스 ID와 창 핸들이 들어가므로 재시작 사이 동일성은 보장되지 않는다.
    pub fn fingerprint(&self) -> String {
        json::hash(&json::default(&json!({
            "Recipient": self.recipient,
            "Message": self.message,
            "ExpectedProcessId": self.expected_process_id,
            "ExpectedWindowHandle": self.expected_window_handle,
            "VerifiedEmptyPlaceholder": self.verified_empty_placeholder,
        })))
    }
}

/// 원본 정규식 `\A0x[0-9a-fA-F]{1,16}\z`.
fn valid_handle(handle: &str) -> bool {
    handle
        .strip_prefix("0x")
        .is_some_and(|h| (1..=16).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

impl SendTarget for TestSendRequest {
    fn request_id(&self) -> &str {
        &self.request_id
    }
    fn recipient(&self) -> &str {
        &self.recipient
    }
    fn expected_process_id(&self) -> i32 {
        self.expected_process_id
    }
    fn expected_window_handle(&self) -> &str {
        &self.expected_window_handle
    }
    fn message(&self) -> &str {
        &self.message
    }
    fn room_id(&self) -> Option<&str> {
        self.room_id.as_deref()
    }
    fn verified_empty_placeholder(&self) -> bool {
        self.verified_empty_placeholder
    }
    fn continuation_of(&self) -> Option<&str> {
        self.continuation_of.as_deref()
    }
    fn send_delay_ms(&self) -> Option<u32> {
        self.send_delay_ms.and_then(|d| u32::try_from(d).ok())
    }
}

/// 원장 영수증. `ReportStore.JsonOptions`(camelCase, 들여쓰기, 한글 비이스케이프)로 저장하며 필드 순서가 원본과 같다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TestSendReceipt {
    pub request_id: String,
    pub recipient: String,
    pub fingerprint: String,
    pub status: String,
    pub detail: String,
    #[serde(with = "kakaorelay_ai::json::time")]
    pub started_at: DateTime<FixedOffset>,
    #[serde(with = "kakaorelay_ai::json::time::option")]
    pub finished_at: Option<DateTime<FixedOffset>>,
    pub enter_posted: bool,
    pub input_response_timed_out: bool,
    pub send_delay_ms: Option<i32>,
    pub kind: String,
    pub attachment_queued: bool,
    pub input_cleared: Option<bool>,
    pub foreground_before: String,
    pub foreground_after: String,
}

impl Default for TestSendReceipt {
    fn default() -> Self {
        Self {
            request_id: String::new(),
            recipient: String::new(),
            fingerprint: String::new(),
            status: "prepared".into(),
            detail: String::new(),
            started_at: now(),
            finished_at: None,
            enter_posted: false,
            input_response_timed_out: false,
            send_delay_ms: None,
            kind: "text".into(),
            attachment_queued: false,
            input_cleared: None,
            foreground_before: String::new(),
            foreground_after: String::new(),
        }
    }
}

impl TestSendReceipt {
    /// Enter를 한 번 요청했고 그 뒤 처리 흔적까지 확인한 발송. 자동 발송의 후속 단계는 이 경우에만 이어진다.
    pub fn confirmed(&self) -> bool {
        self.enter_posted && self.input_cleared == Some(true)
    }

    pub fn display_time(&self) -> String {
        self.started_at
            .with_timezone(&Local)
            .format("%m.%d %H:%M:%S")
            .to_string()
    }

    pub fn display_status(&self) -> &'static str {
        match self.status.as_str() {
            "observed-in-chat" => "대화 확인 완료",
            "needs-review" => "대화 확인 필요",
            "blocked-before-input" => "입력 전 중단",
            "continued-after-visual-review" => "후속 요청으로 처리",
            _ => "확인 필요 · 작업 기록 미완료",
        }
    }

    pub fn to_json(&self) -> String {
        json::indented(self)
    }

    pub fn from_json(text: &str) -> Result<Self> {
        Ok(serde_json::from_str(text)?)
    }
}

pub fn default_ledger() -> PathBuf {
    kakaorelay_ai::paths::app_data().join("test-sends")
}

pub fn receipt_path(ledger: &Path, request_id: &str) -> PathBuf {
    ledger.join(format!("{request_id}.json"))
}

/// 모든 상태 전이에서 `.tmp`(WriteThrough, flush) → 이동.
pub(crate) fn save(path: &Path, receipt: &TestSendReceipt) -> Result<()> {
    write_atomic(path, receipt.to_json().as_bytes(), true)
}

pub(crate) fn load(path: &Path) -> Result<TestSendReceipt> {
    TestSendReceipt::from_json(&kakaorelay_ai::text::read_text(path)?)
}

/// 원본 `TestSender.SendOnce`. 블로킹 호출이므로 비동기 경로는 `spawn_blocking`에서 부른다.
pub fn send_once(
    request: &TestSendRequest,
    transport: &mut dyn TextTransport,
    ledger: &Path,
) -> Result<TestSendReceipt> {
    request.validate()?;
    let _gate = SendLock::acquire(ledger, "Another test send is in progress")?;
    let path = receipt_path(ledger, &request.request_id);
    if path.exists() {
        let existing = load(&path)?;
        if existing.fingerprint != request.fingerprint() {
            return Err(Error::invalid_operation(
                "Request ID conflicts with an earlier request",
            ));
        }
        // 중단된 요청을 포함해 이미 기록된 요청은 절대 다시 실행하지 않는다.
        return Ok(existing);
    }
    let mut receipt = TestSendReceipt {
        request_id: request.request_id.clone(),
        recipient: request.recipient.clone(),
        fingerprint: request.fingerprint(),
        send_delay_ms: request.send_delay_ms,
        ..Default::default()
    };
    save(&path, &receipt)?;
    let mut input_attempted = false;
    if let Err(error) = dispatch(
        request,
        transport,
        ledger,
        &path,
        &mut receipt,
        &mut input_attempted,
    ) {
        receipt.status = if input_attempted {
            "needs-review"
        } else {
            "blocked-before-input"
        }
        .into();
        receipt.detail = error.to_string();
    }
    receipt.foreground_after = transport.foreground_window();
    receipt.finished_at = Some(now());
    save(&path, &receipt)?;
    Ok(receipt)
}

fn dispatch(
    request: &TestSendRequest,
    transport: &mut dyn TextTransport,
    ledger: &Path,
    path: &Path,
    receipt: &mut TestSendReceipt,
    input_attempted: &mut bool,
) -> Result<()> {
    transport.validate_target(request)?;
    receipt.foreground_before = transport.foreground_window();
    let initial = transport.read_draft()?;
    let readable = initial.is_some();
    match &initial {
        Some(draft) if !request.accepts_initial_draft(draft) => {
            return Err(Error::invalid_operation(
                "Input contains a different draft; nothing was overwritten",
            ));
        }
        // 이어 보내기는 같은 초안이 남아 있음을 확인해야만 안전하다. 읽을 수 없으면 중복 입력을 막으려고 중단한다.
        None if request.continuation_of.is_some() => {
            return Err(Error::invalid_operation(
                "입력창을 읽을 수 없어 이전 요청을 이어 보낼 수 없습니다.",
            ));
        }
        _ => {}
    }
    if let Some(previous_id) = &request.continuation_of {
        let previous_path = receipt_path(ledger, previous_id);
        let mut previous = load(&previous_path)?;
        if previous.status != "needs-review"
            || previous.fingerprint != receipt.fingerprint
            || previous.input_cleared != Some(false)
            || previous.finished_at.is_none()
        {
            return Err(Error::invalid_operation(
                "The previous request cannot be continued",
            ));
        }
        previous.status = "continued-after-visual-review".into();
        previous.detail = format!(
            "Unsent identical draft and disabled send button were visually checked. Continued by {}.",
            request.request_id
        );
        save(&previous_path, &previous)?;
    }
    receipt.status = "inputting".into();
    save(path, receipt)?;
    *input_attempted = true;
    match transport.write_draft(&request.message) {
        Ok(()) => {}
        Err(TransportError::InputTimeout(_)) if request.send_delay_ms.is_some() => {
            receipt.input_response_timed_out = true;
            save(path, receipt)?;
        }
        Err(error) => return Err(error.into()),
    }
    let mut unreadable_note = None;
    if let Some(delay) = request.send_delay_ms {
        sleep_ms(delay);
        if !readable {
            unreadable_note =
                Some("입력창을 읽을 수 없어 기존 초안 검사를 생략했습니다.".to_string());
        }
    } else if readable {
        let back = transport.read_draft()?;
        if !back.is_some_and(|b| same_text(&b, &request.message)) {
            return Err(Error::invalid_operation(
                "Input read-back differs from the requested message",
            ));
        }
    } else {
        sleep_ms(UNREADABLE_DRAFT_DELAY_MS);
        receipt.send_delay_ms = Some(UNREADABLE_DRAFT_DELAY_MS);
        unreadable_note = Some(format!(
            "입력창을 읽을 수 없어 기존 초안 검사와 되읽기를 생략하고 {UNREADABLE_DRAFT_DELAY_MS}ms 대기했습니다."
        ));
    }
    transport.validate_target(request)?;
    receipt.status = "dispatching".into();
    save(path, receipt)?; // 되돌릴 수 없는 Enter 요청 전에 저장한다.
    transport.post_enter()?;
    receipt.enter_posted = true;
    receipt.status = "needs-review".into();
    receipt.detail = "Enter was queued once. Verify the new outgoing bubble; do not resend.".into();
    if let Some(note) = unreadable_note {
        receipt.detail = format!("{} {note}", receipt.detail);
    }
    save(path, receipt)?;
    // 입력창 비움·DB 반영은 처리 흔적일 뿐 상대 수신의 증거가 아니다.
    receipt.input_cleared = Some(transport.confirm_after_enter(request)?);
    Ok(())
}

fn sleep_ms(delay: i32) {
    std::thread::sleep(Duration::from_millis(u64::try_from(delay).unwrap_or(0)));
}

pub fn record_observed(
    request_id: &str,
    ledger: &Path,
    expected_fingerprint: Option<&str>,
) -> Result<()> {
    if !valid_request_id(request_id) {
        return Err(Error::argument("Invalid request ID"));
    }
    let _gate = SendLock::acquire(ledger, "Another operation is in progress")?;
    let path = receipt_path(ledger, request_id);
    let mut receipt = load(&path)?;
    if expected_fingerprint.is_some_and(|f| receipt.fingerprint != f)
        || receipt.status != "needs-review"
        || !receipt.enter_posted
    {
        return Err(Error::invalid_operation("No pending send to verify"));
    }
    receipt.status = "observed-in-chat".into();
    receipt.detail = "The new outgoing bubble was visually confirmed in the recipient conversation. Recipient receipt/read status is not inferred.".into();
    save(&path, &receipt)
}

/// 원장 루트의 `*.json` 중 파일 이름과 `requestId`가 같은 영수증만 `startedAt` 내림차순으로 돌려준다. 손상된 파일은 건너뛴다.
pub fn read_history(ledger: &Path) -> Result<Vec<TestSendReceipt>> {
    let mut result = vec![];
    if !ledger.is_dir() {
        return Ok(result);
    }
    for entry in std::fs::read_dir(ledger)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") || !path.is_file() {
            continue;
        }
        let Ok(item) = load(&path) else { continue };
        if path.file_stem().and_then(|s| s.to_str()) == Some(item.request_id.as_str()) {
            result.push(item);
        }
    }
    result.sort_by_key(|receipt| std::cmp::Reverse(receipt.started_at));
    Ok(result)
}
