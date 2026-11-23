//! 읽기 전용 진단 수집(UIA 트리 순회). 원본: `DiagnosticCollector.cs`.
//! 버려도 되는 worker 프로세스에서만 호출한다. UIA 제공자가 멈출 수 있어 앱은 시간 초과 시 worker를 종료한다(`ProbeRunner`).
//! 이름·값·문서 텍스트·선택 영역은 읽지 않는다.
//!
//! 보고서 스키마(`DiagnosticReport`)와 저장(`ReportStore`)은 Phase 2 공통 영역이다. 이 모듈은 수집기가 채우는 필드만
//! `DiagnosticProgress`로 돌려주고, 원본이 보고서를 저장하던 시점마다 `checkpoint`를 호출한다.

use crate::catalog::{AutomationControlSnapshot, ProcessSnapshot, WindowSnapshot};
use crate::{native, process};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, UIA_IsInvokePatternAvailablePropertyId,
    UIA_IsTextPatternAvailablePropertyId, UIA_IsValuePatternAvailablePropertyId, UIA_PROPERTY_ID,
    UIA_ValuePatternId,
};

const TOP_LIMIT: usize = 64;
const WINDOW_LIMIT: usize = 12;
const CONTROL_LIMIT: usize = 250;
const DEPTH_LIMIT: i32 = 16;
const TIME_LIMIT: Duration = Duration::from_secs(4);

/// 수집기가 채우는 진단 보고서 필드. 이름과 의미는 원본 `DiagnosticReport`와 같다.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticProgress {
    pub session_id: i32,
    pub status: String,
    pub stage: String,
    pub foreground_window_before: String,
    pub foreground_window_after: String,
    pub processes: Vec<ProcessSnapshot>,
    pub windows: Vec<WindowSnapshot>,
    pub warnings: Vec<String>,
}

/// 원본 `DiagnosticCollector.Collect`. `finishedAt` 기록과 파일 저장은 호출자가 맡는다.
pub fn collect(checkpoint: &mut dyn FnMut(&DiagnosticProgress)) -> DiagnosticProgress {
    let mut report = DiagnosticProgress {
        session_id: process::current_session().map_or(-1, |s| s as i32),
        status: "running".into(),
        stage: "starting".into(),
        foreground_window_before: native::foreground(),
        ..Default::default()
    };
    checkpoint(&report);
    if let Err(kind) = run(&mut report, checkpoint) {
        report.status = "error".into();
        report.warnings.push(format!("진단 오류: {kind}"));
    }
    report.foreground_window_after = native::foreground();
    checkpoint(&report);
    report
}

fn run(
    report: &mut DiagnosticProgress,
    checkpoint: &mut dyn FnMut(&DiagnosticProgress),
) -> Result<(), &'static str> {
    let pids = process::find_by_name(process::KAKAO_PROCESS).map_err(|_| "Win32Exception")?;
    for pid in pids {
        match process::session_of(pid) {
            Some(session) if session as i32 == report.session_id => {
                report.processes.push(ProcessSnapshot {
                    id: pid as i32,
                    session_id: session as i32,
                    version: process::file_version(pid),
                })
            }
            Some(_) => {}
            None => report
                .warnings
                .push("탐색 중 카카오톡 프로세스가 종료되었습니다.".into()),
        }
    }
    report.stage = "native-windows".into();
    let ids: HashSet<i32> = report.processes.iter().map(|p| p.id).collect();
    report.windows = native::enumerate(&ids);
    if report.windows.len() >= TOP_LIMIT {
        report
            .warnings
            .push("최상위 창 탐색이 64개 제한에 도달했습니다.".into());
    }
    checkpoint(report);
    // 원본은 MTA 스레드에서 실행한다. 이미 초기화된 스레드면 실패 코드가 오지만 UIA 생성은 계속 시도한다.
    // SAFETY: 현재 스레드의 COM 초기화다.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: 등록된 CUIAutomation COM 클래스를 생성한다.
    let automation: Result<IUIAutomation, _> =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) };
    let mut inspected = 0;
    for index in 0..report.windows.len() {
        if !report.windows[index].visible {
            report.windows[index].automation_status = "skipped-hidden".into();
            continue;
        }
        if inspected >= WINDOW_LIMIT {
            report.windows[index].automation_status = "skipped-limit".into();
            continue;
        }
        inspected += 1;
        report.stage = format!("uia:{}", report.windows[index].handle);
        report.windows[index].automation_status = "running".into();
        checkpoint(report);
        match &automation {
            Ok(automation) => inspect(automation, report, index, checkpoint),
            Err(_) => {
                report.windows[index].automation_status = "error".into();
                report.windows[index]
                    .warnings
                    .push("UIA 탐색 불가: COMException".into());
            }
        }
        checkpoint(report);
    }
    let partial = !report.warnings.is_empty()
        || report.windows.iter().any(|w| {
            !w.warnings.is_empty()
                || matches!(
                    w.automation_status.as_str(),
                    "error" | "limited" | "skipped-limit" | "stale"
                )
        });
    report.status = if partial { "partial" } else { "complete" }.into();
    report.stage = "finished".into();
    Ok(())
}

