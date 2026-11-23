//! 원본: `DiagnosticReport.cs`. 읽기 전용 진단 보고서, 창·컨트롤 스냅숏, 요약 판정, 보고서 저장.
//! 스냅숏 JSON은 기존 진단 보고서(`ReportStore.JsonOptions`, camelCase)와 같은 필드·순서로 직렬화한다.
//! 보고서에는 메시지 본문·컨트롤 이름·값을 담지 않는다.

use std::path::Path;

use chrono::{DateTime, FixedOffset};
use kakaorelay_ai::text::is_blank;
use kakaorelay_ai::{Result, json};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use crate::fs::{full_path, write_atomic};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSnapshot {
    pub id: i32,
    pub session_id: i32,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeControlSnapshot {
    pub handle: String,
    pub class_name: String,
    pub visible: bool,
    pub enabled: bool,
}

impl NativeControlSnapshot {
    pub fn is_input_candidate(&self) -> bool {
        self.class_name.to_lowercase().contains("edit")
    }
}

// 원본은 계산 속성 `IsInputCandidate`도 JSON에 쓴다.
impl Serialize for NativeControlSnapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("NativeControlSnapshot", 5)?;
        s.serialize_field("handle", &self.handle)?;
        s.serialize_field("className", &self.class_name)?;
        s.serialize_field("visible", &self.visible)?;
        s.serialize_field("enabled", &self.enabled)?;
        s.serialize_field("isInputCandidate", &self.is_input_candidate())?;
        s.end()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutomationControlSnapshot {
    pub depth: i32,
    pub control_type: String,
    pub class_name: String,
    pub framework_id: String,
    pub enabled: bool,
    pub offscreen: bool,
    pub password: bool,
    pub keyboard_focusable: bool,
    pub value_available: bool,
    pub value_read_only: Option<bool>,
    pub text_available: bool,
    pub invoke_available: bool,
}

impl AutomationControlSnapshot {
    pub fn is_input_candidate(&self) -> bool {
        !self.password
            && (self.control_type == "Edit"
                || (self.value_available && self.value_read_only == Some(false)))
    }
}

impl Serialize for AutomationControlSnapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AutomationControlSnapshot", 13)?;
        s.serialize_field("depth", &self.depth)?;
        s.serialize_field("controlType", &self.control_type)?;
        s.serialize_field("className", &self.class_name)?;
        s.serialize_field("frameworkId", &self.framework_id)?;
        s.serialize_field("enabled", &self.enabled)?;
        s.serialize_field("offscreen", &self.offscreen)?;
        s.serialize_field("password", &self.password)?;
        s.serialize_field("keyboardFocusable", &self.keyboard_focusable)?;
        s.serialize_field("valueAvailable", &self.value_available)?;
        s.serialize_field("valueReadOnly", &self.value_read_only)?;
        s.serialize_field("textAvailable", &self.text_available)?;
        s.serialize_field("invokeAvailable", &self.invoke_available)?;
        s.serialize_field("isInputCandidate", &self.is_input_candidate())?;
        s.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WindowSnapshot {
    pub handle: String,
    pub process_id: i32,
    pub title: String,
    pub class_name: String,
    pub visible: bool,
    pub minimized: bool,
    pub enabled: bool,
    pub automation_status: String,
    pub native_children: Vec<NativeControlSnapshot>,
    pub controls: Vec<AutomationControlSnapshot>,
    pub warnings: Vec<String>,
}

impl Default for WindowSnapshot {
    fn default() -> Self {
        Self {
            handle: String::new(),
            process_id: 0,
            title: String::new(),
            class_name: String::new(),
            visible: false,
            minimized: false,
            enabled: false,
            automation_status: "pending".into(),
            native_children: Vec::new(),
            controls: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

impl WindowSnapshot {
    pub fn display_title(&self) -> &str {
        if is_blank(&self.title) {
            "제목 없는 창"
        } else {
            &self.title
        }
    }

    pub fn display_state(&self) -> &'static str {
        if !self.visible {
            "숨김"
        } else if self.minimized {
            "최소화"
        } else {
            "표시됨 · 가려짐 여부 미판정"
        }
    }

    pub fn display_automation_status(&self) -> &str {
        match self.automation_status.as_str() {
            "complete" => "탐색 완료",
            "pending" => "대기",
            "running" => "탐색 중",
            "skipped-hidden" => "숨김 · 생략",
            "skipped-limit" => "개수 제한",
            "limited" => "일부 탐색",
            "stale" => "창 변경됨",
            "interrupted" => "중단",
            "error" => "탐색 오류",
            other => other,
        }
    }
}

impl AutomationControlSnapshot {
    pub fn patterns(&self) -> String {
        [
            self.value_available.then_some("Value"),
            self.text_available.then_some("Text"),
            self.invoke_available.then_some("Invoke"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }

    pub fn candidate_label(&self) -> &'static str {
        if self.is_input_candidate() {
            "입력 후보"
        } else if self.invoke_available {
            "호출 후보"
        } else {
            "—"
        }
    }
}

/// .NET `RuntimeInformation.ProcessArchitecture` 이름.
fn process_architecture() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "X64",
        "aarch64" => "Arm64",
        "x86" => "X86",
        "arm" => "Arm",
        other => other,
    }
    .into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiagnosticReport {
    pub schema_version: i32,
    #[serde(with = "kakaorelay_ai::json::time")]
    pub started_at: DateTime<FixedOffset>,
    #[serde(with = "kakaorelay_ai::json::time::option")]
    pub finished_at: Option<DateTime<FixedOffset>>,
    pub status: String,
    pub stage: String,
    /// 원본 `Environment.OSVersion.VersionString`. 기본값은 OS 이름뿐이며, 플랫폼 worker가 정확한 값을 채운다.
    pub os_version: String,
    pub probe_architecture: String,
    /// 원본은 현재 프로세스의 Windows 세션 ID다. 기본값 0이며 Windows worker가 채운다.
    pub session_id: i32,
    pub read_only: bool,
    pub message_contents_collected: bool,
    pub background_input_tested: bool,
    pub sending_tested: bool,
    pub foreground_window_before: String,
    pub foreground_window_after: String,
    pub processes: Vec<ProcessSnapshot>,
    pub windows: Vec<WindowSnapshot>,
    pub warnings: Vec<String>,
}

impl Default for DiagnosticReport {
    fn default() -> Self {
        Self {
            schema_version: 1,
            started_at: chrono::Local::now().fixed_offset(),
            finished_at: None,
            status: "running".into(),
            stage: "starting".into(),
            os_version: std::env::consts::OS.into(),
            probe_architecture: process_architecture(),
            session_id: 0,
            read_only: true,
            message_contents_collected: false,
            background_input_tested: false,
            sending_tested: false,
            foreground_window_before: String::new(),
            foreground_window_after: String::new(),
            processes: vec![],
            windows: vec![],
            warnings: vec![],
        }
    }
}

impl DiagnosticReport {
    pub fn input_candidate_count(&self) -> usize {
        self.windows
            .iter()
            .map(|w| w.controls.iter().filter(|c| c.is_input_candidate()).count())
            .sum()
    }

    pub fn native_input_candidate_count(&self) -> usize {
        self.windows
            .iter()
            .map(|w| {
                w.native_children
                    .iter()
                    .filter(|c| c.is_input_candidate())
                    .count()
            })
            .sum()
    }

    pub fn invoke_candidate_count(&self) -> usize {
        self.windows
            .iter()
            .map(|w| w.controls.iter().filter(|c| c.invoke_available).count())
            .sum()
    }

    pub fn control_count(&self) -> usize {
        self.windows.iter().map(|w| w.controls.len()).sum()
    }

    pub fn summary(&self) -> &'static str {
        summarize(self)
    }
}

/// 원본 `ReportAssessment.Summarize`.
pub fn summarize(report: &DiagnosticReport) -> &'static str {
    match report.status.as_str() {
        "timeout" => return "응답 지연으로 진단을 종료했습니다. 수집된 결과만 표시합니다.",
        "cancelled" => return "진단을 중지했습니다. 수집된 결과만 표시합니다.",
        "error" => return "진단을 완료하지 못했습니다. 상세 기록을 확인해 주세요.",
        _ => {}
    }
    if report.processes.is_empty() {
        "현재 사용자 세션에서 카카오톡을 찾지 못했습니다."
    } else if report.windows.is_empty() {
        "카카오톡은 실행 중이지만 창을 찾지 못했습니다."
    } else if report.input_candidate_count() > 0 {
        "입력 후보를 발견했습니다. 본문 입력창 식별과 실제 동작 검증이 필요합니다."
    } else if report.native_input_candidate_count() > 0 {
        "Win32 입력 후보가 있습니다. 지원 메시지와 실제 동작은 미검증입니다."
    } else {
        "자동화 입력 후보를 찾지 못했습니다. 창 상태와 컨트롤 구조를 추가 확인해야 합니다."
    }
}

/// 원본 `ReportStore`. camelCase·들여쓰기·한글 비이스케이프, 임시 파일 후 교체.
pub mod report_store {
    use super::*;

    pub fn save(path: &Path, report: &DiagnosticReport) -> Result<()> {
        let full = full_path(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_atomic(&full, json::indented(report).as_bytes(), false)
    }

    pub fn load(path: &Path) -> Result<Option<DiagnosticReport>> {
        if !path.exists() {
            return Ok(None);
        }
        Ok(serde_json::from_str(&kakaorelay_ai::text::read_text(
            path,
        )?)?)
    }
}
