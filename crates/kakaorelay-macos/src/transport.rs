//! 텍스트·이미지 발송 단계. 원본 `KakaoTestTransport`·`KakaoImageTransport`의 맥 대응(사용자 결정 2026-11-23).
//!
//! 순서: '창' 메뉴로 대상 확인 → 카카오톡 전면화(AXFrontmost) + 메뉴 항목 AXPress → 전면 앱과 가장 앞 창(CGWindowList 이름·ID) 확인
//! → 합성 입력 → Return → DB의 새 발신 메시지로 확인 → 이전 전면 앱 복원.
//! 대상 확인이 하나라도 실패하면 입력 전에 중단한다.
//!
//! 초안 읽기(사용자 결정 2026-11-23, B안): 입력창이 AX에 없어 Cmd+A·Cmd+C로 읽고 클립보드를 복원한다.
//! 클립보드 변경 횟수가 그대로면 복사할 글이 없던 것(빈 초안)이다. 읽은 뒤 →(오른쪽 화살표)로 선택을 풀어
//! 이어지는 입력·Return이 선택 영역을 대체하지 않게 한다. 입력 자체는 클립보드 없이 유니코드 이벤트로 한다.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use core_graphics::event::CGEventFlags;

use kakaorelay_core::platform::{ImageTransport, SendTarget, TextTransport, TransportError};

use crate::app::{Kakao, failed};
use crate::ax::{self, Element};
use crate::catalog::{self, UNKNOWN_HANDLE};
use crate::reader::Store;
use crate::{input, pasteboard};

const TEXT: &[i32] = &[1];
/// 붙여넣은 이미지는 사진(2) 또는 묶음 사진(27)으로, 확장자에 따라 파일(18)로 기록될 수 있다.
const IMAGE: &[i32] = &[2, 27, 18];

fn api(error: kakaorelay_ai::ApiFailure) -> TransportError {
    failed(error.detail)
}

#[derive(Debug, Clone)]
struct Target {
    pid: i32,
    title: String,
    handle: String,
    room_id: Option<i64>,
}

/// 복사 직후 대기 시간. 이 안에 클립보드가 바뀌지 않으면 빈 초안으로 본다.
const COPY_WAIT: Duration = Duration::from_millis(400);

/// Cmd+C 결과 해석. 변경 횟수가 그대로면 선택된 글이 없던 것이다.
fn interpret_copy(changed: bool, text: Option<String>) -> Result<String, TransportError> {
    match (changed, text) {
        (false, _) => Ok(String::new()),
        (true, Some(text)) => Ok(text),
        (true, None) => Err(failed(
            "입력창에 텍스트가 아닌 내용이 있어 입력하지 않았습니다.",
        )),
    }
}

/// 텍스트·이미지 공통 상태. 끝나거나 실패해 버려질 때 이전 전면 앱을 되돌린다.
struct Session {
    store: Arc<Store>,
    target: Option<Target>,
    previous: Option<i32>,
    active: bool,
    baseline: Option<(Vec<i64>, i64)>,
}

