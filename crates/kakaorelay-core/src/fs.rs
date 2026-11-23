//! 원장·상태 파일의 원자적 쓰기와 발송 잠금(inventory §4-4).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use kakaorelay_ai::settings::path_with_suffix;
use kakaorelay_ai::{Error, Result};

/// `.tmp`에 쓴 뒤 덮어쓰기 이동. `durable`이면 원본의 `FileOptions.WriteThrough` + `Flush(true)`처럼 디스크까지 내린다.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], durable: bool) -> Result<()> {
    let temporary = path_with_suffix(path, ".tmp");
    {
        let mut file = File::create(&temporary)?;
        file.write_all(bytes)?;
        if durable {
            file.sync_all()?;
        }
    }
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// 원본 `FileMode.CreateNew` + WriteThrough. 이미 있으면 `AlreadyExists` 입출력 오류다.
pub(crate) fn create_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// 원본 `Path.GetFullPath`. 링크는 따라가지 않고 `.`·`..`만 정리한다.
pub fn full_path(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// 원본 `Path.IsPathFullyQualified`. Windows에서는 드라이브 상대 경로(`C:a`)와 루트 상대 경로(`\a`)를 거부한다.
pub fn is_fully_qualified(path: &str) -> bool {
    if cfg!(windows) {
        let bytes = path.as_bytes();
        let separator = |b: u8| b == b'\\' || b == b'/';
        (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && separator(bytes[2]))
            || (bytes.len() >= 2 && separator(bytes[0]) && separator(bytes[1]))
    } else {
        path.starts_with('/')
    }
}

/// 원본 named mutex `Local\KakaoRelay.TestSend`의 OS 공통 대체. 원장 폴더의 잠금 파일을 비차단으로 잡는다.
/// 파일 잠금은 프로세스 사이뿐 아니라 같은 프로세스의 다른 파일 핸들 사이에도 배타적이다. drop 시 풀린다.
pub(crate) struct SendLock(#[allow(dead_code)] File);

impl SendLock {
    pub(crate) fn acquire(ledger: &Path, busy: &str) -> Result<Self> {
        std::fs::create_dir_all(ledger)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(ledger.join("send.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Self(file)),
            Err(std::fs::TryLockError::WouldBlock) => Err(Error::invalid_operation(busy)),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }
}
