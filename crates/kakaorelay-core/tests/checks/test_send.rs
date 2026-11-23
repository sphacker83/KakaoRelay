//! 원본: `TestSendChecks.cs`(14개), `WorkspaceChecks.cs`의 `ComposeSession`·원장 검사(9개).
//! `WorkspaceChecks`의 `SelectTargets` 검사 3개는 `kakaorelay-windows/tests/selection.rs`에 있다.

use std::path::Path;

use kakaorelay_core::compose::ComposeSession;
use kakaorelay_core::platform::{
    ConversationTarget, SendTarget, TextTransport, TransportError, same_text,
};
use kakaorelay_core::test_send::{
    self, TestSendReceipt, TestSendRequest, read_history, receipt_path, record_observed,
};

use crate::support::{Check, FakeText, in_minutes, now};

fn request(id: &str) -> TestSendRequest {
    TestSendRequest::new(
        id,
        "Test recipient",
        "Test body",
        123,
        "0x123",
        in_minutes(5),
    )
}

pub fn run(root: &Path, check: &Check) {
    let ledger = root.join("send-ledger");
    let mut transport = FakeText::default();
    let once = request("once");
    let receipt = test_send::send_once(&once, &mut transport, &ledger).unwrap();
    check.check(
        transport.posts == 1
            && receipt.status == "needs-review"
            && receipt.input_cleared == Some(true),
        "One Enter request remains unconfirmed even when input clears",
    );
    test_send::send_once(&once, &mut transport, &ledger).unwrap();
    check.check(
        transport.posts == 1 && transport.writes == 1,
        "Repeated request never types or sends again",
    );
    let changed = TestSendRequest {
        message: "Other message".into(),
        ..once.clone()
    };
    let rejected = matches!(
        test_send::send_once(&changed, &mut transport, &ledger),
        Err(kakaorelay_ai::Error::InvalidOperation(_))
    );
    check.check(
        rejected && transport.posts == 1,
        "Changed body cannot reuse an earlier request ID",
    );
    let mut draft = FakeText::with_draft("Unsent user text");
    let blocked = test_send::send_once(&request("draft"), &mut draft, &ledger).unwrap();
    check.check(
        blocked.status == "blocked-before-input"
            && draft.writes == 0
            && draft.posts == 0
            && draft.draft == "Unsent user text",
        "Existing drafts remain untouched",
    );
    let mut wrong_target = FakeText {
        reject_identity: true,
        ..Default::default()
    };
    test_send::send_once(&request("identity"), &mut wrong_target, &ledger).unwrap();
    check.check(
        wrong_target.writes == 0 && wrong_target.posts == 0,
        "Changed or ambiguous target prevents all input",
    );
    let mut uncertain = FakeText {
        throw_after_posting: true,
        ..Default::default()
    };
    let failed = request("uncertain");
    let unknown = test_send::send_once(&failed, &mut uncertain, &ledger).unwrap();
    test_send::send_once(&failed, &mut uncertain, &ledger).unwrap();
    check.check(
        unknown.status == "needs-review" && uncertain.posts == 1,
        "Uncertain Enter outcome never triggers retry",
    );
    let interrupted = request("interrupted");
    let crashed = TestSendReceipt {
        request_id: interrupted.request_id.clone(),
        fingerprint: interrupted.fingerprint(),
        status: "dispatching".into(),
        ..Default::default()
    };
    std::fs::write(
        receipt_path(&ledger, &interrupted.request_id),
        crashed.to_json(),
    )
    .unwrap();
    let mut after_crash = FakeText::default();
    test_send::send_once(&interrupted, &mut after_crash, &ledger).unwrap();
    check.check(
        after_crash.writes == 0 && after_crash.posts == 0,
        "A persisted dispatching request is never replayed after a crash",
    );
    let expired = TestSendRequest {
        expires_at: now() - chrono::Duration::seconds(1),
        ..request("expired")
    };
    check.check(
        matches!(
            test_send::send_once(&expired, &mut FakeText::default(), &ledger),
            Err(kakaorelay_ai::Error::Argument(_))
        ),
        "Expired target bindings cannot send",
    );
    let mut cue = FakeText::with_draft("메시지 입력");
    test_send::send_once(&request("unverified-cue"), &mut cue, &ledger).unwrap();
    check.check(
        cue.posts == 0 && cue.writes == 0,
        "Placeholder text is not treated as empty without visual verification",
    );
    let verified = TestSendRequest {
        verified_empty_placeholder: true,
        ..request("verified-cue")
    };
    test_send::send_once(&verified, &mut cue, &ledger).unwrap();
    check.check(
        cue.posts == 1,
        "Visually verified gray placeholder can be replaced once",
    );
    let mut cue_with_draft = FakeText::with_draft("작성 중인 실제 초안");
    let cue_request = TestSendRequest {
        verified_empty_placeholder: true,
        ..request("cue-with-draft")
    };
    test_send::send_once(&cue_request, &mut cue_with_draft, &ledger).unwrap();
    check.check(
        cue_with_draft.writes == 0 && cue_with_draft.posts == 0,
        "Placeholder exception never accepts a different draft",
    );
    let pending = request("pending-draft");
    let mut pending_transport = FakeText {
        keep_draft_after_posting: true,
        ..Default::default()
    };
    test_send::send_once(&pending, &mut pending_transport, &ledger).unwrap();
    let continuation_request = TestSendRequest {
        continuation_of: Some(pending.request_id.clone()),
        ..request("continue-draft")
    };
    let mut continuation = FakeText::with_draft(&pending.message);
    let continued =
        test_send::send_once(&continuation_request, &mut continuation, &ledger).unwrap();
    check.check(
        continuation.posts == 1 && continued.input_cleared == Some(true),
        "Explicit continuation accepts only the prior identical pending draft",
    );
    let mut second = FakeText::with_draft(&pending.message);
    let again = TestSendRequest {
        continuation_of: Some(pending.request_id.clone()),
        ..request("continue-again")
    };
    test_send::send_once(&again, &mut second, &ledger).unwrap();
    check.check(
        second.posts == 0 && second.writes == 0,
        "One reviewed predecessor cannot be continued twice",
    );
    record_observed(
        &continuation_request.request_id,
        &ledger,
        Some(&continuation_request.fingerprint()),
    )
    .unwrap();
    let observed = TestSendReceipt::from_json(
        &std::fs::read_to_string(receipt_path(&ledger, &continuation_request.request_id)).unwrap(),
    )
    .unwrap();
    check.check(
        observed.status == "observed-in-chat",
        "Visual confirmation is recorded separately from Enter queueing",
    );
    workspace(&ledger, check);
}

