//! 접근성(AX)·창 목록·세션 상태 FFI. 맥 카카오톡은 AX에 메뉴바만 노출하므로 메뉴와 앱 단위 속성만 다룬다.

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;

pub const KAKAO_BUNDLE_ID: &str = "com.kakao.KakaoTalkMac";

type AxElementRef = *const c_void;

const AX_ATTRIBUTE_UNSUPPORTED: i32 = -25205;
const AX_NO_VALUE: i32 = -25212;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> AxElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AxElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementSetAttributeValue(
        element: AxElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
    fn AXUIElementPerformAction(element: AxElementRef, action: CFStringRef) -> i32;
    fn AXUIElementSetMessagingTimeout(element: AxElementRef, seconds: f32) -> i32;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFTypeRef;
    fn CGPreflightScreenCaptureAccess() -> bool;
}

pub fn is_trusted() -> bool {
    // SAFETY: 인자 없는 조회 함수다.
    unsafe { AXIsProcessTrusted() }
}

/// 화면 기록 권한. 다른 앱 창 이름(`kCGWindowName`)을 읽는 데 필요하다.
pub fn screen_capture_allowed() -> bool {
    // SAFETY: 인자 없는 조회 함수이며 권한 요청 대화상자를 띄우지 않는다.
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// 화면 잠금·로그인 창 상태. 세션 정보를 읽을 수 없으면 잠긴 것으로 본다.
pub fn screen_locked() -> bool {
    // SAFETY: Copy 규칙으로 받은 딕셔너리를 create 규칙으로 감싸 해제한다.
    let raw = unsafe { CGSessionCopyCurrentDictionary() };
    if raw.is_null() {
        return true;
    }
    let dict: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_create_rule(raw as _) };
    let flag = |name: &'static str| {
        dict.find(CFString::from_static_string(name)).and_then(|v| {
            v.downcast::<CFBoolean>().map(bool::from).or_else(|| {
                v.downcast::<CFNumber>()
                    .and_then(|n| n.to_i64())
                    .map(|n| n != 0)
            })
        })
    };
    flag("CGSSessionScreenIsLocked").unwrap_or(false)
        || flag("kCGSSessionOnConsoleKey") == Some(false)
}

/// AX 요소. CF 객체로 보관해 해제를 맡긴다.
#[derive(Clone)]
pub struct Element(CFType);

impl Element {
    fn wrap(raw: AxElementRef) -> Option<Self> {
        // SAFETY: Create 함수가 돌려준 +1 참조다.
        (!raw.is_null()).then(|| Self(unsafe { CFType::wrap_under_create_rule(raw as CFTypeRef) }))
    }

    fn raw(&self) -> AxElementRef {
        self.0.as_CFTypeRef()
    }

    pub fn application(pid: i32) -> Option<Self> {
        // SAFETY: pid만 받는 생성 함수다.
        let element = Self::wrap(unsafe { AXUIElementCreateApplication(pid) })?;
        // 응답하지 않는 앱 때문에 발송 구간이 길어지지 않도록 기본(6초)보다 짧게 둔다.
        // SAFETY: 유효한 요소다.
        unsafe { AXUIElementSetMessagingTimeout(element.raw(), 2.0) };
        Some(element)
    }

    pub fn attribute(&self, name: &str) -> Result<CFType, i32> {
        let name = CFString::new(name);
        let mut value: CFTypeRef = std::ptr::null();
        // SAFETY: 성공하면 value는 +1 참조다.
        let status = unsafe {
            AXUIElementCopyAttributeValue(self.raw(), name.as_concrete_TypeRef(), &mut value)
        };
        if status != 0 || value.is_null() {
            return Err(status);
        }
        Ok(unsafe { CFType::wrap_under_create_rule(value) })
    }

    /// 문자열 속성. 값 없음·미지원만 `None`이고, 응답 실패는 오류다(제목을 놓치면 동명 판정이 틀어진다).
    pub fn string(&self, name: &str) -> Result<Option<String>, i32> {
        match self.attribute(name) {
            Ok(value) => Ok(value.downcast::<CFString>().map(|s| s.to_string())),
            Err(AX_NO_VALUE | AX_ATTRIBUTE_UNSUPPORTED) => Ok(None),
            Err(code) => Err(code),
        }
    }

    pub fn child(&self, name: &str) -> Result<Element, i32> {
        let value = self.attribute(name)?;
        // SAFETY: AX 요소 속성 값은 AXUIElement다. get 규칙으로 참조를 하나 더 잡는다.
        Ok(Self(unsafe {
            CFType::wrap_under_get_rule(value.as_CFTypeRef())
        }))
    }

    pub fn children(&self) -> Result<Vec<Element>, i32> {
        let value = match self.attribute("AXChildren") {
            Ok(value) => value,
            Err(AX_NO_VALUE | AX_ATTRIBUTE_UNSUPPORTED) => return Ok(Vec::new()),
            Err(code) => return Err(code),
        };
        if value.type_of() != CFArray::<CFType>::type_id() {
            return Ok(Vec::new());
        }
        // SAFETY: 타입 ID를 확인한 CFArray이며, get 규칙으로 참조를 하나 더 잡는다.
        let array: CFArray<CFType> =
            unsafe { CFArray::wrap_under_get_rule(value.as_CFTypeRef() as _) };
        Ok(array.iter().map(|item| Self(item.clone())).collect())
    }