impl Session {
    fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            target: None,
            previous: None,
            active: false,
            baseline: None,
        }
    }

    fn target(&self) -> Result<&Target, TransportError> {
        self.target
            .as_ref()
            .ok_or_else(|| failed("대상이 확인되지 않았습니다."))
    }

    /// 원본 `ValidateTarget`: 잠금 화면, 프로세스, 제목 유일성, 창 식별자.
    fn validate(&mut self, desired: &dyn SendTarget) -> Result<(), TransportError> {
        if !ax::screen_capture_allowed() {
            return Err(TransportError::PermissionDenied {
                code: "screen_recording_required".into(),
                detail: "대상 대화창을 확인하려면 화면 기록 권한이 필요합니다. 시스템 설정 > 개인정보 보호 및 보안 > 화면 및 시스템 오디오 녹음에서 KakaoRelay를 허용하세요.".into(),
            });
        }
        if ax::screen_locked() {
            return Err(failed("화면이 잠겨 있어 입력할 수 없습니다."));
        }
        let kakao = Kakao::attach(desired.expected_process_id())?;
        let matches = kakao
            .menu()?
            .into_iter()
            .filter(|item| {
                catalog::is_conversation_item(&crate::app::entry(item.clone()))
                    && item.title == desired.recipient()
            })
            .count();
        if matches != 1 {
            return Err(failed(
                "대상 대화창이 열려 있지 않거나 같은 이름의 창이 여러 개입니다.",
            ));
        }
        let handle = catalog::window_handle(desired.recipient(), kakao.pid, &ax::window_list());
        if handle == UNKNOWN_HANDLE {
            return Err(failed(
                "대상 대화창의 창 식별자를 확인하지 못했습니다. 입력하지 않았습니다.",
            ));
        }
        if handle != desired.expected_window_handle() {
            return Err(failed(
                "대화창 식별 정보가 바뀌었습니다. 열린 대화방을 다시 불러오세요.",
            ));
        }
        if let Some(previous) = &self.target
            && (previous.pid != kakao.pid || previous.title != desired.recipient())
        {
            return Err(failed("확인한 대상이 바뀌었습니다."));
        }
        let room_id = match desired.room_id() {
            Some(id) => Some(
                id.parse()
                    .map_err(|_| failed("대상 방 ID 형식이 올바르지 않습니다."))?,
            ),
            None => None,
        };
        self.target = Some(Target {
            pid: kakao.pid,
            title: desired.recipient().to_owned(),
            handle,
            room_id,
        });
        Ok(())
    }

    /// 발송 전 기준점: 대상 제목의 DB 방과 그 방의 마지막 logId.
    fn snapshot(&mut self) -> Result<(), TransportError> {
        let target = self.target()?;
        let chats = match target.room_id {
            Some(id) => vec![id],
            None => self.store.chats_titled(&target.title).map_err(api)?,
        };
        if chats.is_empty() {
            return Err(failed(
                "대상 대화창과 같은 이름의 방을 카카오톡 DB에서 찾지 못해 발송을 확인할 수 없습니다.",
            ));
        }
        let last = self.store.last_log_id(&chats).map_err(api)?;
        self.baseline = Some((chats, last));
        Ok(())
    }

    fn menu_item(&self, kakao: &Kakao) -> Result<crate::ax::MenuItem, TransportError> {
        let title = &self.target()?.title;
        let mut items: Vec<_> = kakao
            .menu()?
            .into_iter()
            .filter(|item| {
                catalog::is_conversation_item(&crate::app::entry(item.clone()))
                    && &item.title == title
            })
            .collect();
        match items.len() {
            1 => Ok(items.remove(0)),
            _ => Err(failed(
                "대상 대화창이 열려 있지 않거나 같은 이름의 창이 여러 개입니다.",
            )),
        }
    }

    /// 카카오톡이 전면 앱이고, 그 가장 앞 일반 창이 대상(이름·CGWindowID 일치)인지.
    /// 카카오톡 창 중 맨 앞이 대상인지. 다른 앱의 전면 여부는 보지 않는다(키는 카카오톡 PID로만 간다).
    fn is_front(&self, kakao: &Kakao) -> Result<bool, TransportError> {
        let target = self.target()?;
        Ok(catalog::front_is_target(
            &ax::window_list(),
            kakao.pid,
            &target.title,
            &target.handle,
        ))
    }

    /// 키를 보내기 직전 확인.
    /// 비활성 카카오톡은 PID로 보낸 키를 버리므로(2026-11-23 실측) 비활성이면 다시 전면화한다.
    /// 복사 직후 카카오톡이 이름 없는 작은 알림 창을 0.25초 미만 맨 위에 띄우므로 최대 1초 다시 확인한다.
    fn require_front(&self) -> Result<Kakao, TransportError> {
        let kakao = Kakao::attach(self.target()?.pid)?;
        if crate::app::frontmost_pid() != Some(kakao.pid) {
            crate::app::bring_to_front(kakao.pid)?;
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while !self.is_front(&kakao)? {
            if Instant::now() >= deadline {
                return Err(failed(
                    "카카오톡 맨 앞 창이 대상 대화창이 아닙니다. 입력하지 않았습니다.",
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if input::modifier_held() {
            return Err(failed("수정 키가 눌려 있어 입력하지 않았습니다."));
        }
        Ok(kakao)
    }

    /// 대상 창을 전면화한다. 이전 전면 앱은 끝날 때 되돌린다.
    fn activate(&mut self) -> Result<(), TransportError> {
        let kakao = Kakao::attach(self.target()?.pid)?;
        if self.previous.is_none() {
            self.previous = crate::app::frontmost_pid().filter(|pid| *pid != kakao.pid);
        }
        crate::app::bring_to_front(kakao.pid)?;
        self.menu_item(&kakao)?.element.press().map_err(|code| {
            failed(format!(
                "대상 대화창을 전면으로 가져오지 못했습니다(AX 오류 {code})."
            ))
        })?;
        self.require_front().map(|_| ())
    }

    /// 처음 한 번만 전면화하고, 이후에는 대상이 여전히 전면 key window인지 확인한다.
    fn prepare(&mut self) -> Result<Kakao, TransportError> {
        if !self.active {
            self.activate()?;
            self.active = true;
        }
        self.require_front()
    }

    /// 입력창 내용을 Cmd+A·Cmd+C로 읽는다. 클립보드가 바뀌었으면 원래대로 되돌린다.
    fn read_input(&mut self) -> Result<String, TransportError> {
        let kakao = self.prepare()?;
        let snapshot = pasteboard::save();
        let before = pasteboard::change_count();
        input::key(kakao.pid, input::KEY_A, CGEventFlags::CGEventFlagCommand)?;
        input::key(kakao.pid, input::KEY_C, CGEventFlags::CGEventFlagCommand)?;
        let deadline = Instant::now() + COPY_WAIT;
        while pasteboard::change_count() == before && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let changed = pasteboard::change_count() != before;
        let text = changed.then(pasteboard::text).flatten();
        let collapsed = input::key(kakao.pid, input::KEY_RIGHT, CGEventFlags::CGEventFlagNull);
        if changed {
            pasteboard::restore(&snapshot)?;
        }
        collapsed?;
        interpret_copy(changed, text)
    }

    fn wait_sent(
        &self,
        kinds: &[i32],
        text: Option<&str>,
        timeout: Duration,
    ) -> Result<bool, TransportError> {
        let (chats, after) = self
            .baseline
            .as_ref()
            .ok_or_else(|| failed("발송 기준점이 없습니다."))?;
        let deadline = Instant::now() + timeout;
        loop {
            if self
                .store
                .find_sent(chats, *after, kinds, text)
                .map_err(api)?
                .is_some()
            {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    fn restore_focus(&mut self) {
        self.active = false;
        if let Some(pid) = self.previous.take()
            && let Some(app) = Element::application(pid)
        {
            // 이전 앱이 종료되었으면 되돌릴 곳이 없다. 실패해도 발송 결과는 바뀌지 않는다.
            let _ = app.set_frontmost();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.restore_focus();
    }
}

/// 해당 pid의 가장 앞 일반 창(layer 0, 화면 표시) CGWindowID.
fn front_window(pid: i32) -> Option<String> {
    ax::window_list()
        .into_iter()
        .find(|w| w.pid == pid && w.layer == 0 && w.on_screen)
        .map(|w| catalog::format_handle(w.id))
}

pub struct MacTextTransport {
    session: Session,
    message: Option<String>,
}

impl MacTextTransport {
    pub(crate) fn new(store: Arc<Store>) -> Self {
        Self {
            session: Session::new(store),
            message: None,
        }
    }
}

impl TextTransport for MacTextTransport {
    fn foreground_window(&self) -> String {
        crate::app::frontmost_pid()
            .and_then(front_window)
            .unwrap_or_else(|| UNKNOWN_HANDLE.to_owned())
    }

    fn validate_target(&mut self, target: &dyn SendTarget) -> Result<(), TransportError> {
        self.session.validate(target)
    }

    fn read_draft(&mut self) -> Result<Option<String>, TransportError> {
        self.session.read_input().map(Some)
    }

    fn write_draft(&mut self, message: &str) -> Result<(), TransportError> {
        self.session.target()?;
        if input::modifier_held() {
            return Err(failed("수정 키가 눌려 있어 입력하지 않았습니다."));
        }
        self.session.snapshot()?;
        let kakao = self.session.prepare()?;
        input::type_text(kakao.pid, message)?;
        self.message = Some(message.to_owned());
        Ok(())
    }

    fn post_enter(&mut self) -> Result<(), TransportError> {
        if self.message.is_none() {
            return Err(failed("입력하지 않은 요청에는 Enter를 보내지 않습니다."));
        }
        let kakao = self.session.require_front()?;
        input::key(kakao.pid, input::KEY_RETURN, CGEventFlags::CGEventFlagNull)
    }

    fn confirm_after_enter(&mut self, target: &dyn SendTarget) -> Result<bool, TransportError> {
        let text = self
            .message
            .clone()
            .unwrap_or_else(|| target.message().to_owned());
        let result = self
            .session
            .wait_sent(TEXT, Some(&text), Duration::from_secs(10));
        self.session.restore_focus();
        result
    }
}

pub struct MacImageTransport {
    session: Session,
    clipboard: Option<pasteboard::Snapshot>,
    before: Vec<u32>,
    attached: bool,
}

impl MacImageTransport {
    pub(crate) fn new(store: Arc<Store>) -> Self {
        Self {
            session: Session::new(store),
            clipboard: None,
            before: Vec::new(),
            attached: false,
        }
    }

    fn restore_clipboard(&mut self) -> Result<(), TransportError> {
        match self.clipboard.take() {
            Some(snapshot) => pasteboard::restore(&snapshot),
            None => Ok(()),
        }
    }

    /// 붙여넣기 뒤 새로 열린 카카오톡 확인 창. 정확히 하나이고 카카오톡의 가장 앞 창이어야 한다.
    fn preview_window(&self, pid: i32) -> Result<Option<u32>, TransportError> {
        let windows = ax::window_list();
        let fresh: Vec<u32> = windows
            .iter()
            .filter(|w| w.pid == pid && w.layer == 0 && w.on_screen && !self.before.contains(&w.id))
            .map(|w| w.id)
            .collect();
        match fresh.as_slice() {
            [] => Ok(None),
            [id] if front_window(pid) == Some(catalog::format_handle(*id)) => Ok(Some(*id)),
            [_] => Ok(None),
            _ => Err(failed(
                "이미지 전송 창이 여러 개입니다. 자동 확인을 중단했습니다.",
            )),
        }
    }

    fn confirm(&mut self) -> Result<bool, TransportError> {
        if !self.attached {
            return Err(failed("이미지 대상이 확인되지 않았습니다."));
        }
        let pid = self.session.target()?.pid;
        let deadline = Instant::now() + Duration::from_secs(3);
        let preview = loop {
            if let Some(id) = self.preview_window(pid)? {
                break id;
            }
            if Instant::now() >= deadline {
                return Err(failed(
                    "붙여넣기 후 이 대화방의 이미지 전송 창을 확인하지 못했습니다. 대화를 확인하고 다시 보내지 마세요.",
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        if crate::app::frontmost_pid() != Some(pid) || input::modifier_held() {
            return Err(failed("이미지 전송 창 또는 키보드 상태가 바뀌었습니다."));
        }
        input::key(pid, input::KEY_RETURN, CGEventFlags::CGEventFlagNull)?;
        let closing = Instant::now() + Duration::from_secs(5);
        while Instant::now() < closing
            && ax::window_list()
                .iter()
                .any(|w| w.id == preview && w.on_screen)
        {
            std::thread::sleep(Duration::from_millis(100));
        }
        self.restore_clipboard()?;
        self.session.wait_sent(IMAGE, None, Duration::from_secs(20))
    }
}

impl Drop for MacImageTransport {
    fn drop(&mut self) {
        // 실패 경로에서도 사용자 클립보드를 되돌린다.
        let _ = self.restore_clipboard();
    }
}

impl ImageTransport for MacImageTransport {
    fn validate(&mut self, target: &dyn SendTarget) -> Result<(), TransportError> {
        self.session.validate(target)
    }

    fn attach(&mut self, path: &Path) -> Result<(), TransportError> {
        let pid = self.session.target()?.pid;
        if !path.is_file() {
            return Err(failed("첨부할 이미지 파일이 없습니다."));
        }
        if input::modifier_held() {
            return Err(failed("수정 키가 눌려 있어 첨부하지 않았습니다."));
        }
        self.session.snapshot()?;
        if !self.session.read_input()?.is_empty() {
            return Err(failed("카카오톡의 작성 중인 초안을 먼저 정리하세요."));
        }
        self.before = ax::window_list()
            .iter()
            .filter(|w| w.pid == pid)
            .map(|w| w.id)
            .collect();
        self.clipboard = Some(pasteboard::save());
        pasteboard::put_file(path)?;
        let kakao = self.session.prepare()?;
        input::key(kakao.pid, input::KEY_V, CGEventFlags::CGEventFlagCommand)?;
        self.attached = true;
        Ok(())
    }

    fn confirm_preview(&mut self) -> Result<bool, TransportError> {
        let result = self.confirm();
        let restored = self.restore_clipboard();
        self.session.restore_focus();
        let confirmed = result?;
        restored?;
        Ok(confirmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kakaorelay_core::platform::SendTarget;

    struct Request {
        continuation: Option<String>,
    }

    impl SendTarget for Request {
        fn request_id(&self) -> &str {
            "r"
        }
        fn recipient(&self) -> &str {
            "방"
        }
        fn expected_process_id(&self) -> i32 {
            1
        }
        fn expected_window_handle(&self) -> &str {
            "0x1"
        }
        fn message(&self) -> &str {
            "보낼 글"
        }
        fn continuation_of(&self) -> Option<&str> {
            self.continuation.as_deref()
        }
    }

    #[test]
    fn unchanged_clipboard_means_empty_draft() {
        assert_eq!(interpret_copy(false, None).unwrap(), "");
        // 복사가 일어나지 않았으면 클립보드에 남은 이전 글은 초안이 아니다.
        assert_eq!(interpret_copy(false, Some("예전 글".into())).unwrap(), "");
        assert!(
            interpret_copy(true, None).is_err(),
            "텍스트가 아닌 초안은 중단"
        );
    }

    #[test]
    fn existing_draft_blocks_before_input() {
        let fresh = Request { continuation: None };
        let draft = interpret_copy(true, Some("쓰던 글".into())).unwrap();
        assert!(
            !fresh.accepts_initial_draft(&draft),
            "초안이 있으면 입력 전에 중단"
        );
        assert!(fresh.accepts_initial_draft(&interpret_copy(false, None).unwrap()));
        // 후속 요청은 같은 글이 입력창에 남아 있을 때만 이어간다(원본 AcceptsInitialDraft).
        let continued = Request {
            continuation: Some("prev".into()),
        };
        assert!(
            continued.accepts_initial_draft(&interpret_copy(true, Some("보낼 글".into())).unwrap())
        );
    }
}
