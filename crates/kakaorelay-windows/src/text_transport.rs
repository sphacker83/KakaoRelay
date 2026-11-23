//! 이미 열려 있고 확인된 대화창 전용 텍스트 발송. 원본: `KakaoTestTransport.cs`.
//! 전면 전환·클립보드·물리 키보드 입력·자동 재시도를 쓰지 않는다.

use crate::catalog::{CONVERSATION_CLASS, MESSAGE_EDITOR_ID, RICH_EDIT_CLASS};
use crate::{native, process};
use kakaorelay_core::platform::{SendTarget, TextTransport, TransportError, same_text};
use std::collections::HashSet;
use std::time::Duration;
use windows::Win32::Foundation::{ERROR_TIMEOUT, GetLastError, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, GetUserObjectInformationW,
    OpenInputDesktop, UOI_NAME,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetDlgCtrlID, PostMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
    WM_KEYDOWN, WM_KEYUP,
};

const EM_SETSEL: u32 = 0x00B1;
const EM_REPLACESEL: u32 = 0x00C2;
const WM_GETTEXT: u32 = 0x000D;
const WM_GETTEXTLENGTH: u32 = 0x000E;
const SEND_TIMEOUT_MS: u32 = 2000;
pub(crate) const VK_RETURN: usize = 0x0D;
pub(crate) const ENTER_DOWN_LPARAM: isize = 0x001C_0001;
pub(crate) const ENTER_UP_LPARAM: isize = 0xC01C_0001;
const CLEAR_CHECKS: usize = 8;
const CLEAR_INTERVAL: Duration = Duration::from_millis(150);

fn failed(message: &str) -> TransportError {
    TransportError::Failed(message.into())
}

pub(crate) fn hwnd(value: usize) -> HWND {
    HWND(value as *mut _)
}

/// 검증 시점에 고정한 요청 정보. HWND는 Send가 아니므로 정수로 보관한다.
#[derive(Debug, Clone)]
pub(crate) struct Identity {
    pub request_id: String,
    pub recipient: String,
    pub process_id: u32,
    pub window_handle: String,
    pub message: String,
    pub verified_empty_placeholder: bool,
    pub continuation_of: Option<String>,
    pub send_delay_ms: Option<u32>,
}

/// 검증 시점의 요청을 그대로 다시 보이게 해, 초안 규칙은 계약의 기본 구현(`is_empty_draft` 등)을 쓴다.
impl SendTarget for Identity {
    fn request_id(&self) -> &str {
        &self.request_id
    }
    fn recipient(&self) -> &str {
        &self.recipient
    }
    fn expected_process_id(&self) -> i32 {
        self.process_id as i32
    }
    fn expected_window_handle(&self) -> &str {
        &self.window_handle
    }
    fn message(&self) -> &str {
        &self.message
    }
    fn verified_empty_placeholder(&self) -> bool {
        self.verified_empty_placeholder
    }
    fn continuation_of(&self) -> Option<&str> {
        self.continuation_of.as_deref()
    }
    fn send_delay_ms(&self) -> Option<u32> {
        self.send_delay_ms
    }
}

impl Identity {
    pub(crate) fn of(target: &dyn SendTarget) -> Result<Self, TransportError> {
        if target.expected_process_id() <= 0 {
            return Err(failed("Missing observed window identity"));
        }
        native::parse(target.expected_window_handle())?;
        Ok(Self {
            request_id: target.request_id().to_string(),
            recipient: target.recipient().to_string(),
            process_id: target.expected_process_id() as u32,
            window_handle: target.expected_window_handle().to_string(),
            message: target.message().to_string(),
            verified_empty_placeholder: target.verified_empty_placeholder(),
            continuation_of: target.continuation_of().map(str::to_string),
            send_delay_ms: target.send_delay_ms(),
        })
    }
}

#[derive(Default)]
pub struct KakaoTextTransport {
    editor: usize,
    target: usize,
    request: Option<Identity>,
}

