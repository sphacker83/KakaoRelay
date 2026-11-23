//! OS 어댑터 계약. 원본 인터페이스: `ITestSendTransport`(TestSend.cs), `IImageSendTransport`(ImageSender.cs),
//! `ConversationCatalog.Scan`(ConversationCatalog.cs), `IChatReader`(ChatModels.cs).
//!
//! 발송 상태 기계(`TestSender.SendOnce`, `ImageSender.SendOnce`)는 Phase 2에서 이 crate에 OS 공통으로 이식하며,
//! 어댑터는 아래 단계만 제공한다. 상태 기계는 원장 기록·중복 방지·되돌릴 수 없는 단계 직전 저장을 책임진다.
//!
//! OS별 의미 차이(사용자 결정 2026-11-23, `dev/active/rust-port/rust-port-macos-research.md`):
//! - Windows: 대상 창에 `EM_REPLACESEL`로 입력하고 Enter를 게시한다. 포커스·클립보드를 쓰지 않으며 입력창을 읽어 확인한다.
//! - macOS: 입력창이 접근성 API에 노출되지 않는다. '창' 메뉴로 대상 대화창을 전면화하고 합성 키 입력과 Return으로 보낸다.
//!   입력창을 읽을 수 없어 `read_draft`는 `None`이다. Enter 이후 확인은 로컬 DB의 새 발신 메시지로 한다.

pub use kakaorelay_ai::{ChatContext, ChatReader, LocalMessage, LocalRoom};

use serde::{Deserialize, Serialize};

/// 열린 대화방(발송 대상). 원본 `ConversationTarget`, API `GET /v1/rooms` 응답 항목.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTarget {
    pub title: String,
    pub process_id: i32,
    /// `0x` + 16진수(최대 16자리). 원장 지문에 포함된다(`TestSendRequest.Fingerprint`).
    /// Windows는 HWND, macOS는 해당 대화창의 CGWindowID를 쓴다.
    pub handle: String,
    /// 같은 제목의 열린 방이 둘 이상이면 true. 이 경우 발송 대상으로 선택할 수 없다.
    pub ambiguous: bool,
    /// 로컬 DB의 방 ID(chatId). 아는 어댑터만 채우며 발송 요청의 `SendTarget::room_id`로 넘긴다.
    /// API·화면 응답 형식은 원본과 같게 유지하려고 직렬화하지 않는다.
    #[serde(skip)]
    pub room_id: Option<String>,
}

impl ConversationTarget {
    pub fn new(
        title: impl Into<String>,
        process_id: i32,
        handle: impl Into<String>,
        ambiguous: bool,
    ) -> Self {
        Self {
            title: title.into(),
            process_id,
            handle: handle.into(),
            ambiguous,
            room_id: None,
        }
    }

    pub fn hint(&self) -> &'static str {
        if self.ambiguous {
            "같은 이름의 방이 여러 개 있어 선택할 수 없습니다"
        } else {
            "열린 대화방"
        }
    }
    pub fn selectable(&self) -> bool {
        !self.ambiguous
    }
}

/// 어댑터 오류. 상태 기계는 메시지를 원장 `detail`에 그대로 기록한다.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// 텍스트 입력 응답 시간 초과. 지연 발송(API) 경로에서만 계속 진행할 수 있다(원본 `TextInputTimeoutException`).
    #[error("Text input response timed out: {0}")]
    InputTimeout(String),
    /// OS 권한 부족(손쉬운 사용, 전체 디스크 접근 등). API는 `code`로 409 응답을 만든다.
    #[error("{detail}")]
    PermissionDenied { code: String, detail: String },
    /// 대상 확인 실패, 잠긴 화면, 다른 초안 등 진행하면 안 되는 그 밖의 실패.
    #[error("{0}")]
    Failed(String),
}

/// 원장 요청에서 어댑터가 확인해야 하는 대상 정보. Phase 2의 `TestSendRequest`가 이 trait을 구현한다.
pub trait SendTarget {
    fn request_id(&self) -> &str;
    fn recipient(&self) -> &str;
    fn expected_process_id(&self) -> i32;
    fn expected_window_handle(&self) -> &str;
    /// 발송할 본문(텍스트) 또는 이미지 파일 식별자(`image:<SHA256 대문자 hex>`).
    fn message(&self) -> &str;
    /// 로컬 DB의 방 ID(chatId). 알면 제목 대신 이 값으로 발송을 확인한다(macOS DB 확인). 원본에는 없는 선택 값이다.
    fn room_id(&self) -> Option<&str> {
        None
    }
    /// 원본 `VerifiedEmptyPlaceholder`. true면 입력창의 정확히 `메시지 입력`을 빈 안내로 취급한다.
    fn verified_empty_placeholder(&self) -> bool {
        false
    }
    /// 원본 `ContinuationOf`. 앞선 `needs-review` 요청의 같은 초안을 이어서 보낼 때의 이전 요청 ID.
    fn continuation_of(&self) -> Option<&str> {
        None
    }
    /// 원본 `SendDelayMs`. 값이 있으면(API 경로) 되읽기 대신 지연 후 Enter를 보낸다.
    fn send_delay_ms(&self) -> Option<u32> {
        None
    }
    /// 원본 `TestSendRequest.IsEmptyDraft`.
    fn is_empty_draft(&self, value: &str) -> bool {
        value.is_empty() || (self.verified_empty_placeholder() && value == "메시지 입력")
    }
    /// 원본 `TestSendRequest.AcceptsInitialDraft`.
    fn accepts_initial_draft(&self, value: &str) -> bool {
        match self.continuation_of() {
            None => self.is_empty_draft(value),
            Some(_) => same_text(value, self.message()),
        }
    }
}

