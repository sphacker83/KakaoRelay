//! 카카오톡 맥 DB 열기. SQLCipher compatibility 3 원본을 읽기 전용으로 직접 연다.
//!
//! 방식 선택 근거:
//! - DB가 약 1GB라 폴링(5초)마다 전체 복호화·스냅샷 복사를 할 수 없다. SQLCipher가 필요한 페이지만 복호화한다.
//! - `mode=ro`로 DB·WAL을 `O_RDONLY`로 열고, `readonly_shm=1`로 `-shm`도 읽기 전용으로 연다.
//!   SQLite는 읽기 전용 `-shm`에서 read-mark를 기록하지 않고 WAL을 직접 읽어 최신 커밋을 반영한다.
//!   SQLite는 읽기 전용 연결이어도 `-wal`을 `O_RDWR`로 열지만(`sqlite3WalOpen`), 쓰기 트랜잭션을 시작할 수 없고
//!   `-shm`이 읽기 전용이면 체크포인트가 `SQLITE_READONLY`로 거부된다. 닫을 때의 체크포인트 시도(배타 잠금 포함)는
//!   `SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE`로 아예 끈다. 그래서 카카오톡 파일을 쓰지 않는다.
//! - 같은 프로세스의 연결끼리는 `-shm` 매핑을 공유한다. 이 프로세스에서 카카오톡 DB를 읽기·쓰기로 여는 연결을 만들면
//!   `readonly_shm`이 무력해지므로 모든 연결은 `open`으로만 만든다.
//! - 매 조회는 새 읽기 트랜잭션이라 카카오톡이 WAL에 커밋한 새 메시지가 다음 폴링에 보인다.

use std::path::{Path, PathBuf};

use kakaorelay_ai::ApiFailure;
use rusqlite::{Connection, OpenFlags};

pub const CONTAINER: &str = "Library/Containers/com.kakao.KakaoTalkMac/Data/Library";

pub fn database_dir(home: &Path) -> PathBuf {
    home.join(CONTAINER)
        .join("Application Support/com.kakao.KakaoTalkMac")
}

pub fn preferences_plist(home: &Path) -> PathBuf {
    home.join(CONTAINER)
        .join("Preferences/com.kakao.KakaoTalkMac.plist")
}

pub fn full_disk_access_failure() -> ApiFailure {
    ApiFailure::new(
        409,
        "full_disk_access_required",
        "카카오톡 데이터를 읽을 권한이 없습니다. 시스템 설정 > 개인정보 보호 및 보안 > 전체 디스크 접근 권한에서 KakaoRelay를 허용한 뒤 다시 실행하세요.",
    )
}

fn io_failure(error: std::io::Error) -> ApiFailure {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => full_disk_access_failure(),
        std::io::ErrorKind::NotFound => ApiFailure::new(
            404,
            "kakao_data_missing",
            "카카오톡 로컬 데이터가 없습니다. 맥용 카카오톡에 로그인했는지 확인하세요.",
        ),
        _ => ApiFailure::new(
            409,
            "local_unavailable",
            format!("카카오톡 데이터 폴더를 읽지 못했습니다: {error}"),
        ),
    }
}

/// DB 후보 파일. 카카오톡은 계정마다 해시 이름의 DB 하나와 `-wal`/`-shm`을 둔다.
pub fn database_candidates(dir: &Path) -> Result<Vec<PathBuf>, ApiFailure> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.contains('-') || name.contains('.') {
            continue;
        }
        let meta = entry.metadata().map_err(io_failure)?;
        if meta.is_file() && meta.len() >= 1024 && meta.len() % 1024 == 0 {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}

/// 경로를 SQLite URI로 만든다. 데이터 폴더 경로에는 공백("Application Support")이 있다.
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                uri.push(byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri.push_str("?mode=ro&readonly_shm=1");
    uri
}

/// 키를 적용해 읽기 전용으로 연다. 키가 틀리면 첫 조회에서 실패한다(SQLCipher의 1페이지 HMAC 검증).
pub fn open(path: &Path, passphrase: &str) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(
        file_uri(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        true,
    )?;
    connection.pragma_update(None, "key", passphrase)?;
    connection.pragma_update(None, "cipher_compatibility", 3)?;
    connection.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_escapes_spaces_and_sets_read_only() {
        assert_eq!(
            file_uri(Path::new("/a b/c%d?")),
            "file:/a%20b/c%25d%3F?mode=ro&readonly_shm=1"
        );
    }
}
