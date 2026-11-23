//! 열린 대화방 목록. 원본 `ConversationCatalog.Scan`의 맥 대응.
//!
//! 분류 규칙(카카오톡 26.10.0 실측, 2026-11-23):
//! - '창' 메뉴의 고정 항목(최소화, 확대/축소, 채우기, 친구·채팅·더보기 등)은 `AXIdentifier`가 액션 이름(`miniaturizeAll:`,
//!   `zoomAll:`)이거나 `_NS:<번호>`이거나 없다(구분선).
//! - 열린 창 항목은 AppKit이 창마다 자동으로 추가하며 `AXIdentifier`가 `makeKeyAndOrderFront:`다. 제목이 창 제목이다.
//! - 메인 창(친구·채팅 목록)도 같은 방식으로 나열되므로 제목 `카카오톡`은 대화창에서 제외한다.

use kakaorelay_core::platform::ConversationTarget;

use crate::ax::WindowInfo;

pub const WINDOW_ITEM_IDENTIFIER: &str = "makeKeyAndOrderFront:";
pub const MAIN_WINDOW_TITLE: &str = "카카오톡";
/// CGWindowID를 알 수 없을 때의 handle. 화면 기록 권한이 없으면 창 이름을 읽을 수 없다.
/// 이 handle로는 발송하지 않는다(사용자 결정 2026-11-23 A안: 발송에는 화면 기록 권한 필수).
pub const UNKNOWN_HANDLE: &str = "0x0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    pub title: String,
    pub identifier: Option<String>,
}

pub fn is_conversation_item(entry: &MenuEntry) -> bool {
    entry.identifier.as_deref() == Some(WINDOW_ITEM_IDENTIFIER)
        && !entry.title.trim().is_empty()
        && entry.title != MAIN_WINDOW_TITLE
}

pub fn format_handle(id: u32) -> String {
    format!("0x{id:X}")
}

/// 제목이 같은 카카오톡 일반 창(layer 0)이 정확히 하나면 그 CGWindowID.
pub fn window_handle(title: &str, pid: i32, windows: &[WindowInfo]) -> String {
    let mut matches = windows
        .iter()
        .filter(|w| w.pid == pid && w.layer == 0 && w.name.as_deref() == Some(title));
    match (matches.next(), matches.next()) {
        (Some(window), None) => format_handle(window.id),
        _ => UNKNOWN_HANDLE.to_owned(),
    }
}

/// 화면에 보이는 가장 앞 일반 창(layer 0)의 소유 pid. 창 목록은 앞에서 뒤 순서다.
pub fn frontmost_owner(windows: &[WindowInfo]) -> Option<i32> {
    windows
        .iter()
        .find(|w| w.layer == 0 && w.on_screen)
        .map(|w| w.pid)
}

/// 카카오톡의 가장 앞(z-order 최상위) 일반 창이 대상인지. 이름과 CGWindowID가 모두 같아야 한다.
/// '창' 메뉴 체크 표시는 실측(2026-11-23)에서 전면 대화창과 어긋나 판정에 쓰지 않는다.
pub fn front_is_target(windows: &[WindowInfo], pid: i32, title: &str, handle: &str) -> bool {
    handle != UNKNOWN_HANDLE
        && windows
            .iter()
            .find(|w| w.pid == pid && w.layer == 0 && w.on_screen)
            .is_some_and(|w| w.name.as_deref() == Some(title) && format_handle(w.id) == handle)
}

