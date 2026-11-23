//! 단일 인스턴스. 원본: mutex `Local\KakaoRelay.MainWindow`와 자동 재설정 event `Local\KakaoRelay.ShowWindow`.
//! 두 번째 실행은 기존 창 표시를 요청하고 종료한다.
//! - Windows: `kakaorelay_windows::instance`(같은 이름의 커널 객체라 원본 앱과도 서로 배제된다).
//! - macOS: 앱 데이터 폴더의 잠금 파일(`flock`)과 유닉스 소켓. 잠금은 프로세스가 끝나면 커널이 풀어 비정상 종료 뒤에도 남지 않는다.

pub enum Start {
    Primary(Guard),
    AlreadyRunning,
}

#[cfg(windows)]
pub struct Guard(std::sync::Arc<kakaorelay_windows::instance::MainInstance>);

#[cfg(windows)]
pub fn acquire() -> std::io::Result<Start> {
    use kakaorelay_windows::instance::{InstanceStart, acquire_main_instance};
    match acquire_main_instance().map_err(std::io::Error::other)? {
        InstanceStart::Primary(main) => Ok(Start::Primary(Guard(std::sync::Arc::new(main)))),
        InstanceStart::AlreadyRunning => Ok(Start::AlreadyRunning),
    }
}

#[cfg(windows)]
impl Guard {
    /// 창 표시 요청을 기다리는 스레드를 시작한다.
    /// mutex 해제는 획득한 주 스레드의 drop이어야 해서 리스너는 약한 참조만 잡는다.
    pub fn listen(&self, show: impl Fn() + Send + 'static) {
        let instance = std::sync::Arc::downgrade(&self.0);
        std::thread::spawn(move || {
            while let Some(main) = instance.upgrade() {
                let requested = main.wait_for_show(std::time::Duration::from_secs(1));
                drop(main);
                if requested {
                    show();
                }
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub struct Guard {
    _lock: std::fs::File,
    socket: std::path::PathBuf,
}

#[cfg(target_os = "macos")]
pub fn acquire() -> std::io::Result<Start> {
    use std::os::fd::AsRawFd;
    let root = kakaorelay_ai::paths::app_data();
    std::fs::create_dir_all(&root)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("main-window.lock"))?;
    let socket = root.join("show-window.sock");
    // SAFETY: 열린 파일 기술자에 대한 비차단 배타 잠금이다.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        // 주 인스턴스가 소켓을 열기 전이면 연결이 실패할 수 있다. 이 경우에도 종료한다(원본도 신호만 보내고 끝난다).
        let _ = std::os::unix::net::UnixStream::connect(&socket);
        return Ok(Start::AlreadyRunning);
    }
    Ok(Start::Primary(Guard {
        _lock: lock,
        socket,
    }))
}

#[cfg(target_os = "macos")]
impl Guard {
    pub fn listen(&self, show: impl Fn() + Send + 'static) {
        let _ = std::fs::remove_file(&self.socket);
        let listener = match std::os::unix::net::UnixListener::bind(&self.socket) {
            Ok(listener) => listener,
            Err(_) => return,
        };
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stream.is_ok() {
                    show();
                }
            }
        });
    }
}

#[cfg(target_os = "macos")]
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}