impl KakaoTextTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn validate(&mut self, desired: Identity) -> Result<(), TransportError> {
        require_unlocked_desktop()?;
        let same_process = process::name_of(desired.process_id)
            .map_err(|_| failed("Target process or session changed"))?
            .is_some_and(|name| name == process::KAKAO_PROCESS);
        let same_session = process::session_of(desired.process_id).is_some()
            && process::session_of(desired.process_id) == process::current_session();
        if !same_process || !same_session {
            return Err(failed("Target process or session changed"));
        }
        let ids = HashSet::from([desired.process_id as i32]);
        let matches: Vec<_> = native::enumerate(&ids)
            .into_iter()
            .filter(|w| w.title == desired.recipient)
            .collect();
        let [window] = matches.as_slice() else {
            return Err(failed("Recipient window is missing or ambiguous"));
        };
        if window.handle != desired.window_handle
            || window.class_name != CONVERSATION_CLASS
            || !window.visible
            || window.minimized
            || !window.enabled
        {
            return Err(failed("Observed conversation identity or state changed"));
        }
        let mut candidates = Vec::new();
        for child in &window.native_children {
            if child.class_name == RICH_EDIT_CLASS && child.enabled && child.visible {
                let handle = native::parse(&child.handle)?;
                // SAFETY: 창 ID 조회 함수다.
                if unsafe { GetDlgCtrlID(handle) } == MESSAGE_EDITOR_ID {
                    candidates.push(handle.0 as usize);
                }
            }
        }
        let [current_editor] = candidates.as_slice() else {
            return Err(failed("Expected one message editor with control ID 1006"));
        };
        if self.editor != 0 && self.editor != *current_editor {
            return Err(failed("Message editor was replaced"));
        }
        self.target = native::parse(&window.handle)?.0 as usize;
        self.editor = *current_editor;
        self.request = Some(desired);
        self.check_identity()
    }

    fn revalidate(&mut self) -> Result<&Identity, TransportError> {
        let request = self
            .request
            .clone()
            .ok_or_else(|| failed("Target was not validated"))?;
        self.validate(request)?;
        Ok(self.request.as_ref().expect("검증 직후"))
    }

    fn check_identity(&self) -> Result<(), TransportError> {
        let (editor, target) = (hwnd(self.editor), hwnd(self.target));
        // SAFETY: 임의 값을 넘겨도 안전한 조회 함수다.
        let root = unsafe { GetAncestor(editor, GA_ROOT) };
        let Some(request) = &self.request else {
            return Err(failed("Window or editor no longer exists"));
        };
        if self.editor == 0 || !native::exists(target) || !native::exists(editor) || root != target
        {
            return Err(failed("Window or editor no longer exists"));
        }
        if native::process_of(editor) != request.process_id {
            return Err(failed("Editor process changed"));
        }
        Ok(())
    }

    pub(crate) fn read_text(&self) -> Result<String, TransportError> {
        self.check_identity()?;
        let editor = hwnd(self.editor);
        let mut count = 0usize;
        // SAFETY: 응답 대기 제한이 있는 메시지 전송이며 count는 유효한 출력 포인터다.
        let sent = unsafe {
            SendMessageTimeoutW(
                editor,
                WM_GETTEXTLENGTH,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                SEND_TIMEOUT_MS,
                Some(&mut count),
            )
        };
        if sent.0 == 0 {
            return Err(failed("Editor length query timed out or failed"));
        }
        let mut buffer = vec![0u16; count + 1];
        // SAFETY: buffer 길이를 wParam으로 넘겨 그 범위 안에서만 쓰게 한다.
        let read = unsafe {
            SendMessageTimeoutW(
                editor,
                WM_GETTEXT,
                WPARAM(buffer.len()),
                LPARAM(buffer.as_mut_ptr() as isize),
                SMTO_ABORTIFHUNG,
                SEND_TIMEOUT_MS,
                None,
            )
        };
        if read.0 == 0 {
            return Err(failed("Editor read timed out or failed"));
        }
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Ok(String::from_utf16_lossy(&buffer[..end]))
    }
}

impl TextTransport for KakaoTextTransport {
    fn foreground_window(&self) -> String {
        native::foreground()
    }

    fn validate_target(&mut self, target: &dyn SendTarget) -> Result<(), TransportError> {
        self.validate(Identity::of(target)?)
    }

