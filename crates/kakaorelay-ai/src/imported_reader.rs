//! 원본: `ImportedChatReader.cs`. 계정 자격 증명이나 플랫폼 DB 없이 쓰는 이식용 입력 어댑터.
//! 카카오톡 한국어 TXT 내보내기(PC·모바일)와 KakaoRelay JSON을 읽는다.

use std::sync::{LazyLock, Mutex};

use async_trait::async_trait;
use chrono::{Duration, FixedOffset, Local, NaiveDate, TimeZone};
use indexmap::IndexMap;
use serde_json::Value;

use crate::chat_models::{ApiFailure, ChatContext, ChatReader, LocalMessage, LocalRoom};
use crate::error::{Error, Result};
use crate::text::{is_blank, utf16_len};

#[derive(Default)]
pub struct ImportedChatReader {
    contexts: Mutex<IndexMap<String, ChatContext>>,
}

fn ends_with_json(name: &str) -> bool {
    name.to_lowercase().ends_with(".json")
}

/// `JsonElement.ToString()`: 문자열은 값 그대로, 나머지는 원문 JSON.
fn element_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

impl ImportedChatReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn import(&self, filename: &str, content: &str) -> Result<LocalRoom> {
        if utf16_len(content) > 10_000_000 {
            return Err(Error::argument("파일은 10MB 이하로 가져오세요."));
        }
        if ends_with_json(filename)
            && let Value::Array(rows) = serde_json::from_str::<Value>(content)?
        {
            let mut groups: IndexMap<String, Vec<Value>> = IndexMap::new();
            for row in rows {
                let key = row.get("roomId").map(element_text).unwrap_or_default();
                groups.entry(key).or_default().push(row);
            }
            if groups.len() > 1 {
                let mut first = None;
                for (key, group) in groups {
                    let name = match group[0].get("room") {
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(Value::Null) => None,
                        Some(other) => {
                            return Err(Error::invalid_operation(format!(
                                "room 값이 문자열이 아닙니다: {other}"
                            )));
                        }
                        None => Some(key),
                    };
                    let imported = self.import(
                        &format!("{}.json", name.unwrap_or_else(|| "대화".into())),
                        &serde_json::to_string(&group)?,
                    )?;
                    first.get_or_insert(imported);
                }
                return first.ok_or_else(|| Error::invalid_operation("가져온 대화가 없습니다."));
            }
        }
        let stem = std::path::Path::new(filename)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let title = if is_blank(&stem) {
            "가져온 대화".to_string()
        } else {
            stem
        };
        let id = crate::json::hash(content)[..24].to_string();
        let room = LocalRoom::new("import", &id, title, true);
        let messages = if ends_with_json(filename) {
            parse_json(content)?
        } else {
            parse_text(content)?
        };
        if messages.is_empty() {
            return Err(Error::argument(
                "메시지를 인식하지 못했습니다. 카카오톡 한국어 TXT 내보내기 또는 KakaoRelay JSON을 사용하세요.",
            ));
        }
        let context = ChatContext {
            room: room.clone(),
            messages,
            read_at: Local::now().fixed_offset(),
        };
        self.contexts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, context);
        Ok(room)
    }
}

#[async_trait]
impl ChatReader for ImportedChatReader {
    async fn rooms(&self) -> std::result::Result<Vec<LocalRoom>, ApiFailure> {
        Ok(self
            .contexts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|c| c.room.clone())
            .collect())
    }

    async fn read(
        &self,
        profile: &str,
        room_id: &str,
        limit: usize,
    ) -> std::result::Result<ChatContext, ApiFailure> {
        if !(1..=500).contains(&limit) {
            return Err(ApiFailure::new(
                400,
                "invalid_request",
                "메시지 수는 1~500입니다.",
            ));
        }
        let contexts = self.contexts.lock().unwrap_or_else(|e| e.into_inner());
        let context = contexts
            .get(room_id)
            .filter(|_| profile == "import")
            .ok_or_else(|| ApiFailure::new(404, "room_missing", "대화를 먼저 가져오세요."))?;
        let messages = context.messages[context.messages.len().saturating_sub(limit)..].to_vec();
        Ok(ChatContext {
            room: context.room.clone(),
            messages,
            read_at: Local::now().fixed_offset(),
        })
    }
}

