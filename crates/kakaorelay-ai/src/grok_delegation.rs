//! 원본: `GrokDelegation.cs`.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::text::{is_blank, utf16_len};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokTask {
    #[serde(default)]
    pub task: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokCompletion {
    pub job_id: String,
    pub request: String,
    pub task: GrokTask,
    pub result: String,
}

pub const INSTRUCTIONS: &str = "너는 대화 담당 Codex 코디네이터다. 일상 대화, 간단한 질문과 코딩은 직접 답하라. 현재 요청이 복잡한 분석, 심도 깊은 추론, 어려운 코딩이며 전문가 검토가 필요하면 Grok에 위임할 수 있다. 위임할 때 먼저 사용자에게 짧게 검토를 맡기고 대화를 계속할 수 있다고 알린 뒤, 마지막 줄(감정 표시 앞)에 [[delegate_grok:{\"task\":\"필요한 문맥·조건·코드를 포함한 독립적인 작업 설명\",\"reason\":\"위임 이유\"}]]를 한 번만 붙여라. task와 reason은 각각 JSON 문자열이어야 한다. 관련 없는 대화나 개인정보는 포함하지 마라. Grok은 별도 백그라운드 작업이고 결과가 오면 다시 전달받는다. 아직 완료됐다고 말하지 마라. 과거 요청이나 진행 중인 작업을 중복 위임하지 마라. 이미지 생성과 위임을 동시에 요청하지 마라. 매 턴의 delegationAllowed가 false이면 위임 표시를 절대 출력하지 마라.";

/// 문자열 끝의 표시(`(?:^|\r?\n)\[\[name:(.+)\]\]\s*$`)를 찾는다. .NET `$`는 끝 줄바꿈 하나 앞에서도 맞는다.
pub(crate) fn trailing_marker(text: &str, name: &str) -> Option<(usize, String)> {
    static CACHE: LazyLock<std::sync::Mutex<std::collections::HashMap<String, regex::Regex>>> =
        LazyLock::new(Default::default);
    let regex = CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(name.to_string())
        .or_insert_with(|| {
            regex::Regex::new(&format!(r"(?:^|\r?\n)\[\[{name}:(.+)\]\]\s*\n?\z")).expect("정규식")
        })
        .clone();
    // `.`은 줄바꿈을 넘지 않고 탐욕적이라 .NET과 같은 첫 일치를 찾는다.
    regex
        .captures(text)
        .map(|c| (c.get(0).map(|m| m.start()).unwrap_or(0), c[1].to_string()))
}

pub fn parse(text: &str) -> Result<(String, Option<GrokTask>)> {
    let Some((index, payload)) = trailing_marker(text, "delegate_grok") else {
        if text.contains("[[delegate_grok:") {
            return Err(Error::invalid_data(
                "Grok 위임 요청 형식이 올바르지 않습니다.",
            ));
        }
        return Ok((text.trim().to_string(), None));
    };
    let invalid = || Error::invalid_data("Grok 위임 작업과 이유를 확인하세요.");
    let mut value: serde_json::Value = serde_json::from_str(&payload)?;
    crate::json::normalize_keys(&mut value, &["task", "reason"], &[]);
    let task: Option<GrokTask> = serde_json::from_value(value)?;
    let answer = text[..index].trim().to_string();
    let task = task.ok_or_else(invalid)?;
    if is_blank(&task.task)
        || utf16_len(&task.task) > 30000
        || is_blank(&task.reason)
        || utf16_len(&task.reason) > 1000
        || task.task.contains('\0')
        || answer.contains("[[delegate_grok:")
    {
        return Err(invalid());
    }
    Ok((answer, Some(task)))
}
