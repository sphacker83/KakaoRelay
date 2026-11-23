//! 프로세스 조회. 원본의 `Process.GetProcessesByName`, `Process.SessionId`, `MainModule.FileVersionInfo`.

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::core::{HSTRING, Owned, PWSTR};

pub(crate) const KAKAO_PROCESS: &str = "KakaoTalk";

/// 실행 중인 프로세스의 (PID, 확장자 없는 실행 파일 이름). .NET `ProcessName`과 같은 형태다.
fn list() -> windows::core::Result<Vec<(u32, String)>> {
    // SAFETY: 스냅숏 핸들은 Owned가 닫고, entry는 dwSize를 채운 유효한 버퍼다.
    unsafe {
        let snapshot = Owned::new(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut result = Vec::new();
        let mut next = Process32FirstW(*snapshot, &mut entry);
        while next.is_ok() {
            let length = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let exe = String::from_utf16_lossy(&entry.szExeFile[..length]);
            let name = match exe.rsplit_once('.') {
                Some((stem, ext)) if ext.eq_ignore_ascii_case("exe") => stem.to_string(),
                _ => exe,
            };
            result.push((entry.th32ProcessID, name));
            next = Process32NextW(*snapshot, &mut entry);
        }
        Ok(result)
    }
}

/// 이름이 같은(대소문자 무시) 프로세스 PID. 원본 `GetProcessesByName`.
pub(crate) fn find_by_name(name: &str) -> windows::core::Result<Vec<u32>> {
    Ok(list()?
        .into_iter()
        .filter(|(_, n)| n.eq_ignore_ascii_case(name))
        .map(|(pid, _)| pid)
        .collect())
}

/// 실행 중이면 확장자 없는 실행 파일 이름. 원본 `Process.GetProcessById(...).ProcessName`.
pub(crate) fn name_of(pid: u32) -> windows::core::Result<Option<String>> {
    Ok(list()?.into_iter().find(|(p, _)| *p == pid).map(|(_, n)| n))
}

pub(crate) fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0u32;
    // SAFETY: session은 유효한 출력 포인터다.
    unsafe { ProcessIdToSessionId(pid, &mut session) }
        .ok()
        .map(|_| session)
}

pub(crate) fn current_session() -> Option<u32> {
    // SAFETY: 인자 없는 조회 함수다.
    session_of(unsafe { GetCurrentProcessId() })
}

/// 원본 `MainModule.FileVersionInfo.FileVersion ?? "unknown"`, 조회 실패는 `"unavailable"`.
pub(crate) fn file_version(pid: u32) -> String {
    match image_path(pid) {
        Ok(path) => read_file_version(&path).unwrap_or_else(|| "unknown".into()),
        Err(_) => "unavailable".into(),
    }
}

fn image_path(pid: u32) -> windows::core::Result<String> {
    // SAFETY: 핸들은 Owned가 닫고, buffer와 size는 일치한다.
    unsafe {
        let process = Owned::new(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?);
        let mut buffer = [0u16; 32_768];
        let mut size = buffer.len() as u32;
        QueryFullProcessImageNameW(
            *process as HANDLE,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )?;
        Ok(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

fn read_file_version(path: &str) -> Option<String> {
    let path = HSTRING::from(path);
    // SAFETY: data는 GetFileVersionInfoSizeW가 알려 준 크기이고, VerQueryValueW의 결과 포인터는 data 안을 가리킨다.
    unsafe {
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()).ok()?;
        let mut pointer = std::ptr::null_mut();
        let mut length = 0u32;
        // 첫 번역(언어·코드 페이지)을 쓰고, 없으면 .NET과 같은 기본값 후보를 차례로 시도한다.
        let mut tables = Vec::new();
        if VerQueryValueW(
            data.as_ptr().cast(),
            &HSTRING::from("\\VarFileInfo\\Translation"),
            &mut pointer,
            &mut length,
        )
        .as_bool()
            && length >= 4
        {
            let pair = std::slice::from_raw_parts(pointer as *const u16, 2);
            tables.push(format!("{:04X}{:04X}", pair[0], pair[1]));
        }
        tables.extend(["040904B0", "040904E4", "04090000"].map(String::from));
        for table in tables {
            let query = HSTRING::from(format!("\\StringFileInfo\\{table}\\FileVersion"));
            if VerQueryValueW(data.as_ptr().cast(), &query, &mut pointer, &mut length).as_bool()
                && length > 0
            {
                let text = std::slice::from_raw_parts(pointer as *const u16, length as usize);
                let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
                return Some(String::from_utf16_lossy(&text[..end]));
            }
        }
        None
    }
}