    pub fn press(&self) -> Result<(), i32> {
        let action = CFString::from_static_string("AXPress");
        // SAFETY: 유효한 요소와 문자열이다.
        match unsafe { AXUIElementPerformAction(self.raw(), action.as_concrete_TypeRef()) } {
            0 => Ok(()),
            status => Err(status),
        }
    }

    pub fn set_frontmost(&self) -> Result<(), i32> {
        let name = CFString::from_static_string("AXFrontmost");
        // SAFETY: 유효한 요소와 CFBoolean 값이다.
        match unsafe {
            AXUIElementSetAttributeValue(
                self.raw(),
                name.as_concrete_TypeRef(),
                CFBoolean::true_value().as_CFTypeRef(),
            )
        } {
            0 => Ok(()),
            status => Err(status),
        }
    }
}

/// 메뉴 항목 하나. 분류는 `catalog`가 순수 함수로 한다.
#[derive(Clone)]
pub struct MenuItem {
    pub title: String,
    pub identifier: Option<String>,
    pub element: Element,
}

/// 메뉴바의 '창' 메뉴 항목. 메뉴 제목은 시스템 언어에 따라 '창' 또는 'Window'다.
/// 카카오톡이 잠시 AX에 응답하지 않을 때(-25204)가 있어 짧게 재시도한다.
pub fn window_menu_items(app: &Element) -> Result<Vec<MenuItem>, String> {
    let mut last = 0;
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        match read_window_menu(app) {
            Ok(Some(items)) => return Ok(items),
            Ok(None) => return Err("카카오톡 '창' 메뉴를 찾지 못했습니다".into()),
            Err(code) => last = code,
        }
    }
    Err(format!(
        "카카오톡 '창' 메뉴를 읽지 못했습니다(AX 오류 {last}). 카카오톡이 응답하는지 확인하세요."
    ))
}

fn read_window_menu(app: &Element) -> Result<Option<Vec<MenuItem>>, i32> {
    let bar = app.child("AXMenuBar")?;
    let mut window = None;
    for top in bar.children()? {
        if matches!(top.string("AXTitle")?.as_deref(), Some("창" | "Window")) {
            window = Some(top);
            break;
        }
    }
    let Some(top) = window else { return Ok(None) };
    let Some(menu) = top.children()?.into_iter().next() else {
        return Ok(None);
    };
    let mut items = Vec::new();
    for element in menu.children()? {
        items.push(MenuItem {
            title: element.string("AXTitle")?.unwrap_or_default(),
            identifier: element.string("AXIdentifier")?,
            element,
        });
    }
    Ok(Some(items))
}

/// CGWindowList의 창 하나. 이름은 화면 기록 권한이 있을 때만 채워진다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: u32,
    pub pid: i32,
    pub layer: i64,
    pub name: Option<String>,
    pub on_screen: bool,
}

/// 화면에 표시된 창 목록(앞에서 뒤 순서).
/// `kCGWindowListOptionAll`은 실측(2026-11-23)에서 z-order 순서가 아니어서 전면 판정이 틀렸다.
/// 앞에서 뒤 순서는 화면 표시 목록(`OnScreenOnly`)에서만 보장된다. 최소화된 창은 목록에 없다.
pub fn window_list() -> Vec<WindowInfo> {
    use core_graphics::display::CGWindowListCopyWindowInfo;
    use core_graphics::window::{kCGNullWindowID, kCGWindowListOptionOnScreenOnly};
    // SAFETY: Copy 규칙으로 받은 배열을 create 규칙으로 감싼다.
    let raw =
        unsafe { CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID) };
    if raw.is_null() {
        return Vec::new();
    }
    let array: CFArray<CFDictionary<CFString, CFType>> =
        unsafe { CFArray::wrap_under_create_rule(raw) };
    array
        .iter()
        .filter_map(|dict| {
            let number = |key: &'static str| {
                dict.find(CFString::from_static_string(key))
                    .and_then(|v| v.downcast::<CFNumber>())
                    .and_then(|n| n.to_i64())
            };
            Some(WindowInfo {
                id: u32::try_from(number("kCGWindowNumber")?).ok()?,
                pid: i32::try_from(number("kCGWindowOwnerPID")?).ok()?,
                layer: number("kCGWindowLayer").unwrap_or(0),
                name: dict
                    .find(CFString::from_static_string("kCGWindowName"))
                    .and_then(|v| v.downcast::<CFString>())
                    .map(|s| s.to_string()),
                on_screen: dict
                    .find(CFString::from_static_string("kCGWindowIsOnscreen"))
                    .and_then(|v| v.downcast::<CFBoolean>())
                    .is_some_and(bool::from),
            })
        })
        .collect()
}