/// 원본 `TestSendRequest.SameText`. `\r\n`과 `\r`을 `\n`으로 정규화해 비교한다.
pub fn same_text(first: &str, second: &str) -> bool {
    fn normalize(value: &str) -> String {
        value.replace("\r\n", "\n").replace('\r', "\n")
    }
    normalize(first) == normalize(second)
}

/// 열린 대화방 목록. 원본 `ConversationCatalog.Scan`. 같은 제목 판정은 어댑터가 `ambiguous`로 표시한다.
pub trait RoomCatalog: Send + Sync {
    fn scan(&self) -> Result<Vec<ConversationTarget>, TransportError>;
}

/// 텍스트 발송 단계. 원본 `ITestSendTransport`. 한 번의 발송마다 새 인스턴스를 만든다.
pub trait TextTransport: Send {
    /// 진단용 전면 창 식별자(원장 `foregroundBefore/After`).
    fn foreground_window(&self) -> String;
    /// 프로세스·창·제목이 요청과 같은지, 화면이 잠겨 있지 않은지 확인한다. 각 되돌릴 수 없는 단계 직전에 다시 호출된다.
    fn validate_target(&mut self, target: &dyn SendTarget) -> Result<(), TransportError>;
    /// 입력창의 현재 초안. 읽을 수 없는 OS(macOS)는 `Ok(None)`을 반환하고, 상태 기계는 초안 보호·되읽기 검사를 건너뛴다.
    fn read_draft(&mut self) -> Result<Option<String>, TransportError>;
    fn write_draft(&mut self, message: &str) -> Result<(), TransportError>;
    /// 되돌릴 수 없는 전송 요청. 정확히 한 번만 호출된다.
    fn post_enter(&mut self) -> Result<(), TransportError>;
    /// Enter 이후 처리 확인(원장 `inputCleared`). Windows는 입력창 비움(150ms×8회), macOS는 DB의 새 발신 메시지 확인.
    /// `false`는 "확인하지 못함"이며 실패로 재전송하지 않는다.
    fn confirm_after_enter(&mut self, target: &dyn SendTarget) -> Result<bool, TransportError>;
}

/// 이미지 발송 단계. 원본 `IImageSendTransport`.
/// Windows는 `WM_DROPFILES`, macOS는 클립보드 보관 → 파일 붙여넣기 → 확인 창 Enter → 클립보드 복원.
pub trait ImageTransport: Send {
    fn validate(&mut self, target: &dyn SendTarget) -> Result<(), TransportError>;
    fn attach(&mut self, path: &std::path::Path) -> Result<(), TransportError>;
    /// 미리보기(확인 창) 전송 후 닫힘 또는 DB 확인 결과. 원장 `inputCleared`가 된다.
    fn confirm_preview(&mut self) -> Result<bool, TransportError>;
}

/// OS 어댑터 묶음. 앱은 대상 OS의 구현 하나를 주입한다.
pub trait Platform: Send + Sync {
    fn rooms(&self) -> &dyn RoomCatalog;
    fn reader(&self) -> std::sync::Arc<dyn ChatReader>;
    fn text_transport(&self) -> Box<dyn TextTransport>;
    fn image_transport(&self) -> Box<dyn ImageTransport>;
    /// `capabilities.imageSend.transport` 값. 예: `"WM_DROPFILES + HWND"`, `"clipboard paste + window menu"`.
    fn image_transport_name(&self) -> &'static str;
    /// 어댑터가 스스로 찾은 내 카카오 사용자 ID(십진 문자열). 설정의 '카카오 ID'가 비어 있을 때 채우는 데 쓴다.
    /// 맥은 DB 키 유도에 쓴 ID를 돌려준다. 찾을 방법이 없는 OS는 `None`이다. 블로킹 호출이다.
    fn detected_self_id(&self) -> Option<String> {
        None
    }
}
