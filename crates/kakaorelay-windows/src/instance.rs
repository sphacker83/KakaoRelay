//! 다중 실행 방지. 원본: `App.xaml.cs`의 mutex `Local\KakaoRelay.MainWindow`와 자동 재설정 event `Local\KakaoRelay.ShowWindow`.
//! 두 번째 실행은 event를 신호 상태로 만들고 종료하며, 첫 실행은 신호를 받으면 창을 다시 보인다. 앱 연결은 Phase 5에서 한다.

use std::time::Duration;
use windows::Win32::Foundation::{HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, ReleaseMutex, SetEvent, WaitForSingleObject,
};
use windows::core::{Owned, w};

/// 첫 실행이 소유하는 단일 인스턴스 자원.
/// named mutex 소유권은 획득한 스레드에 묶인다. 해제가 확실하도록 획득한 스레드(보통 메인 스레드)에서 drop한다.
pub struct MainInstance {
    mutex: Owned<HANDLE>,
    show: Owned<HANDLE>,
}

// SAFETY: 커널 객체 핸들은 스레드 간에 공유해도 안전하다. 대기·신호 함수는 동시 호출을 허용한다.
unsafe impl Send for MainInstance {}
// SAFETY: 위와 같다. 공유 참조로는 event 대기만 한다.
unsafe impl Sync for MainInstance {}

pub enum InstanceStart {
    /// 이 프로세스가 첫 실행이다.
    Primary(MainInstance),
    /// 이미 실행 중인 앱에 창 표시를 요청했다. 호출자는 종료한다.
    AlreadyRunning,
}

/// 원본 순서대로 event를 먼저 만들고 mutex를 즉시(0ms) 획득한다. 버려진 mutex도 획득으로 본다.
pub fn acquire_main_instance() -> windows::core::Result<InstanceStart> {
    // SAFETY: 이름은 상수 문자열이고 반환 핸들은 Owned가 닫는다.
    unsafe {
        let show = Owned::new(CreateEventW(
            None,
            false,
            false,
            w!("Local\\KakaoRelay.ShowWindow"),
        )?);
        let mutex = Owned::new(CreateMutexW(
            None,
            false,
            w!("Local\\KakaoRelay.MainWindow"),
        )?);
        let wait = WaitForSingleObject(*mutex, 0);
        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            Ok(InstanceStart::Primary(MainInstance { mutex, show }))
        } else {
            SetEvent(*show)?;
            Ok(InstanceStart::AlreadyRunning)
        }
    }
}

impl MainInstance {
    /// 다른 실행이 창 표시를 요청하면 true. 시간 초과면 false. 자동 재설정 event라 신호 한 번에 한 번만 true다.
    pub fn wait_for_show(&self, timeout: Duration) -> bool {
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: 살아 있는 event 핸들에 대한 대기다.
        unsafe { WaitForSingleObject(*self.show, millis) == WAIT_OBJECT_0 }
    }
}

impl Drop for MainInstance {
    fn drop(&mut self) {
        // 다른 스레드에서 drop하면 해제가 실패하고, mutex는 소유 스레드가 끝날 때 버려진 상태가 된다.
        // SAFETY: 획득한 mutex를 한 번 해제한다. 핸들은 이후 Owned가 닫는다.
        let _ = unsafe { ReleaseMutex(*self.mutex) };
    }
}
