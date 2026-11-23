//! 외부 CLI 실행 공통부. 원본은 `Process.Kill(entireProcessTree: true)`로 자식까지 종료한다.
//! Unix는 새 프로세스 그룹으로 띄워 그룹 전체에 SIGKILL을 보내고, Windows는 Job Object로 묶어 한꺼번에 끝낸다.
//! 미래(future)가 취소되어 버려질 때도 남지 않도록 `Drop`에서 같은 종료를 한다.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use crate::error::{Error, Result};

/// 출력은 최대 2,000,000자(UTF-16)까지만 보관한다(원본 `DrainAsync`).
pub const MAX_OUTPUT_CHARS: usize = 2_000_000;

/// 자식 환경에서 지우는 상위 에이전트 세션 표시.
pub const SESSION_MARKERS: [&str; 3] = [
    "CLAUDECODE",
    "CODEX_THREAD_ID",
    "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub remove_env: Vec<String>,
}

impl CliCommand {
    pub fn new(program: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: vec![],
            cwd: cwd.into(),
            env: vec![],
            remove_env: vec![],
        }
    }
    pub fn arg(&mut self, value: impl Into<String>) -> &mut Self {
        self.args.push(value.into());
        self
    }
    pub fn set_env(&mut self, key: &str, value: impl Into<String>) {
        self.env.retain(|(k, _)| k != key);
        self.remove_env.retain(|k| k != key);
        self.env.push((key.into(), value.into()));
    }
    pub fn remove(&mut self, key: &str) {
        self.env.retain(|(k, _)| k != key);
        self.remove_env.push(key.into());
    }
    pub fn env_value(&self, key: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    /// 실행될 환경에 이 키가 남는지(상속 포함).
    pub fn has_env(&self, key: &str) -> bool {
        if self.env.iter().any(|(k, _)| k == key) {
            return true;
        }
        !self.remove_env.iter().any(|k| k == key) && std::env::var_os(key).is_some()
    }

    /// 창 없이(Windows `CREATE_NO_WINDOW`) 표준 입출력을 모두 연결해 실행한다.
    pub fn spawn(&self) -> Result<ChildProcess> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in &self.remove_env {
            command.env_remove(key);
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let child = command
            .spawn()
            .map_err(|e| Error::invalid_operation(format!("CLI 시작 실패: {e}")))?;
        ChildProcess::new(child)
    }
}

pub struct ChildProcess {
    child: Child,
    pid: Option<u32>,
    exited: bool,
    #[cfg(windows)]
    job: windows_job::Job,
}

impl ChildProcess {
    fn new(child: Child) -> Result<Self> {
        let pid = child.id();
        #[cfg(windows)]
        let job = windows_job::Job::assign(&child)?;
        Ok(Self {
            child,
            pid,
            exited: false,
            #[cfg(windows)]
            job,
        })
    }

    pub fn id(&self) -> Option<u32> {
        self.pid
    }
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    pub fn has_exited(&mut self) -> bool {
        if !self.exited {
            self.exited = matches!(self.child.try_wait(), Ok(Some(_)));
        }
        self.exited
    }

    /// 아직 실행 중이면 프로세스 트리 전체를 종료한다.
    pub fn kill_tree(&mut self) {
        if self.has_exited() {
            return;
        }
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            // SAFETY: 이 프로세스가 `process_group(0)`으로 만든 그룹 ID(=자식 PID)에만 신호를 보낸다.
            unsafe {
                libc::killpg(pid as libc::pid_t, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        self.job.terminate();
        let _ = self.child.start_kill();
    }

    pub async fn wait(&mut self) -> Result<i32> {
        let status = self.child.wait().await?;
        self.exited = true;
        Ok(status.code().unwrap_or(-1))
    }

    /// 최대 `timeout` 동안 종료를 기다린다(원본 `WaitForExit(3000)`).
    pub async fn wait_for(&mut self, timeout: std::time::Duration) {
        let _ = tokio::time::timeout(timeout, self.wait()).await;
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.kill_tree();
    }
}

/// 출력 끝까지 읽되 앞부분 2,000,000자만 보관한다. UTF-8로 해석하고 BOM은 버린다.
pub async fn drain<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> String {
    let mut bytes = Vec::new();
    let mut buffer = vec![0u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if bytes.len() < MAX_OUTPUT_CHARS * 4 {
                    bytes.extend_from_slice(&buffer[..n]);
                }
            }
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    crate::text::utf16_prefix(text, MAX_OUTPUT_CHARS).to_string()
}

/// 폴더를 지우되 실패는 무시한다(원본 `try { Directory.Delete(...) } catch (IOException) {}`).
pub fn remove_dir_quietly(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

#[cfg(windows)]
mod windows_job {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    use crate::error::{Error, Result};

    /// 자식과 그 자손을 묶는 Job. 핸들을 닫으면(KILL_ON_JOB_CLOSE) 남은 프로세스도 끝난다.
    pub struct Job(HANDLE);

    // SAFETY: Job 핸들은 프로세스 전역 커널 객체이며 스레드 사이에서 써도 된다.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn assign(child: &tokio::process::Child) -> Result<Self> {
            let process = child
                .raw_handle()
                .ok_or_else(|| Error::invalid_operation("CLI 프로세스 핸들 없음"))?;
            // SAFETY: Win32 호출에 유효한 포인터와 방금 만든 핸들만 넘긴다.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Err(Error::invalid_operation("Job Object를 만들 수 없습니다."));
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                AssignProcessToJobObject(job, process as HANDLE);
                Ok(Self(job))
            }
        }
        pub fn terminate(&self) {
            // SAFETY: 이 객체가 소유한 Job 핸들이다.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: 한 번만 닫는다.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
