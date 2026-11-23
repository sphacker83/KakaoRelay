//! 테스트 보조. 원본 테스트 실행 파일이 가짜 CLI를 겸하던 방식(`tests/KakaoRelay.Tests/Program.cs:5-18`)은
//! `fake_cli` 모듈로 옮겼다. 실행 파일 `kakaorelay-fake-cli`로 쓰거나, `harness = false` 테스트가
//! 자기 실행 파일을 가짜 CLI로 겸할 때 `fake_cli::run`을 먼저 호출한다.

pub mod fake_cli;
pub mod pdf;

use std::path::{Path, PathBuf};

/// 테스트별 임시 폴더. drop 시 삭제한다.
pub struct TempDir(PathBuf);

impl TempDir {
    /// macOS 임시 폴더는 `/var` → `/private/var` 링크 아래에 있어, 링크를 거부하는 보관함 검사와 부딪치지 않도록 실제 경로를 쓴다.
    pub fn new(label: &str) -> Self {
        let temp = std::env::temp_dir();
        let path = std::fs::canonicalize(&temp)
            .unwrap_or(temp)
            .join("KakaoRelay.Tests")
            .join(format!("{label}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&path).expect("임시 폴더 생성");
        Self(path)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
