//! user32 창 열거. 원본: `NativeWindows.cs`. 최상위 창 64개, 자식 창 512개까지만 수집한다.
//! 최상위 창 제목만 읽고 자식 입력창의 텍스트나 대화 내용은 요청하지 않는다.

use crate::catalog::{NativeControlSnapshot, WindowSnapshot};
use kakaorelay_core::platform::TransportError;
use std::collections::HashSet;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, IsWindowEnabled};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
};
use windows::core::BOOL;

const TOP_LIMIT: usize = 64;
const CHILD_LIMIT: usize = 512;

pub(crate) fn format(handle: HWND) -> String {
    format!("0x{:X}", handle.0 as usize)
}

/// `0x` + 16진수 1~16자리만 받는다(원본 요청 검증 형식).
pub(crate) fn parse(value: &str) -> Result<HWND, TransportError> {
    let digits = value
        .strip_prefix("0x")
        .filter(|d| (1..=16).contains(&d.len()));
    digits
        .and_then(|d| u64::from_str_radix(d, 16).ok())
        .map(|v| HWND(v as usize as *mut _))
        .ok_or_else(|| TransportError::Failed("Missing observed window identity".into()))
}

pub(crate) fn foreground() -> String {
    // SAFETY: 인자 없는 조회 함수다.
    format(unsafe { GetForegroundWindow() })
}

pub(crate) fn process_of(handle: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: pid는 유효한 출력 포인터다. 잘못된 핸들이면 0을 남긴다.
    unsafe { GetWindowThreadProcessId(handle, Some(&mut pid)) };
    pid
}

pub(crate) fn exists(handle: HWND) -> bool {
    // SAFETY: 임의 값을 넘겨도 안전한 조회 함수다.
    unsafe { IsWindow(Some(handle)) }.as_bool()
}

/// Shift·Ctrl·Alt 중 하나라도 눌려 있으면 true. 게시한 Enter가 조합 키로 해석되지 않게 한다.
pub(crate) fn modifier_held() -> bool {
    // SAFETY: 가상 키 코드 조회 함수다.
    [0x10, 0x11, 0x12]
        .iter()
        .any(|&key| (unsafe { GetAsyncKeyState(key) } as u16 & 0x8000) != 0)
}

fn text(buffer: &[u16], length: i32) -> String {
    String::from_utf16_lossy(&buffer[..length.clamp(0, buffer.len() as i32) as usize])
}

fn class_name(handle: HWND) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: buffer 길이를 함께 넘긴다.
    let length = unsafe { GetClassNameW(handle, &mut buffer) };
    text(&buffer, length)
}

struct TopState<'a> {
    process_ids: &'a HashSet<i32>,
    result: Vec<WindowSnapshot>,
}

struct ChildState {
    process_id: u32,
    children: Vec<NativeControlSnapshot>,
}

unsafe extern "system" fn enum_child(child: HWND, parameter: LPARAM) -> BOOL {
    // SAFETY: parameter는 enumerate가 넘긴 ChildState이며 열거가 끝날 때까지 살아 있다.
    let state = unsafe { &mut *(parameter.0 as *mut ChildState) };
    if process_of(child) == state.process_id {
        // SAFETY: 창 상태 조회 함수다.
        let (visible, enabled) = unsafe {
            (
                IsWindowVisible(child).as_bool(),
                IsWindowEnabled(child).as_bool(),
            )
        };
        state.children.push(NativeControlSnapshot {
            handle: format(child),
            class_name: class_name(child),
            visible,
            enabled,
        });
    }
    (state.children.len() < CHILD_LIMIT).into()
}

unsafe extern "system" fn enum_top(handle: HWND, parameter: LPARAM) -> BOOL {
    // SAFETY: parameter는 enumerate가 넘긴 TopState이며 열거가 끝날 때까지 살아 있다.
    let state = unsafe { &mut *(parameter.0 as *mut TopState) };
    let pid = process_of(handle);
    if !state.process_ids.contains(&(pid as i32)) {
        return true.into();
    }
    let mut title = [0u16; 1024];
    // SAFETY: 버퍼 길이를 함께 넘기는 조회 함수들이다.
    let (length, visible, enabled, minimized) = unsafe {
        (
            GetWindowTextW(handle, &mut title),
            IsWindowVisible(handle).as_bool(),
            IsWindowEnabled(handle).as_bool(),
            IsIconic(handle).as_bool(),
        )
    };
    let mut children = ChildState {
        process_id: pid,
        children: Vec::new(),
    };
    // SAFETY: children은 이 호출이 끝날 때까지 살아 있다.
    let _ = unsafe {
        EnumChildWindows(
            Some(handle),
            Some(enum_child),
            LPARAM(&mut children as *mut _ as isize),
        )
    };
    let mut window = WindowSnapshot {
        handle: format(handle),
        process_id: pid as i32,
        title: text(&title, length),
        class_name: class_name(handle),
        visible,
        minimized,
        enabled,
        native_children: children.children,
        ..Default::default()
    };
    if window.native_children.len() == CHILD_LIMIT {
        window
            .warnings
            .push("Win32 자식 창 탐색이 512개 제한에 도달했습니다.".into());
    }
    state.result.push(window);
    (state.result.len() < TOP_LIMIT).into()
}

/// 주어진 프로세스의 최상위 창을 수집한다. 보이는 창, 최소화되지 않은 창 순서로 정렬한다. 원본 `NativeWindows.Enumerate`.
pub(crate) fn enumerate(process_ids: &HashSet<i32>) -> Vec<WindowSnapshot> {
    let mut state = TopState {
        process_ids,
        result: Vec::new(),
    };
    // 콜백이 한도에서 false를 돌려주면 EnumWindows도 실패를 반환하므로 결과는 쓰지 않는다(원본 동일).
    // SAFETY: state는 이 호출이 끝날 때까지 살아 있다.
    let _ = unsafe { EnumWindows(Some(enum_top), LPARAM(&mut state as *mut _ as isize)) };
    let mut result = state.result;
    result.sort_by_key(|w| (!w.visible, w.minimized));
    result
}
