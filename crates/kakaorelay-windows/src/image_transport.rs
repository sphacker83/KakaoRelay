//! 파일 드롭(`WM_DROPFILES`)과 새로 열린 첨부 확인 창의 Enter. 원본: `KakaoImageTransport.cs`.
//! 클립보드·전면 창·물리 키보드 포커스를 바꾸지 않는다.

use crate::catalog::{RICH_EDIT_CLASS, WindowSnapshot};
use crate::native;
use crate::text_transport::{
    ENTER_DOWN_LPARAM, ENTER_UP_LPARAM, Identity, KakaoTextTransport, VK_RETURN,
};
use kakaorelay_core::platform::{ImageTransport, SendTarget, TransportError};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;
use windows::Win32::Foundation::{GlobalFree, HWND, LPARAM, WPARAM};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GW_OWNER, GetDlgCtrlID, GetWindow, PostMessageW, WM_KEYDOWN, WM_KEYUP,
};

const WM_DROPFILES: u32 = 0x0233;
const PREVIEW_TITLES: [&str; 6] = [
    "사진 전송",
    "사진 보내기",
    "이미지 전송",
    "이미지 보내기",
    "파일 전송",
    "파일 보내기",
];
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const PREVIEW_ATTEMPTS: usize = 30;
const CLOSE_ATTEMPTS: usize = 50;
const OWNER_DEPTH: usize = 8;

fn failed(message: &str) -> TransportError {
    TransportError::Failed(message.into())
}