fn parse_json(content: &str) -> Result<Vec<LocalMessage>> {
    let document: Value = serde_json::from_str(content)?;
    let rows = match &document {
        Value::Array(rows) => rows,
        Value::Object(map) => match map.get("messages") {
            Some(Value::Array(rows)) => rows,
            Some(_) => return Err(Error::invalid_operation("messages가 배열이 아닙니다.")),
            None => return Err(Error::KeyNotFound("messages".into())),
        },
        _ => {
            return Err(Error::invalid_operation(
                "JSON 최상위 형식이 올바르지 않습니다.",
            ));
        }
    };
    let mut result = vec![];
    for row in rows {
        let get = |keys: &[&str]| {
            keys.iter()
                .find_map(|k| row.get(*k).filter(|v| !v.is_null()).map(element_text))
                .unwrap_or_default()
        };
        let time = crate::json::time::parse(&get(&["timeKST", "time"]))
            .ok_or_else(|| Error::argument("JSON 메시지 시각이 올바르지 않습니다."))?;
        let id = get(&["messageId", "id"]);
        let id = if id.is_empty() {
            (result.len() + 1).to_string()
        } else {
            id
        };
        let kind = get(&["type"]).trim().parse::<i32>().unwrap_or(1);
        let deleted = matches!(get(&["deleted"]).as_str(), "1" | "true" | "True");
        result.push(LocalMessage::new(
            id,
            get(&["authorId"]),
            get(&["author"]),
            time,
            kind,
            get(&["message", "text"]),
            deleted,
        ));
    }
    Ok(result)
}

static DATE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?<y>\d{4})년\s*(?<m>\d{1,2})월\s*(?<d>\d{1,2})일").expect("정규식")
});
static WINDOWS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"^\[(?<name>.+?)\]\s*\[(?<ampm>오전|오후)\s*(?<h>\d{1,2}):(?<min>\d{2})\]\s?(?<text>.*)$",
    )
    .expect("정규식")
});
static MOBILE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^\d{4}년\s*\d{1,2}월\s*\d{1,2}일\s*(?<ampm>오전|오후)\s*(?<h>\d{1,2}):(?<min>\d{2}),\s*(?<name>.+?)\s*:\s?(?<text>.*)$").expect("정규식")
});

fn number(captures: &regex::Captures, name: &str) -> Result<u32> {
    captures[name].parse::<u32>().map_err(|_| {
        Error::Format(format!(
            "숫자 형식이 올바르지 않습니다: {}",
            &captures[name]
        ))
    })
}

fn parse_text(content: &str) -> Result<Vec<LocalMessage>> {
    let mut messages: Vec<LocalMessage> = vec![];
    let mut date: Option<NaiveDate> = None;
    let kst = FixedOffset::east_opt(9 * 3600).expect("오프셋");
    for line in content.replace("\r\n", "\n").split('\n') {
        // .NET `$`는 끝의 `\n` 앞에서도 맞지만 줄을 `\n`으로 나눴으므로 차이가 없다. `\d`는 유니코드 숫자까지 포함한다.
        let mobile = MOBILE.captures(line);
        let matched = if mobile.is_some() {
            MOBILE.captures(line)
        } else {
            WINDOWS.captures(line)
        };
        if let Some(day) = DATE.captures(line) {
            let heading = (line.starts_with("---") && line.ends_with("---")) || line == &day[0];
            if mobile.is_some() || heading {
                let (y, m, d) = (number(&day, "y")?, number(&day, "m")?, number(&day, "d")?);
                date = Some(
                    NaiveDate::from_ymd_opt(y as i32, m, d)
                        .ok_or_else(|| Error::argument("날짜가 올바르지 않습니다."))?,
                );
                if matched.is_none() {
                    continue;
                }
            }
        }
        match (&matched, date) {
            (Some(m), Some(date)) => {
                let (hour, minute) = (number(m, "h")?, number(m, "min")?);
                if !(1..=12).contains(&hour) || minute > 59 {
                    return Err(Error::argument("TXT 시각을 인식할 수 없습니다."));
                }
                let hour = hour % 12 + if &m["ampm"] == "오후" { 12 } else { 0 };
                let author = m["name"].to_string();
                let local = date.and_hms_opt(0, 0, 0).expect("자정")
                    + Duration::hours(hour as i64)
                    + Duration::minutes(minute as i64);
                let at = kst
                    .from_local_datetime(&local)
                    .single()
                    .ok_or_else(|| Error::argument("TXT 시각을 인식할 수 없습니다."))?;
                messages.push(LocalMessage::new(
                    (messages.len() + 1).to_string(),
                    &author,
                    &author,
                    at,
                    1,
                    &m["text"],
                    false,
                ));
            }
            _ => {
                if let Some(last) = messages.last_mut() {
                    last.text = format!("{}\n{line}", last.text);
                }
            }
        }
    }
    Ok(messages)
}
