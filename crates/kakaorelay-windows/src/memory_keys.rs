//! 카카오톡 프로세스 메모리에서 SQLCipher 원시 키 후보(`x'<hex>'`)를 찾는다. 원본: `LocalChatReader.cs`의 `KakaoMemoryKeys`.
//! 후보 판정은 OS와 무관하므로 cfg 없이 두고, 메모리 읽기만 Windows 전용이다.

use zeroize::Zeroizing;

pub type KeyCandidate = Zeroizing<[u8; 32]>;

/// 버퍼의 `x'<64hex>'` 또는 `x'<96hex>'`(키+salt)에서 앞 64자리를 키 후보로 모은다. 중복은 넣지 않는다.
pub fn scan_key_candidates(buffer: &[u8], keys: &mut Vec<KeyCandidate>) {
    let count = buffer.len();
    let mut i = 0;
    while i + 67 <= count {
        let at = i;
        i += 1;
        if !matches!(buffer[at], b'x' | b'X') || buffer[at + 1] != b'\'' {
            continue;
        }
        let length = if at + 99 <= count && buffer[at + 98] == b'\'' {
            96
        } else if buffer[at + 66] == b'\'' {
            64
        } else {
            continue;
        };
        let hex = &buffer[at + 2..at + 2 + length];
        if !hex.iter().all(u8::is_ascii_hexdigit) {
            continue;
        }
        let mut key = Zeroizing::new([0u8; 32]);
        if hex::decode_to_slice(&hex[..64], key.as_mut()).is_err() {
            continue;
        }
        if !keys.iter().any(|known| **known == *key) {
            keys.push(key);
        }
    }
}

#[cfg(windows)]
pub(crate) use win::read;

#[cfg(windows)]
mod win {
    use super::{KeyCandidate, scan_key_candidates};
    use crate::process;
    use kakaorelay_ai::ApiFailure;
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Memory::{
        MEM_COMMIT, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS,
        VirtualQueryEx,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };
    use windows::core::Owned;
    use zeroize::Zeroize;

    const CHUNK: usize = 1_048_576;
    /// 원본처럼 100바이트를 겹쳐 읽어 청크 경계에 걸친 키 문자열도 찾는다.
    const STEP: usize = 1_048_476;
    const TOTAL_LIMIT: usize = 512 * 1024 * 1024;

    /// 모든 `KakaoTalk` 프로세스의 커밋된 개인 메모리를 읽어 키 후보를 모은다. 원본 `KakaoMemoryKeys.Read`.
    pub(crate) fn read() -> Result<Vec<KeyCandidate>, ApiFailure> {
        let processes = process::find_by_name(process::KAKAO_PROCESS).map_err(|e| {
            ApiFailure::new(
                500,
                "unknown_result",
                format!("프로세스 목록 조회 실패: {e}"),
            )
        })?;
        if processes.is_empty() {
            return Err(ApiFailure::new(
                409,
                "kakao_not_running",
                "카카오톡을 실행하고 대화방을 열어주세요.",
            ));
        }
        let mut keys = Vec::new();
        let mut read_any = false;
        let mut total = 0usize;
        let mut buffer = vec![0u8; CHUNK];
        for pid in processes {
            // SAFETY: 반환 핸들은 Owned가 닫는다.
            let Ok(handle) =
                (unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) })
            else {
                continue;
            };
            let handle = unsafe { Owned::new(handle) };
            let mut address = 0usize;
            loop {
                let mut info = MEMORY_BASIC_INFORMATION::default();
                // SAFETY: info는 크기를 넘겨준 유효한 출력 버퍼다.
                let size = unsafe {
                    VirtualQueryEx(
                        *handle,
                        Some(address as *const _),
                        &mut info,
                        size_of::<MEMORY_BASIC_INFORMATION>(),
                    )
                };
                if size == 0 {
                    break;
                }
                let base = info.BaseAddress as usize;
                if info.RegionSize == 0 || base + info.RegionSize <= address {
                    break;
                }
                address = base + info.RegionSize;
                if info.State != MEM_COMMIT
                    || info.Type != MEM_PRIVATE
                    || (info.Protect.0 & (PAGE_NOACCESS.0 | PAGE_GUARD.0)) != 0
                {
                    continue;
                }
                let mut offset = 0usize;
                while offset < info.RegionSize {
                    let length = CHUNK.min(info.RegionSize - offset);
                    total += length;
                    if total > TOTAL_LIMIT {
                        buffer.zeroize();
                        return Ok(keys);
                    }
                    let mut count = 0usize;
                    // SAFETY: buffer는 length 이상 크기의 쓰기 가능한 버퍼다.
                    let ok = unsafe {
                        ReadProcessMemory(
                            *handle,
                            (base + offset) as *const _,
                            buffer.as_mut_ptr().cast(),
                            length,
                            Some(&mut count),
                        )
                    };
                    if ok.is_ok() {
                        read_any = true;
                        scan_key_candidates(&buffer[..count.min(length)], &mut keys);
                    }
                    // 실패한 읽기도 일부를 썼을 수 있어 매번 지운다.
                    buffer[..length].zeroize();
                    offset += STEP;
                }
            }
        }
        buffer.zeroize();
        if !read_any {
            return Err(ApiFailure::new(
                409,
                "memory_unavailable",
                "카카오톡 메모리를 읽을 수 없습니다. 앱과 카카오톡의 실행 권한을 확인하세요.",
            ));
        }
        Ok(keys)
    }
}