/// `DROPFILES` 구조체(20바이트, pFiles=20, fWide=1) 뒤에 NUL 두 개로 끝나는 UTF-16 경로 목록을 붙인다.
fn drop_files_payload(path: &str) -> Vec<u8> {
    let mut bytes = vec![0u8; 20];
    bytes[0..4].copy_from_slice(&20u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
    for unit in path.encode_utf16().chain([0, 0]) {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

fn control_id(handle: &str) -> Option<i32> {
    // SAFETY: 창 ID 조회 함수다.
    native::parse(handle)
        .ok()
        .map(|h| unsafe { GetDlgCtrlID(h) })
}

fn is_attachment_dialog(window: &WindowSnapshot) -> bool {
    window.class_name == "EVA_Window_Dblclk"
        && (window.title.is_empty() || PREVIEW_TITLES.contains(&window.title.as_str()))
        && window.native_children.iter().any(|c| {
            c.class_name == "EVA_ChildWindow"
                && c.visible
                && c.enabled
                && control_id(&c.handle) == Some(1001)
        })
        && window.native_children.iter().any(|c| {
            c.class_name == "EVA_ChildWindow_Dblclk" && control_id(&c.handle) == Some(1002)
        })
        && !window
            .native_children
            .iter()
            .any(|c| c.class_name == RICH_EDIT_CLASS)
}

#[derive(Default)]
pub struct KakaoImageTransport {
    request: Option<Identity>,
    before: HashSet<String>,
    target_check: KakaoTextTransport,
}

impl KakaoImageTransport {
    pub fn new() -> Self {
        Self::default()
    }

    fn request(&self) -> Result<&Identity, TransportError> {
        self.request
            .as_ref()
            .ok_or_else(|| failed("이미지 대상이 확인되지 않았습니다."))
    }

    fn owned_by_target(&self, window: HWND) -> Result<bool, TransportError> {
        let target = native::parse(&self.request()?.window_handle)?;
        let mut current = window;
        for _ in 0..OWNER_DEPTH {
            if current.is_invalid() {
                break;
            }
            // SAFETY: 소유자 창 조회 함수다. 소유자가 없으면 오류를 반환한다.
            current = unsafe { GetWindow(current, GW_OWNER) }.unwrap_or_default();
            if current == target {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn windows(&self) -> Result<Vec<WindowSnapshot>, TransportError> {
        Ok(native::enumerate(&HashSet::from([
            self.request()?.process_id as i32,
        ])))
    }
}

impl ImageTransport for KakaoImageTransport {
    fn validate(&mut self, target: &dyn SendTarget) -> Result<(), TransportError> {
        let desired = Identity::of(target)?;
        self.target_check.validate(desired.clone())?;
        if !desired.is_empty_draft(&self.target_check.read_text()?) {
            return Err(failed("카카오톡의 작성 중인 초안을 먼저 정리하세요."));
        }
        self.request = Some(desired);
        self.before = self.windows()?.into_iter().map(|w| w.handle).collect();
        Ok(())
    }

    fn attach(&mut self, path: &Path) -> Result<(), TransportError> {
        let request = self.request()?.clone();
        self.target_check.validate(request.clone())?;
        if !request.is_empty_draft(&self.target_check.read_text()?) {
            return Err(failed("이미지 첨부 전에 다른 초안이 생겼습니다."));
        }
        let full = std::path::absolute(path).map_err(|e| TransportError::Failed(e.to_string()))?;
        let bytes = drop_files_payload(&full.to_string_lossy());
        let target = native::parse(&request.window_handle)?;
        // SAFETY: 할당한 메모리에 길이만큼만 복사하고, 게시에 실패했을 때만 해제한다.
        // 게시에 성공하면 카카오톡이 HDROP을 소유하고 DragFinish로 해제한다.
        unsafe {
            let memory = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len())
                .map_err(|e| TransportError::Failed(e.message()))?;
            let pointer = GlobalLock(memory);
            if pointer.is_null() {
                let _ = GlobalFree(Some(memory));
                return Err(failed("파일 첨부 메모리 잠금 실패"));
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
            // 잠금 횟수가 0이 되면 성공이어도 FALSE를 반환하므로 결과를 쓰지 않는다.
            let _ = GlobalUnlock(memory);
            if PostMessageW(
                Some(target),
                WM_DROPFILES,
                WPARAM(memory.0 as usize),
                LPARAM(0),
            )
            .is_err()
            {
                let _ = GlobalFree(Some(memory));
                return Err(failed("파일 첨부 요청 실패"));
            }
        }
        Ok(())
    }

    fn confirm_preview(&mut self) -> Result<bool, TransportError> {
        let process_id = self.request()?.process_id;
        let mut preview = None;
        for _ in 0..PREVIEW_ATTEMPTS {
            std::thread::sleep(POLL_INTERVAL);
            let mut candidates = Vec::new();
            for window in self.windows()? {
                if window.visible
                    && window.enabled
                    && !self.before.contains(&window.handle)
                    && is_attachment_dialog(&window)
                {
                    let handle = native::parse(&window.handle)?;
                    if self.owned_by_target(handle)? {
                        candidates.push(window);
                    }
                }
            }
            if candidates.len() > 1 {
                return Err(failed(
                    "이미지 전송 창이 여러 개입니다. 자동 확인을 중단했습니다.",
                ));
            }
            if let Some(found) = candidates.pop() {
                preview = Some(found);
                break;
            }
        }
        let Some(preview) = preview else {
            return Err(failed(
                "첨부 요청 후 이 대화방의 이미지 전송 창을 확인하지 못했습니다. 대화를 확인하고 다시 보내지 마세요.",
            ));
        };
        let handle = native::parse(&preview.handle)?;
        if native::process_of(handle) != process_id
            || !self.owned_by_target(handle)?
            || native::modifier_held()
        {
            return Err(failed("이미지 전송 창 또는 키보드 상태가 바뀌었습니다."));
        }
        // SAFETY: 메시지 게시 함수다.
        unsafe {
            PostMessageW(
                Some(handle),
                WM_KEYDOWN,
                WPARAM(VK_RETURN),
                LPARAM(ENTER_DOWN_LPARAM),
            )
        }
        .map_err(|_| failed("이미지 전송 Enter 실패"))?;
        // SAFETY: 메시지 게시 함수다.
        unsafe {
            PostMessageW(
                Some(handle),
                WM_KEYUP,
                WPARAM(VK_RETURN),
                LPARAM(ENTER_UP_LPARAM),
            )
        }
        .map_err(|_| failed("이미지 전송 요청 결과가 불확실합니다. 재전송하지 마세요."))?;
        for _ in 0..CLOSE_ATTEMPTS {
            std::thread::sleep(POLL_INTERVAL);
            if !self
                .windows()?
                .iter()
                .any(|w| w.handle == preview.handle && w.visible)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::drop_files_payload;

    #[test]
    fn drop_files_payload_matches_dropfiles_layout() {
        let bytes = drop_files_payload("C:\\a.png");
        assert_eq!(&bytes[0..4], &20u32.to_le_bytes());
        assert_eq!(&bytes[16..20], &1u32.to_le_bytes());
        assert_eq!(bytes.len(), 20 + ("C:\\a.png".len() + 2) * 2);
        assert_eq!(&bytes[bytes.len() - 4..], &[0, 0, 0, 0]);
    }
}
