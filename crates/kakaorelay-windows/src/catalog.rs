//! 창 스냅숏과 열린 대화방 선택 규칙. 원본: `DiagnosticReport.cs`의 스냅숏 타입, `ConversationCatalog.SelectTargets`.
//! 스냅숏 JSON은 기존 진단 보고서(`ReportStore.JsonOptions`, camelCase)와 같은 필드·순서로 직렬화한다.

use kakaorelay_core::platform::ConversationTarget;
use std::cmp::Ordering;

pub const CONVERSATION_CLASS: &str = "EVA_Window_Dblclk";
pub const RICH_EDIT_CLASS: &str = "RICHEDIT50W";
pub const MESSAGE_EDITOR_ID: i32 = 1006;

/// 스냅숏 타입은 진단 보고서와 함께 core로 옮겼다. 기존 경로를 유지하려고 다시 내보낸다.
pub use kakaorelay_core::diagnostics::{
    AutomationControlSnapshot, NativeControlSnapshot, ProcessSnapshot, WindowSnapshot,
};

/// 원본의 `StringComparer.CurrentCulture` 정렬 근사. 대소문자를 먼저 무시해 비교하고, 같으면 서수 비교한다.
pub(crate) fn culture_order(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// 보이고·최소화되지 않고·활성인 대화창 중 메시지 입력창(RichEdit, ID 1006)이 정확히 하나인 창만 고른다.
/// 같은 제목의 창이 전체 목록(숨김 포함)에 둘 이상이면 `ambiguous`다. 원본 `ConversationCatalog.SelectTargets`.
pub fn select_targets(
    windows: &[WindowSnapshot],
    is_message_editor: impl Fn(&NativeControlSnapshot) -> bool,
) -> Vec<ConversationTarget> {
    let mut targets: Vec<ConversationTarget> = windows
        .iter()
        .filter(|w| {
            w.visible
                && !w.minimized
                && w.enabled
                && w.class_name == CONVERSATION_CLASS
                && !w.title.trim().is_empty()
                && w.native_children
                    .iter()
                    .filter(|c| {
                        c.visible
                            && c.enabled
                            && c.class_name == RICH_EDIT_CLASS
                            && is_message_editor(c)
                    })
                    .count()
                    == 1
        })
        .map(|w| ConversationTarget {
            title: w.title.clone(),
            process_id: w.process_id,
            handle: w.handle.clone(),
            ambiguous: windows
                .iter()
                .filter(|other| other.title == w.title)
                .count()
                != 1,
            room_id: None,
        })
        .collect();
    targets.sort_by(|a, b| culture_order(&a.title, &b.title));
    targets
}