/// UIA 컨트롤 형식 ID를 .NET `ControlType.ProgrammaticName`의 접두어 뺀 이름으로 바꾼다.
fn control_type_name(id: i32) -> String {
    const NAMES: [&str; 41] = [
        "Button",
        "Calendar",
        "CheckBox",
        "ComboBox",
        "Edit",
        "Hyperlink",
        "Image",
        "ListItem",
        "List",
        "Menu",
        "MenuBar",
        "MenuItem",
        "ProgressBar",
        "RadioButton",
        "ScrollBar",
        "Slider",
        "Spinner",
        "StatusBar",
        "Tab",
        "TabItem",
        "Text",
        "ToolBar",
        "ToolTip",
        "Tree",
        "TreeItem",
        "Custom",
        "Group",
        "Thumb",
        "DataGrid",
        "DataItem",
        "Document",
        "SplitButton",
        "Window",
        "Pane",
        "Header",
        "HeaderItem",
        "Table",
        "TitleBar",
        "Separator",
        "SemanticZoom",
        "AppBar",
    ];
    usize::try_from(id - 50_000)
        .ok()
        .and_then(|i| NAMES.get(i))
        .map_or_else(|| id.to_string(), |n| (*n).to_string())
}

fn bool_property(
    element: &IUIAutomationElement,
    property: UIA_PROPERTY_ID,
) -> windows::core::Result<bool> {
    // SAFETY: 속성 조회이며 VARIANT 형식을 확인한 뒤 boolVal을 읽는다.
    unsafe {
        let value: VARIANT = element.GetCurrentPropertyValue(property)?;
        let inner = &value.Anonymous.Anonymous;
        if inner.vt != VT_BOOL {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_UNEXPECTED,
            ));
        }
        Ok(inner.Anonymous.boolVal.as_bool())
    }
}

/// 자식·형제가 없으면 UIA는 성공 코드와 null을 돌려준다. windows crate는 이를 S_OK 오류로 바꾸므로 None으로 되돌린다.
fn optional(
    result: windows::core::Result<IUIAutomationElement>,
) -> windows::core::Result<Option<IUIAutomationElement>> {
    match result {
        Ok(element) => Ok(Some(element)),
        Err(error) if error.code().is_ok() => Ok(None),
        Err(error) => Err(error),
    }
}

