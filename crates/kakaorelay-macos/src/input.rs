//! 합성 키 입력. 클립보드 없이 `CGEventKeyboardSetUnicodeString`으로 문자열을 넣는다.

use core_graphics::event::{CGEvent, CGEventFlags, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

use kakaorelay_core::platform::TransportError;

use crate::app::failed;

pub const KEY_RETURN: CGKeyCode = 36;
pub const KEY_V: CGKeyCode = 9;
pub const KEY_A: CGKeyCode = 0;
pub const KEY_C: CGKeyCode = 8;
pub const KEY_RIGHT: CGKeyCode = 124;
/// 한 이벤트에 싣는 UTF-16 단위 수. 이보다 긴 문자열은 일부 앱에서 잘린다.
const CHUNK: usize = 20;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceFlagsState(state: CGEventSourceStateID) -> u64;
}

/// Shift·Control·Option·Command가 눌려 있는지. 원본 `GetAsyncKeyState(Shift/Ctrl/Alt)` 확인의 대응이다.
pub fn modifier_held() -> bool {
    let held = CGEventFlags::CGEventFlagShift
        | CGEventFlags::CGEventFlagControl
        | CGEventFlags::CGEventFlagAlternate
        | CGEventFlags::CGEventFlagCommand;
    // SAFETY: 상태 ID만 받는 조회 함수다.
    let state = unsafe { CGEventSourceFlagsState(CGEventSourceStateID::HIDSystemState) };
    CGEventFlags::from_bits_truncate(state).intersects(held)
}

fn source() -> Result<CGEventSource, TransportError> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| failed("키 입력 이벤트 원본을 만들지 못했습니다."))
}

fn event(source: &CGEventSource, key: CGKeyCode, down: bool) -> Result<CGEvent, TransportError> {
    CGEvent::new_keyboard_event(source.clone(), key, down)
        .map_err(|_| failed("키 입력 이벤트를 만들지 못했습니다."))
}

/// 대상 프로세스에만 키를 보낸다. 포커스가 바뀌어도 다른 앱으로 입력이 새지 않게 한다.
pub fn key(pid: i32, key: CGKeyCode, flags: CGEventFlags) -> Result<(), TransportError> {
    let source = source()?;
    for down in [true, false] {
        let event = event(&source, key, down)?;
        event.set_flags(flags);
        event.post_to_pid(pid);
    }
    Ok(())
}

/// 문자열을 UTF-16 20단위 이하로 나눈다. 서로게이트 쌍은 나누지 않는다.
pub fn chunks(text: &str) -> Vec<Vec<u16>> {
    let mut result = Vec::new();
    let mut current = Vec::new();
    for ch in text.chars() {
        let mut units = [0u16; 2];
        let encoded = ch.encode_utf16(&mut units);
        if current.len() + encoded.len() > CHUNK {
            result.push(std::mem::take(&mut current));
        }
        current.extend_from_slice(encoded);
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

/// 줄바꿈은 `\n` 문자로 넣는다. Return 키 코드가 아니라 문자 입력이라 전송되지 않는다.
pub fn type_text(pid: i32, text: &str) -> Result<(), TransportError> {
    let source = source()?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for chunk in chunks(&normalized) {
        let down = event(&source, 0, true)?;
        down.set_flags(CGEventFlags::CGEventFlagNull);
        down.set_string_from_utf16_unchecked(&chunk);
        down.post_to_pid(pid);
        let up = event(&source, 0, false)?;
        up.set_flags(CGEventFlags::CGEventFlagNull);
        up.set_string_from_utf16_unchecked(&chunk);
        up.post_to_pid(pid);
        // 이벤트가 몰려 순서가 섞이지 않도록 짧게 쉰다.
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_keep_surrogate_pairs_together() {
        let text = format!("{}😀끝", "가".repeat(19));
        let parts = chunks(&text);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 19);
        assert_eq!(String::from_utf16(&parts[1]).unwrap(), "😀끝");
        assert_eq!(parts.concat(), text.encode_utf16().collect::<Vec<_>>());
        assert!(chunks("").is_empty());
    }
}
