//! 실행 중인 카카오톡 프로세스와 '창' 메뉴 조회.

use std::time::{Duration, Instant};

use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_foundation::NSString;

use kakaorelay_core::platform::{ConversationTarget, RoomCatalog, TransportError};

use crate::ax::{self, Element, KAKAO_BUNDLE_ID, MenuItem};
use crate::catalog::{self, MenuEntry};

/// 실행 중인 카카오톡 pid 목록(종료 중인 프로세스 제외).
pub fn kakao_pids() -> Vec<i32> {
    let bundle = NSString::from_str(KAKAO_BUNDLE_ID);
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&bundle)
        .iter()
        .filter(|app| !app.isTerminated())
        .map(|app| app.processIdentifier())
        .collect()
}

/// 활성(전면) 앱의 pid = 화면에 보이는 가장 앞 일반 창(layer 0)의 소유 프로세스.
/// 실측(2026-11-23): 시스템 AX의 `AXFocusedApplication`은 카카오톡을 돌려주지 않고, NSWorkspace의
/// `frontmostApplication`은 런루프 없는 스레드에서 갱신되지 않아 창 서버 목록으로 판정한다.
pub fn frontmost_pid() -> Option<i32> {
    catalog::frontmost_owner(&ax::window_list())
}

/// 전면 앱이 바뀔 때까지 최대 1.5초 기다린다.
fn wait_frontmost(pid: i32) -> bool {
    let deadline = Instant::now() + Duration::from_millis(1500);
    loop {
        if frontmost_pid() == Some(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 카카오톡을 전면 앱으로 만든다. 성공한 방식 이름을 돌려준다.
/// 실측(2026-11-23): 다른 앱(Codex)이 전면이면 `AXFrontmost`가 성공 코드를 내고도 전환되지 않아 차례로 시도한다.
/// `open -b`는 LaunchServices로 활성화하므로 자동화(Apple Events) 권한 요청이 없다.
pub fn bring_to_front(pid: i32) -> Result<&'static str, TransportError> {
    if frontmost_pid() == Some(pid) {
        return Ok("already");
    }
    if Element::application(pid).is_some_and(|app| app.set_frontmost().is_ok())
        && wait_frontmost(pid)
    {
        return Ok("AXFrontmost");
    }
    if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        #[allow(deprecated)]
        let options = NSApplicationActivationOptions::ActivateAllWindows
            | NSApplicationActivationOptions::ActivateIgnoringOtherApps;
        if app.activateWithOptions(options) && wait_frontmost(pid) {
            return Ok("NSRunningApplication.activate");
        }
    }
    let opened = std::process::Command::new("/usr/bin/open")
        .args(["-b", KAKAO_BUNDLE_ID])
        .status()
        .is_ok_and(|status| status.success());
    if opened && wait_frontmost(pid) {
        return Ok("open -b");
    }
    Err(failed(
        "카카오톡을 전면 앱으로 만들지 못했습니다. 입력하지 않았습니다.",
    ))
}

pub fn failed(message: impl Into<String>) -> TransportError {
    TransportError::Failed(message.into())
}

pub fn require_accessibility() -> Result<(), TransportError> {
    if ax::is_trusted() {
        Ok(())
    } else {
        Err(TransportError::PermissionDenied {
            code: "accessibility_required".into(),
            detail: "손쉬운 사용 권한이 없습니다. 시스템 설정 > 개인정보 보호 및 보안 > 손쉬운 사용에서 KakaoRelay를 허용하세요.".into(),
        })
    }
}

/// 카카오톡 앱 하나와 그 '창' 메뉴.
pub struct Kakao {
    pub pid: i32,
    pub element: Element,
}

impl Kakao {
    pub fn attach(pid: i32) -> Result<Self, TransportError> {
        if !kakao_pids().contains(&pid) {
            return Err(failed(
                "대상 카카오톡 프로세스가 바뀌었거나 종료되었습니다.",
            ));
        }
        let element = Element::application(pid)
            .ok_or_else(|| failed("카카오톡 접근성 요소를 만들지 못했습니다."))?;
        Ok(Self { pid, element })
    }

    pub fn menu(&self) -> Result<Vec<MenuItem>, TransportError> {
        ax::window_menu_items(&self.element).map_err(failed)
    }

    pub fn targets(&self) -> Result<Vec<ConversationTarget>, TransportError> {
        let entries: Vec<MenuEntry> = self.menu()?.into_iter().map(entry).collect();
        Ok(catalog::select_targets(
            &entries,
            self.pid,
            &ax::window_list(),
        ))
    }
}

pub fn entry(item: MenuItem) -> MenuEntry {
    MenuEntry {
        title: item.title,
        identifier: item.identifier,
    }
}

/// `RoomCatalog` 구현. 카카오톡이 실행 중이 아니면 빈 목록이다(원본 `ConversationCatalog.Scan`과 같음).
#[derive(Debug, Default)]
pub struct MacRoomCatalog;

impl RoomCatalog for MacRoomCatalog {
    fn scan(&self) -> Result<Vec<ConversationTarget>, TransportError> {
        require_accessibility()?;
        let mut targets = Vec::new();
        for pid in kakao_pids() {
            targets.extend(Kakao::attach(pid)?.targets()?);
        }
        targets.sort_by(|a, b| a.title.cmp(&b.title));
        Ok(targets)
    }
}
