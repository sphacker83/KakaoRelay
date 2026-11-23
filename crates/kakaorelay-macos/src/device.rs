//! 기기 UUID(`IOPlatformUUID`) 조회. `ioreg -rd1 -c IOPlatformExpertDevice`와 같은 값을 IOKit로 직접 읽는다.

use core_foundation::base::{CFType, TCFType, kCFAllocatorDefault};
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::{c_char, c_void};

type IoObject = u32;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> IoObject;
    fn IORegistryEntryCreateCFProperty(
        entry: IoObject,
        key: CFStringRef,
        allocator: *const c_void,
        options: u32,
    ) -> *const c_void;
    fn IOObjectRelease(object: IoObject) -> i32;
}

pub fn platform_uuid() -> Option<String> {
    // SAFETY: IOServiceGetMatchingService가 matching 딕셔너리의 소유권을 가져가고, 서비스 객체는 아래에서 해제한다.
    unsafe {
        let service =
            IOServiceGetMatchingService(0, IOServiceMatching(c"IOPlatformExpertDevice".as_ptr()));
        if service == 0 {
            return None;
        }
        let key = CFString::from_static_string("IOPlatformUUID");
        let value = IORegistryEntryCreateCFProperty(
            service,
            key.as_concrete_TypeRef(),
            kCFAllocatorDefault as _,
            0,
        );
        IOObjectRelease(service);
        if value.is_null() {
            return None;
        }
        let value = CFType::wrap_under_create_rule(value as _);
        value
            .downcast::<CFString>()
            .map(|s| s.to_string())
            .filter(|s| s.len() == 36 && s.is_ascii())
    }
}
