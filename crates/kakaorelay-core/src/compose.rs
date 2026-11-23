//! 원본: `ConversationCatalog.cs`의 `ComposeSession`. 한 번 누른 보내기가 같은 요청 ID를 유지하게 한다.

use kakaorelay_ai::{Error, Result};

use crate::platform::ConversationTarget;
use crate::support::now;
use crate::test_send::{TestSendReceipt, TestSendRequest};

#[derive(Debug, Default)]
pub struct ComposeSession {
    request: Option<TestSendRequest>,
}

impl ComposeSession {
    pub fn request(&self) -> Option<&TestSendRequest> {
        self.request.as_ref()
    }

    pub fn submitted(&self) -> bool {
        self.request.is_some()
    }

    pub fn begin(
        &mut self,
        target: Option<&ConversationTarget>,
        message: &str,
        verified_placeholder: bool,
    ) -> Result<TestSendRequest> {
        if self.submitted() {
            return Err(Error::invalid_operation(
                "This compose session was already submitted",
            ));
        }
        let Some(target) = target.filter(|t| t.selectable()) else {
            return Err(Error::invalid_operation(
                "Select an unambiguous conversation",
            ));
        };
        let mut request = TestSendRequest::new(
            format!("app-{}", uuid::Uuid::new_v4().simple()),
            target.title.clone(),
            message,
            target.process_id,
            target.handle.clone(),
            now() + chrono::Duration::minutes(5),
        );
        request.verified_empty_placeholder = verified_placeholder;
        request.room_id = target.room_id.clone();
        request.validate()?;
        self.request = Some(request.clone());
        Ok(request)
    }

    pub fn reset(&mut self) {
        self.request = None;
    }

    /// 처리 흔적까지 확인된 현재 요청이면 비우고 다음 메시지를 허용한다. 불확실하면 같은 요청 ID를 유지한다.
    pub fn complete(&mut self, receipt: &TestSendReceipt) -> bool {
        if self.request.as_ref().map(|r| r.request_id.as_str()) != Some(receipt.request_id.as_str())
            || !receipt.confirmed()
        {
            return false;
        }
        self.reset();
        true
    }
}
