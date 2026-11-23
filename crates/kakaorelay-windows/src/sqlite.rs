//! 복호화한 DB를 Windows 내장 SQLite(`winsqlite3.dll`)의 메모리 DB로 조회한다. 원본: `LocalChatReader.cs`의 `MemoryChatDb`.
//! 평문은 디스크에 쓰지 않는다. rusqlite `bundled`는 맥에서 windows 타깃 C 컴파일(Windows SDK 헤더 필요)이 불가능해
//! 원본과 같은 시스템 DLL을 `raw-dylib`로 직접 호출한다. 가져오기 라이브러리 없이 링크된다.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use zeroize::Zeroizing;

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const OPEN_READWRITE_CREATE: c_int = 6;
/// FREEONCLOSE(1) | READONLY(4). 실패하면 SQLite가 버퍼를 직접 해제한다.
const DESERIALIZE_FLAGS: u32 = 5;

#[link(name = "winsqlite3", kind = "raw-dylib")]
unsafe extern "system" {
    fn sqlite3_open_v2(
        filename: *const c_char,
        db: *mut *mut c_void,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_deserialize(
        db: *mut c_void,
        schema: *const c_char,
        data: *mut u8,
        size: i64,
        capacity: i64,
        flags: u32,
    ) -> c_int;
    fn sqlite3_malloc64(size: u64) -> *mut c_void;
    fn sqlite3_close(db: *mut c_void) -> c_int;
    fn sqlite3_prepare_v2(
        db: *mut c_void,
        sql: *const c_char,
        size: c_int,
        statement: *mut *mut c_void,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_step(statement: *mut c_void) -> c_int;
    fn sqlite3_finalize(statement: *mut c_void) -> c_int;
    fn sqlite3_column_count(statement: *mut c_void) -> c_int;
    fn sqlite3_column_text(statement: *mut c_void, column: c_int) -> *const c_char;
}

/// 원본 `InvalidDataException`·`OutOfMemoryException`에 대응하는 조회 실패.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct SqliteError(pub &'static str);

pub(crate) struct MemoryChatDb {
    db: *mut c_void,
}

impl MemoryChatDb {
    pub(crate) fn open(bytes: Zeroizing<Vec<u8>>) -> Result<Self, SqliteError> {
        let mut db = std::ptr::null_mut();
        // SAFETY: 경로는 NUL로 끝나는 상수이고 db는 유효한 출력 포인터다.
        if unsafe {
            sqlite3_open_v2(
                c":memory:".as_ptr(),
                &mut db,
                OPEN_READWRITE_CREATE,
                std::ptr::null(),
            )
        } != SQLITE_OK
        {
            // 실패해도 핸들이 할당될 수 있어 닫는다.
            let _ = Self { db };
            return Err(SqliteError("SQLite 열기 실패"));
        }
        let this = Self { db };
        // SAFETY: SQLite 할당기로 받은 버퍼에 길이만큼 복사하고, 소유권은 deserialize가 가져간다.
        unsafe {
            let buffer = sqlite3_malloc64(bytes.len() as u64).cast::<u8>();
            if buffer.is_null() {
                return Err(SqliteError("메모리가 부족합니다."));
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
            let size = bytes.len() as i64;
            drop(bytes);
            if sqlite3_deserialize(
                this.db,
                c"main".as_ptr(),
                buffer,
                size,
                size,
                DESERIALIZE_FLAGS,
            ) != SQLITE_OK
            {
                return Err(SqliteError("SQLite 읽기 실패"));
            }
        }
        let check = this.query("PRAGMA quick_check")?;
        if check.len() != 1 || check[0].first().map(String::as_str) != Some("ok") {
            return Err(SqliteError("DB 무결성 검사 실패"));
        }
        Ok(this)
    }

    /// 모든 열을 텍스트로 읽는다. NULL은 빈 문자열이다.
    pub(crate) fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, SqliteError> {
        let sql = CString::new(sql)
            .map_err(|_| SqliteError("카카오톡 DB 스키마가 지원되지 않습니다."))?;
        let mut statement = std::ptr::null_mut();
        // SAFETY: db는 열린 연결이고 sql은 NUL로 끝난다. statement는 아래에서 반드시 finalize한다.
        unsafe {
            if sqlite3_prepare_v2(
                self.db,
                sql.as_ptr(),
                -1,
                &mut statement,
                std::ptr::null_mut(),
            ) != SQLITE_OK
            {
                sqlite3_finalize(statement);
                return Err(SqliteError("카카오톡 DB 스키마가 지원되지 않습니다."));
            }
            let mut rows = Vec::new();
            let status = loop {
                let status = sqlite3_step(statement);
                if status != SQLITE_ROW {
                    break status;
                }
                let columns = sqlite3_column_count(statement);
                let row = (0..columns)
                    .map(|i| {
                        let text = sqlite3_column_text(statement, i);
                        if text.is_null() {
                            String::new()
                        } else {
                            CStr::from_ptr(text).to_string_lossy().into_owned()
                        }
                    })
                    .collect();
                rows.push(row);
            };
            sqlite3_finalize(statement);
            if status != SQLITE_DONE {
                return Err(SqliteError("SQLite 조회 실패"));
            }
            Ok(rows)
        }
    }
}

impl Drop for MemoryChatDb {
    fn drop(&mut self) {
        if !self.db.is_null() {
            // SAFETY: 이 구조체만 연결을 소유하며 한 번만 닫는다.
            unsafe { sqlite3_close(self.db) };
        }
    }
}