fn workspace(ledger: &Path, check: &Check) {
    let target = ConversationTarget {
        title: "Recipient".into(),
        process_id: 12,
        handle: "0x10".into(),
        ambiguous: false,
        room_id: None,
    };
    let mut compose = ComposeSession::default();
    let rejected = compose.begin(None, "hello", false).is_err();
    check.check(
        rejected && !compose.submitted(),
        "Empty recipient cannot create a send request",
    );
    let first = compose
        .begin(Some(&target), "첫 줄\r\n둘째 줄", false)
        .unwrap();
    let rejected = compose.begin(Some(&target), "another", false).is_err();
    check.check(
        rejected && compose.request() == Some(&first),
        "Repeated send clicks keep the one compose identity",
    );
    compose.reset();
    check.check(
        compose
            .begin(Some(&target), "new", false)
            .unwrap()
            .request_id
            != first.request_id,
        "Explicit new message creates a separate request",
    );
    let next = compose.request().unwrap().request_id.clone();
    let receipt = |id: &str, cleared: bool| TestSendReceipt {
        request_id: id.into(),
        enter_posted: true,
        input_cleared: Some(cleared),
        ..Default::default()
    };
    check.check(
        !compose.complete(&receipt(&next, false)) && compose.submitted(),
        "Uncertain send retains the draft identity for duplicate protection",
    );
    check.check(
        !compose.complete(&receipt("another", true)) && compose.submitted(),
        "Unrelated completion cannot clear the current draft",
    );
    check.check(
        compose.complete(&receipt(&next, true))
            && !compose.submitted()
            && compose
                .begin(Some(&target), "바로 다음 메시지", true)
                .unwrap()
                .request_id
                != next,
        "Processed send immediately permits the next message without a New Message action",
    );
    check.check(
        same_text("첫 줄\r\n둘째 줄", "첫 줄\r둘째 줄") && !same_text("hello ", "hello"),
        "RichEdit newline normalization preserves meaningful spaces",
    );
    std::fs::write(ledger.join("broken.json"), "broken JSON").unwrap();
    let history = read_history(ledger).unwrap();
    check.check(
        history.iter().any(|r| r.status == "observed-in-chat")
            && !history.iter().any(|r| r.request_id == "broken"),
        "Unified history imports earlier receipts and tolerates a damaged file",
    );
    let blocked = history
        .iter()
        .find(|r| r.status == "blocked-before-input")
        .unwrap();
    check.check(
        matches!(
            record_observed(&blocked.request_id, ledger, None),
            Err(kakaorelay_ai::Error::InvalidOperation(_))
        ),
        "History cannot mark an unsent blocked request as observed",
    );
}

/// 지연 발송 중 사용자 키 입력이 섞이는 입력창(맥 실측 2026-11-23).
struct Interleaved(FakeText);

impl TextTransport for Interleaved {
    fn foreground_window(&self) -> String {
        self.0.foreground_window()
    }
    fn validate_target(&mut self, target: &dyn SendTarget) -> Result<(), TransportError> {
        self.0.validate_target(target)
    }
    fn read_draft(&mut self) -> Result<Option<String>, TransportError> {
        self.0.read_draft()
    }
    fn write_draft(&mut self, message: &str) -> Result<(), TransportError> {
        self.0.write_draft(&format!("{message}3"))
    }
    fn post_enter(&mut self) -> Result<(), TransportError> {
        self.0.post_enter()
    }
    fn confirm_after_enter(&mut self, target: &dyn SendTarget) -> Result<bool, TransportError> {
        self.0.confirm_after_enter(target)
    }
    fn verify_after_delay(&self) -> bool {
        true
    }
}

pub fn interleaved_input(root: &Path, check: &Check) {
    let ledger = root.join("interleaved-ledger");
    let mut delayed = request("interleaved");
    delayed.send_delay_ms = Some(0);
    let mut transport = Interleaved(FakeText::default());
    let outcome = test_send::send_once(&delayed, &mut transport, &ledger);
    check.check(
        transport.0.posts == 0 && outcome.map_or(true, |r| !r.enter_posted),
        "A delayed send re-reads the editor before Enter when the transport asks for it and stops if typed keys were mixed in",
    );
}
