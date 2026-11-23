//! 원본에 없는 검사. 입력창을 읽을 수 없는 OS(`read_draft`가 `None`)의 상태 기계 분기(작업지시서 "발송 상태 기계").

use std::path::Path;

use kakaorelay_core::platform::{SendTarget, TextTransport, TransportError};
use kakaorelay_core::test_send::{self, TestSendRequest};

use crate::support::{Check, in_minutes};

#[derive(Default)]
struct Blind {
    writes: usize,
    posts: usize,
    written_at: Option<std::time::Instant>,
    waited_ms: u128,
}

impl TextTransport for Blind {
    fn foreground_window(&self) -> String {
        "0x1".into()
    }
    fn validate_target(&mut self, _: &dyn SendTarget) -> Result<(), TransportError> {
        Ok(())
    }
    fn read_draft(&mut self) -> Result<Option<String>, TransportError> {
        Ok(None)
    }
    fn write_draft(&mut self, _: &str) -> Result<(), TransportError> {
        self.writes += 1;
        self.written_at = Some(std::time::Instant::now());
        Ok(())
    }
    fn post_enter(&mut self) -> Result<(), TransportError> {
        self.posts += 1;
        self.waited_ms = self.written_at.map_or(0, |t| t.elapsed().as_millis());
        Ok(())
    }
    fn confirm_after_enter(&mut self, _: &dyn SendTarget) -> Result<bool, TransportError> {
        Ok(true)
    }
}

pub fn run(root: &Path, check: &Check) {
    let ledger = root.join("unreadable");
    let request = TestSendRequest::new("blind-manual", "방", "본문", 1, "0x1", in_minutes(5));
    let mut blind = Blind::default();
    let receipt = test_send::send_once(&request, &mut blind, &ledger).unwrap();
    check.check(
        blind.posts == 1
            && blind.waited_ms >= 950
            && receipt.send_delay_ms == Some(1000)
            && receipt.detail.contains("생략")
            && receipt.confirmed(),
        "Unreadable editors skip draft checks, wait the default 1000 ms instead of read-back and record it",
    );
    let api = TestSendRequest {
        send_delay_ms: Some(0),
        ..TestSendRequest::new("blind-api", "방", "본문", 1, "0x1", in_minutes(5))
    };
    let mut blind = Blind::default();
    let receipt = test_send::send_once(&api, &mut blind, &ledger).unwrap();
    check.check(
        blind.posts == 1 && receipt.send_delay_ms == Some(0) && receipt.detail.contains("생략"),
        "An explicit API delay is kept when the editor cannot be read",
    );
    let continued = TestSendRequest {
        continuation_of: Some("blind-manual".into()),
        ..TestSendRequest::new("blind-continue", "방", "본문", 1, "0x1", in_minutes(5))
    };
    let mut blind = Blind::default();
    let receipt = test_send::send_once(&continued, &mut blind, &ledger).unwrap();
    check.check(
        blind.writes == 0 && blind.posts == 0 && receipt.status == "blocked-before-input",
        "Continuations are refused when the pending draft cannot be read",
    );
}
