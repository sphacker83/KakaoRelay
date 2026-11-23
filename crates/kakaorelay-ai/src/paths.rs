//! 저장 경로 결정. 모든 기본 경로는 이 모듈에서만 정한다(rust-port-context "중요한 의사결정").
//!
//! - 앱 데이터(설정·원장·페르소나 이미지·정리 상태): Windows `%LOCALAPPDATA%/KakaoRelay`,
//!   macOS `~/Library/Application Support/KakaoRelay`.
//! - 프로바이더 데이터(`data/providers`): Windows는 실행 파일 옆 `data/`(기존 위치),
//!   그 밖의 OS는 서명된 번들에 쓸 수 없어 앱 데이터 아래 `data/`.

use std::path::PathBuf;
use std::sync::OnceLock;

struct Roots {
    app_data: PathBuf,
    provider_data: PathBuf,
}

static ROOTS: OnceLock<Roots> = OnceLock::new();

/// 기본 경로 대신 쓸 위치를 한 번만 고정한다. 테스트 격리와 휴대용 실행에 쓴다.
/// 이미 고정됐거나 기본 경로를 쓰기 시작한 뒤라면 `false`다.
pub fn set_roots(app_data: PathBuf, provider_data: PathBuf) -> bool {
    ROOTS
        .set(Roots {
            app_data,
            provider_data,
        })
        .is_ok()
}

fn roots() -> &'static Roots {
    ROOTS.get_or_init(|| {
        let app_data = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("KakaoRelay");
        let provider_data = if cfg!(windows) {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|p| p.join("data")))
                .unwrap_or_else(|| app_data.join("data"))
        } else {
            app_data.join("data")
        };
        Roots {
            app_data,
            provider_data,
        }
    })
}

/// 원본 `Environment.SpecialFolder.LocalApplicationData/KakaoRelay`.
pub fn app_data() -> PathBuf {
    roots().app_data.clone()
}

/// 원본 `AppContext.BaseDirectory/data`.
pub fn provider_data() -> PathBuf {
    roots().provider_data.clone()
}

/// 원본 `Environment.SpecialFolder.UserProfile`.
pub fn user_profile() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// 원본 `Environment.SpecialFolder.ApplicationData`. .NET은 Unix에서 `~/.config`로 본다.
pub fn roaming_app_data() -> PathBuf {
    if cfg!(windows) {
        dirs::config_dir().unwrap_or_default()
    } else {
        user_profile().join(".config")
    }
}

/// 원본 `Environment.SpecialFolder.LocalApplicationData`(KakaoRelay 하위가 아닌 루트). Windows Codex 설치 탐색에 쓴다.
pub fn local_app_data_root() -> PathBuf {
    dirs::data_local_dir().unwrap_or_default()
}
