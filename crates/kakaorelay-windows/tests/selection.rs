//! 메모리 키 후보 판정과 열린 방 선택 규칙.
//! 선택 규칙은 원본 `tests/KakaoRelay.Tests/WorkspaceChecks.cs`의 `ConversationCatalog.SelectTargets` 검사 이식이다.
//! 같은 파일의 `ComposeSession`·`TestSender` 원장 검사는 Phase 2(`kakaorelay-core`) 범위다.

use kakaorelay_windows::catalog::{NativeControlSnapshot, WindowSnapshot, select_targets};
use kakaorelay_windows::memory_keys::scan_key_candidates;

fn window(title: &str, handle: &str) -> WindowSnapshot {
    WindowSnapshot {
        title: title.into(),
        handle: handle.into(),
        process_id: 12,
        visible: true,
        enabled: true,
        class_name: "EVA_Window_Dblclk".into(),
        native_children: vec![NativeControlSnapshot {
            handle: "0x200".into(),
            class_name: "RICHEDIT50W".into(),
            visible: true,
            enabled: true,
        }],
        ..Default::default()
    }
}

#[test]
fn conversation_picker_excludes_main_hidden_and_minimized_windows() {
    let conversation = window("Recipient", "0x10");
    let mut main = window("KakaoTalk", "0x20");
    main.native_children.clear();
    let mut hidden = window("Hidden", "0x30");
    hidden.visible = false;
    let mut minimized = window("Minimized", "0x40");
    minimized.minimized = true;
    let choices = select_targets(&[conversation, main, hidden, minimized], |_| true);
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].title, "Recipient");
    assert!(choices[0].selectable());
}

#[test]
fn edit_controls_without_the_message_control_id_are_excluded() {
    assert!(select_targets(&[window("Recipient", "0x10")], |_| false).is_empty());
}

#[test]
fn same_name_hidden_window_also_blocks_ambiguous_selection() {
    let mut duplicate = window("Recipient", "0x50");
    duplicate.visible = false;
    let choices = select_targets(&[window("Recipient", "0x10"), duplicate], |_| true);
    assert!(choices[0].ambiguous);
    assert!(!choices[0].selectable());
}

#[test]
fn blank_titles_and_two_editors_are_excluded() {
    let blank = window("  ", "0x10");
    let mut two = window("Two", "0x20");
    two.native_children.push(two.native_children[0].clone());
    assert!(select_targets(&[blank, two], |_| true).is_empty());
}

#[test]
fn rooms_are_sorted_ignoring_case_first() {
    let choices = select_targets(
        &[window("b", "0x1"), window("A", "0x2"), window("가", "0x3")],
        |_| true,
    );
    let titles: Vec<_> = choices.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, ["A", "b", "가"]);
}

#[test]
fn window_snapshot_json_keeps_report_field_names() {
    let json = serde_json::to_value(window("Recipient", "0x10")).unwrap();
    assert_eq!(json["automationStatus"], "pending");
    assert_eq!(json["nativeChildren"][0]["className"], "RICHEDIT50W");
    assert_eq!(json["nativeChildren"][0]["isInputCandidate"], true);
}

fn hex(byte: u8, count: usize) -> String {
    format!("{byte:02x}").repeat(count / 2)
}

#[test]
fn raw_key_literals_become_candidates() {
    let mut buffer = b"..x'".to_vec();
    buffer.extend(hex(0xab, 64).bytes());
    buffer.extend(b"'..X'");
    buffer.extend(hex(0xcd, 64).bytes());
    buffer.extend(hex(0xef, 32).bytes());
    buffer.extend(b"'..");
    let mut keys = Vec::new();
    scan_key_candidates(&buffer, &mut keys);
    assert_eq!(keys.len(), 2);
    assert_eq!(*keys[0], [0xab; 32]);
    // 96자리(키+salt)는 앞 64자리만 키로 쓴다.
    assert_eq!(*keys[1], [0xcd; 32]);
}

#[test]
fn malformed_or_duplicate_literals_are_ignored() {
    let literal = format!("x'{}'", hex(0x11, 64));
    let not_hex = format!("x'{}g'", hex(0x22, 62));
    let unterminated = format!("x'{} ", hex(0x33, 64));
    let buffer = format!("{literal}{literal}{not_hex}{unterminated}");
    let mut keys = Vec::new();
    scan_key_candidates(buffer.as_bytes(), &mut keys);
    assert_eq!(keys.len(), 1);
    assert_eq!(*keys[0], [0x11; 32]);
    // 이미 찾은 후보는 다음 청크에서도 다시 넣지 않는다.
    scan_key_candidates(literal.as_bytes(), &mut keys);
    assert_eq!(keys.len(), 1);
}

#[test]
fn literal_cut_at_buffer_end_is_not_read() {
    let literal = format!("x'{}'", hex(0x44, 64));
    let mut keys = Vec::new();
    scan_key_candidates(&literal.as_bytes()[..literal.len() - 1], &mut keys);
    assert!(keys.is_empty());
}