    fn read_draft(&mut self) -> Result<Option<String>, TransportError> {
        self.read_text().map(Some)
    }

    fn write_draft(&mut self, message: &str) -> Result<(), TransportError> {
        let request = self.revalidate()?.clone();
        if !request.accepts_initial_draft(&self.read_text()?) {
            return Err(failed("A different draft appeared before input"));
        }
        let editor = hwnd(self.editor);
        // WM_SETTEXT는 다중 줄 편집기에 EN_CHANGE를 알리지 않는다. 카카오톡이 전송 버튼 상태를 갱신하도록
        // 선택 영역 치환(EM_REPLACESEL)으로 입력한다.
        // SAFETY: 응답 대기 제한이 있는 메시지 전송이다.
        let selected = unsafe {
            SendMessageTimeoutW(
                editor,
                EM_SETSEL,
                WPARAM(0),
                LPARAM(-1),
                SMTO_ABORTIFHUNG,
                SEND_TIMEOUT_MS,
                None,
            )
        };
        if selected.0 == 0 {
            return Err(failed("Selecting the verified test draft failed"));
        }
        let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: text는 NUL로 끝나며 호출이 끝날 때까지 살아 있다. 실패 직후 GetLastError를 읽는다.
        let (replaced, error) = unsafe {
            let result = SendMessageTimeoutW(
                editor,
                EM_REPLACESEL,
                WPARAM(1),
                LPARAM(text.as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                SEND_TIMEOUT_MS,
                None,
            );
            (result, GetLastError())
        };
        if replaced.0 == 0 {
            if error == ERROR_TIMEOUT {
                return Err(TransportError::InputTimeout(
                    "Text input failed or is uncertain".into(),
                ));
            }
            return Err(failed("Text input failed or is uncertain"));
        }
        Ok(())
    }

    fn post_enter(&mut self) -> Result<(), TransportError> {
        let request = self.revalidate()?.clone();
        if request.send_delay_ms.is_none() && !same_text(&self.read_text()?, &request.message) {
            return Err(failed("Draft changed before Enter"));
        }
        if native::modifier_held() {
            return Err(failed("A keyboard modifier is held; Enter was not sent"));
        }
        let editor = Some(hwnd(self.editor));
        // SAFETY: 메시지 게시 함수다.
        unsafe {
            PostMessageW(
                editor,
                WM_KEYDOWN,
                WPARAM(VK_RETURN),
                LPARAM(ENTER_DOWN_LPARAM),
            )
        }
        .map_err(|_| failed("Enter key-down was not queued"))?;
        // SAFETY: 메시지 게시 함수다.
        unsafe { PostMessageW(editor, WM_KEYUP, WPARAM(VK_RETURN), LPARAM(ENTER_UP_LPARAM)) }
            .map_err(|_| failed("Enter key-up is uncertain; do not resend"))
    }

    /// 빈 입력창은 UI 처리의 증거일 뿐 전달 증거가 아니다. 원본 `TestSender.SendOnce`의 150ms×8회 확인.
    fn confirm_after_enter(&mut self, target: &dyn SendTarget) -> Result<bool, TransportError> {
        for _ in 0..CLEAR_CHECKS {
            std::thread::sleep(CLEAR_INTERVAL);
            if target.is_empty_draft(&self.read_text()?) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn require_unlocked_desktop() -> Result<(), TransportError> {
    // SAFETY: 데스크톱 핸들은 아래에서 닫는다.
    let desktop = unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) }
        .map_err(|_| failed("Interactive desktop is unavailable or locked"))?;
    let mut name = [0u16; 256];
    // SAFETY: name 버퍼의 바이트 크기를 함께 넘긴다.
    let read = unsafe {
        GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            (name.len() * 2) as u32,
            None,
        )
    };
    // SAFETY: 위에서 연 핸들을 한 번만 닫는다.
    let _ = unsafe { CloseDesktop(desktop) };
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    if read.is_err() || String::from_utf16_lossy(&name[..end]) != "Default" {
        return Err(failed("Default interactive desktop is unavailable"));
    }
    Ok(())
}