pub fn select_targets(
    entries: &[MenuEntry],
    pid: i32,
    windows: &[WindowInfo],
) -> Vec<ConversationTarget> {
    let conversations: Vec<&MenuEntry> =
        entries.iter().filter(|e| is_conversation_item(e)).collect();
    let mut targets: Vec<ConversationTarget> = conversations
        .iter()
        .map(|entry| {
            let same = conversations
                .iter()
                .filter(|other| other.title == entry.title)
                .count();
            ConversationTarget {
                title: entry.title.clone(),
                process_id: pid,
                handle: if same == 1 {
                    window_handle(&entry.title, pid, windows)
                } else {
                    UNKNOWN_HANDLE.to_owned()
                },
                ambiguous: same != 1,
                room_id: None,
            }
        })
        .collect();
    targets.sort_by(|a, b| a.title.cmp(&b.title));
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, identifier: Option<&str>) -> MenuEntry {
        MenuEntry {
            title: title.into(),
            identifier: identifier.map(Into::into),
        }
    }

    fn window(id: u32, name: Option<&str>) -> WindowInfo {
        WindowInfo {
            id,
            pid: 7,
            layer: 0,
            name: name.map(Into::into),
            on_screen: true,
        }
    }

    /// 실측한 '창' 메뉴 구조(25개 항목)를 가짜 제목으로 재현한다.
    fn observed_menu() -> Vec<MenuEntry> {
        vec![
            entry("최소화", Some("_NS:127")),
            entry("모두 최소화", Some("miniaturizeAll:")),
            entry("확대/축소", Some("_NS:130")),
            entry("모두 확대/축소", Some("zoomAll:")),
            entry("채우기", Some("_zoomFill:")),
            entry("가운데", Some("_zoomCenter:")),
            entry("전체 화면 시작", Some("toggleFullScreen:")),
            entry("", None),
            entry("이동 및 크기 조절", None),
            entry("전체 화면 타일", None),
            entry("", None),
            entry(
                "스테이지 매니저에서 제거",
                Some("_removeWindowFromStageManagerSet:"),
            ),
            entry("창 닫기", Some("_NS:132")),
            entry("모든 창 닫기", Some("_NS:135")),
            entry("", Some("_NS:137")),
            entry("친구", Some("_NS:138")),
            entry("채팅", Some("_NS:141")),
            entry("더보기", Some("_NS:144")),
            entry("", Some("_NS:147")),
            entry("새 채팅", Some("_NS:148")),
            entry("", Some("_NS:151")),
            entry("모두 앞으로 가져오기", Some("_NS:152")),
            entry("모두 정렬", Some("alternateArrangeInFront:")),
            entry("", None),
            entry("방 가", Some(WINDOW_ITEM_IDENTIFIER)),
        ]
    }

    #[test]
    fn only_window_items_are_conversations() {
        let menu = observed_menu();
        let titles: Vec<&str> = menu
            .iter()
            .filter(|e| is_conversation_item(e))
            .map(|e| e.title.as_str())
            .collect();
        assert_eq!(titles, ["방 가"]);
        assert!(!is_conversation_item(&entry(
            MAIN_WINDOW_TITLE,
            Some(WINDOW_ITEM_IDENTIFIER)
        )));
        assert!(!is_conversation_item(&entry(
            "  ",
            Some(WINDOW_ITEM_IDENTIFIER)
        )));
    }

    #[test]
    fn duplicate_titles_are_ambiguous_and_have_no_handle() {
        let mut menu = observed_menu();
        menu.push(entry("방 나", Some(WINDOW_ITEM_IDENTIFIER)));
        menu.push(entry("방 나", Some(WINDOW_ITEM_IDENTIFIER)));
        let windows = [
            window(0x2A, Some("방 가")),
            window(0x30, Some("방 나")),
            window(0x31, Some("방 나")),
        ];
        let targets = select_targets(&menu, 7, &windows);
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].title, "방 가");
        assert!(!targets[0].ambiguous && targets[0].selectable());
        assert_eq!(targets[0].handle, "0x2A");
        assert!(targets[1].ambiguous && targets[2].ambiguous);
        assert_eq!(targets[1].handle, UNKNOWN_HANDLE);
        assert_eq!(
            targets[1].hint(),
            "같은 이름의 방이 여러 개 있어 선택할 수 없습니다"
        );
    }

    #[test]
    fn front_window_must_match_name_and_id() {
        let other_app = WindowInfo {
            id: 9,
            pid: 8,
            layer: 0,
            name: Some("x".into()),
            on_screen: true,
        };
        let hidden = WindowInfo {
            id: 5,
            pid: 7,
            layer: 0,
            name: Some("방 나".into()),
            on_screen: false,
        };
        let overlay = WindowInfo {
            id: 6,
            pid: 7,
            layer: 3,
            name: None,
            on_screen: true,
        };
        let windows = [
            other_app,
            hidden,
            overlay,
            window(0x2A, Some("방 가")),
            window(0x30, Some("방 나")),
        ];
        assert!(
            front_is_target(&windows, 7, "방 가", "0x2A"),
            "다른 앱·화면 밖·다른 layer 창은 건너뛴다"
        );
        assert!(
            !front_is_target(&windows, 7, "방 나", "0x30"),
            "뒤에 있는 창은 대상이 아니다"
        );
        assert!(
            !front_is_target(&windows, 7, "방 가", "0x30"),
            "이름이 같아도 ID가 다르면 아니다"
        );
        assert!(
            !front_is_target(&windows, 7, "방 가", UNKNOWN_HANDLE),
            "handle을 모르면 보내지 않는다"
        );
        assert!(
            !front_is_target(&[window(0x2A, None)], 7, "방 가", "0x2A"),
            "이름을 읽을 수 없으면 아니다"
        );
    }

    #[test]
    fn handle_requires_exactly_one_named_kakao_window() {
        // 창 이름이 없으면(화면 기록 권한 없음) handle을 정할 수 없다.
        assert_eq!(
            window_handle("방 가", 7, &[window(1, None)]),
            UNKNOWN_HANDLE
        );
        // 다른 프로세스·다른 layer의 같은 이름 창은 무시한다.
        let other = WindowInfo {
            id: 2,
            pid: 8,
            layer: 0,
            name: Some("방 가".into()),
            on_screen: true,
        };
        let overlay = WindowInfo {
            id: 3,
            pid: 7,
            layer: 3,
            name: Some("방 가".into()),
            on_screen: true,
        };
        assert_eq!(
            window_handle("방 가", 7, &[other, overlay, window(0xFF, Some("방 가"))]),
            "0xFF"
        );
    }
}
