//! 권한 진단. 앱 첫 실행·진단 화면에서 상태와 설정 위치를 보여 주는 데 쓴다.

use std::path::Path;

use crate::{ax, db};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataAccess {
    Allowed,
    /// 전체 디스크 접근 권한이 없어 카카오톡 컨테이너를 읽지 못한다.
    Denied,
    /// 카카오톡 데이터 폴더가 없다(미설치·미로그인).
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionStatus {
    /// 손쉬운 사용: '창' 메뉴 조회·AXPress·합성 키 입력에 필요하다.
    pub accessibility: bool,
    /// 전체 디스크 접근: 카카오톡 DB·설정 읽기에 필요하다. DB 폴더 읽기 실패 형태로 판별한다.
    pub data_access: DataAccess,
    /// 화면 기록: 대화창 CGWindowID(`handle`)를 창 이름으로 찾고, 입력 직전 가장 앞 창이 대상인지 확인한다.
    /// 발송에는 필수이고(없으면 handle `0x0`, 발송 중단), 읽기에는 필요 없다.
    pub screen_recording: bool,
}

impl PermissionStatus {
    /// 사용자에게 보여 줄 부족한 권한 안내.
    pub fn problems(&self) -> Vec<&'static str> {
        let mut problems = Vec::new();
        if !self.accessibility {
            problems.push(
                "손쉬운 사용 권한이 필요합니다: 시스템 설정 > 개인정보 보호 및 보안 > 손쉬운 사용",
            );
        }
        if !self.screen_recording {
            problems.push("발송에는 화면 기록 권한이 필요합니다: 시스템 설정 > 개인정보 보호 및 보안 > 화면 및 시스템 오디오 녹음");
        }
        match self.data_access {
            DataAccess::Denied => {
                problems.push("전체 디스크 접근 권한이 필요합니다: 시스템 설정 > 개인정보 보호 및 보안 > 전체 디스크 접근 권한")
            }
            DataAccess::Missing => problems.push("맥용 카카오톡 데이터가 없습니다. 카카오톡에 로그인했는지 확인하세요."),
            DataAccess::Allowed => {}
        }
        problems
    }
}

pub fn data_access(home: &Path) -> DataAccess {
    match std::fs::read_dir(db::database_dir(home)) {
        Ok(_) => DataAccess::Allowed,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DataAccess::Missing,
        Err(_) => DataAccess::Denied,
    }
}

pub fn permission_status() -> PermissionStatus {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    PermissionStatus {
        accessibility: ax::is_trusted(),
        data_access: data_access(&home),
        screen_recording: ax::screen_capture_allowed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_container_is_reported_as_missing() {
        let home = std::env::temp_dir().join("kr-macos-no-such-home");
        assert_eq!(data_access(&home), DataAccess::Missing);
        let status = PermissionStatus {
            accessibility: false,
            data_access: DataAccess::Denied,
            screen_recording: false,
        };
        assert_eq!(status.problems().len(), 3);
    }
}
