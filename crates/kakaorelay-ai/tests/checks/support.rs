//! 원본 러너(`tests/KakaoRelay.Tests/Program.cs`)의 `Check(condition, description)` 대응.

use std::cell::Cell;
use std::time::Duration;

pub struct Check {
    count: Cell<usize>,
}

impl Check {
    pub fn new() -> Self {
        Self {
            count: Cell::new(0),
        }
    }
    pub fn count(&self) -> usize {
        self.count.get()
    }
    #[track_caller]
    pub fn check(&self, condition: bool, description: &str) {
        if !condition {
            panic!("FAIL {description}");
        }
        self.count.set(self.count.get() + 1);
        println!("PASS {description}");
    }
}

/// 이 테스트 실행 파일 자신. `fake_cli::run`으로 가짜 CLI를 겸한다(원본 `Environment.ProcessPath`).
pub fn fake_exe() -> String {
    std::env::current_exe()
        .expect("실행 파일 경로")
        .to_string_lossy()
        .into_owned()
}

/// 프로세스가 끝났는지 최대 3초 기다려 확인한다. 좀비(회수 전)는 끝난 것으로 본다.
pub fn running(pid: i64) -> bool {
    for _ in 0..60 {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output();
        let state = output
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if state.is_empty() || state.starts_with('Z') {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// 지금 실행 중인지 한 번 확인한다. `ps`를 쓰므로 Unix 전용 검사에서만 호출한다.
#[cfg(unix)]
pub fn running_now(pid: i64) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output();
    let state = output
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    !state.is_empty() && !state.starts_with('Z')
}

pub fn field(json: &str, key: &str) -> i64 {
    serde_json::from_str::<serde_json::Value>(json).expect("JSON")[key]
        .as_i64()
        .expect("정수")
}