fn snapshot(
    element: &IUIAutomationElement,
    depth: i32,
) -> windows::core::Result<AutomationControlSnapshot> {
    // SAFETY: 현재 값 조회 함수들이다.
    unsafe {
        let mut entry = AutomationControlSnapshot {
            depth,
            control_type: control_type_name(element.CurrentControlType()?.0),
            class_name: element.CurrentClassName()?.to_string(),
            framework_id: element.CurrentFrameworkId()?.to_string(),
            enabled: element.CurrentIsEnabled()?.as_bool(),
            offscreen: element.CurrentIsOffscreen()?.as_bool(),
            password: element.CurrentIsPassword()?.as_bool(),
            keyboard_focusable: element.CurrentIsKeyboardFocusable()?.as_bool(),
            value_available: bool_property(element, UIA_IsValuePatternAvailablePropertyId)?,
            value_read_only: None,
            text_available: bool_property(element, UIA_IsTextPatternAvailablePropertyId)?,
            invoke_available: bool_property(element, UIA_IsInvokePatternAvailablePropertyId)?,
        };
        if !entry.password
            && entry.value_available
            && let Ok(pattern) =
                element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            && let Ok(read_only) = pattern.CurrentIsReadOnly()
        {
            entry.value_read_only = Some(read_only.as_bool());
        }
        Ok(entry)
    }
}

fn walk(
    automation: &IUIAutomation,
    report: &mut DiagnosticProgress,
    index: usize,
    checkpoint: &mut dyn FnMut(&DiagnosticProgress),
) -> windows::core::Result<()> {
    let handle =
        native::parse(&report.windows[index].handle).map_err(|_| windows::core::Error::empty())?;
    let process_id = report.windows[index].process_id;
    let clock = Instant::now();
    // SAFETY: UIA 조회 함수들이다.
    let (root, walker): (IUIAutomationElement, IUIAutomationTreeWalker) = unsafe {
        (
            automation.ElementFromHandle(handle)?,
            automation.RawViewWalker()?,
        )
    };
    // 반복 깊이 우선 순회로 무제한 자식 열거와 재귀를 피한다.
    let mut stack = vec![(root, 0, false)];
    while !stack.is_empty()
        && report.windows[index].controls.len() < CONTROL_LIMIT
        && clock.elapsed() < TIME_LIMIT
    {
        let (element, depth, include_sibling) = stack.pop().expect("비어 있지 않음");
        // SAFETY: 현재 값 조회 함수다.
        if unsafe { element.CurrentProcessId()? } != process_id {
            continue;
        }
        let entry = snapshot(&element, depth)?;
        let window = &mut report.windows[index];
        window.controls.push(entry);
        if window.controls.len().is_multiple_of(10) {
            checkpoint(report);
        }
        // SAFETY: 트리 탐색 함수들이다.
        unsafe {
            if include_sibling
                && let Some(sibling) = optional(walker.GetNextSiblingElement(&element))?
            {
                stack.push((sibling, depth, true));
            }
            if depth < DEPTH_LIMIT {
                if let Some(child) = optional(walker.GetFirstChildElement(&element))? {
                    stack.push((child, depth + 1, true));
                }
            } else {
                report.windows[index]
                    .warnings
                    .push("UIA 탐색 깊이 제한에 도달했습니다.".into());
            }
        }
    }
    let window = &mut report.windows[index];
    window.automation_status = if !stack.is_empty() || !window.warnings.is_empty() {
        "limited"
    } else {
        "complete"
    }
    .into();
    if !stack.is_empty() {
        window
            .warnings
            .push("UIA 탐색 시간 또는 250개 컨트롤 제한에 도달했습니다.".into());
    }
    Ok(())
}

fn inspect(
    automation: &IUIAutomation,
    report: &mut DiagnosticProgress,
    index: usize,
    checkpoint: &mut dyn FnMut(&DiagnosticProgress),
) {
    let window = &report.windows[index];
    let Ok(handle) = native::parse(&window.handle) else {
        return;
    };
    if !native::exists(handle) || native::process_of(handle) as i32 != window.process_id {
        report.windows[index].automation_status = "stale".into();
        return;
    }
    if walk(automation, report, index, checkpoint).is_err() {
        let window = &mut report.windows[index];
        window.automation_status = "error".into();
        window.warnings.push("UIA 탐색 불가: COMException".into());
    }
}
