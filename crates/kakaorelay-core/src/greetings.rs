//! 자동인사. 원본에는 없는 기능이다(사용자 요청 2026-11-23).
//!
//! 방별로 켠 자동답장 방에서, 인사 구간(한국 시간 0시·12시에 바뀌는 반나절) 안에 처음 말한 사람에게 `🤖 <닉네임 두 글자> 안녕!`을 보낸다.
//! 닉네임 속 두 자리 출생연도가 82 이하이거나 연도가 없으면 `안녕하세요`로 높인다.
//! 설정은 원본 형식을 지키는 `ai-settings.json`과 분리해 `auto-greetings.json`에 둔다.

use std::path::PathBuf;

use chrono::{FixedOffset, NaiveDate};
use kakaorelay_ai::{LocalMessage, LocalRoom, Result, json};
use serde::{Deserialize, Serialize};

use crate::ids::MessageId;

/// 이 연도(두 자리) 이하로 태어난 사람에게는 높임말로 인사한다. 30 미만은 2000년대생으로 본다.
const POLITE_UNTIL_YEAR: u32 = 82;
const CENTURY_SPLIT: u32 = 30;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GreetingSettings {
    /// 자동인사를 켠 방의 키(`settings::room_key`).
    pub rooms: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GreetingStore {
    path: PathBuf,
}

impl GreetingStore {
    pub fn default_path() -> PathBuf {
        kakaorelay_ai::paths::app_data().join("auto-greetings.json")
    }

    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 파일이 없으면 모두 꺼진 설정이다.
    pub fn load(&self) -> Result<GreetingSettings> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ok(serde_json::from_str(&text)
                .map_err(|e| kakaorelay_ai::Error::io(format!("자동인사 설정 오류: {e}")))?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GreetingSettings::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn enabled(&self, room_key: &str) -> Result<bool> {
        Ok(self.load()?.rooms.iter().any(|k| k == room_key))
    }

    pub fn set(&self, room_key: &str, enabled: bool) -> Result<GreetingSettings> {
        let mut settings = self.load()?;
        settings.rooms.retain(|k| k != room_key);
        if enabled {
            settings.rooms.push(room_key.to_owned());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = self.path.with_extension("json.tmp");
        std::fs::write(
            &temp,
            serde_json::to_string_pretty(&settings)
                .map_err(|e| kakaorelay_ai::Error::io(e.to_string()))?,
        )?;
        std::fs::rename(&temp, &self.path)?;
        Ok(settings)
    }
}

fn is_hangul(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c)
}

/// 닉네임의 앞 두 글자(한글 음절 우선). 예: `하늘83검단` → `하늘`, `🔥바다/85` → `바다`.
pub fn short_name(nickname: &str) -> String {
    let hangul: String = nickname.chars().filter(|c| is_hangul(*c)).take(2).collect();
    if hangul.chars().count() == 2 {
        return hangul;
    }
    let plain: String = nickname
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect();
    if plain.is_empty() {
        nickname.trim().chars().take(2).collect()
    } else {
        plain
    }
}

/// 닉네임 안의 첫 두 자리 숫자(앞뒤가 숫자가 아닌 것). 세 자리 이상 숫자는 연도로 보지 않는다.
pub fn birth_year(nickname: &str) -> Option<u32> {
    let chars: Vec<char> = nickname.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i - start == 2 {
                return chars[start..i].iter().collect::<String>().parse().ok();
            }
        } else {
            i += 1;
        }
    }
    None
}

pub fn greeting_text(nickname: &str) -> String {
    let polite =
        birth_year(nickname).is_none_or(|y| (CENTURY_SPLIT..=POLITE_UNTIL_YEAR).contains(&y));
    format!(
        "🤖 {} {}",
        short_name(nickname),
        if polite { "안녕하세요" } else { "안녕!" }
    )
}

/// 인사 구간. 한국 시간 0시~11시 59분이 오전, 12시~23시 59분이 오후다(사용자 결정 2026-11-23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GreetingPeriod {
    pub day: NaiveDate,
    pub afternoon: bool,
}

impl std::fmt::Display for GreetingPeriod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}-{}",
            self.day,
            if self.afternoon { "pm" } else { "am" }
        )
    }
}

pub fn greeting_period(time: chrono::DateTime<FixedOffset>) -> GreetingPeriod {
    use chrono::Timelike;
    let kst = FixedOffset::east_opt(9 * 3600).expect("KST");
    let local = time.with_timezone(&kst);
    GreetingPeriod {
        day: local.date_naive(),
        afternoon: local.hour() >= 12,
    }
}

fn period_of(message: &LocalMessage) -> GreetingPeriod {
    greeting_period(message.time)
}

/// 새 메시지(`watermark` 이후) 중, 현재 인사 구간(`greeting_period`)에 그 방에서 처음 말한 사람의 첫 메시지. 나·삭제·시스템(type 0) 메시지는 뺀다.
/// 같은 구간에 앞서 말한 기록이 읽은 범위 안에 있으면(봇을 켜기 전 포함) 인사하지 않는다.
pub fn first_speakers(
    messages: &[LocalMessage],
    watermark: &str,
    self_id: &str,
    period: GreetingPeriod,
) -> Result<Vec<LocalMessage>> {
    let watermark = MessageId::parse(watermark)?;
    let mut ordered = Vec::with_capacity(messages.len());
    for message in messages {
        ordered.push((MessageId::parse(&message.id)?, message));
    }
    ordered.sort_by(|a, b| a.0.cmp(&b.0));
    let mut spoke: Vec<&str> = vec![];
    let mut found = vec![];
    for (id, message) in ordered {
        if message.author_id == self_id
            || message.deleted
            || message.kind == 0
            || period_of(message) != period
        {
            continue;
        }
        if spoke.contains(&message.author_id.as_str()) {
            continue;
        }
        spoke.push(&message.author_id);
        if id > watermark {
            found.push(message.clone());
        }
    }
    Ok(found)
}

/// 같은 방·사람·구간은 항상 같은 요청 ID다. 발송 원장이 중복을 막으므로 재시작해도 두 번 인사하지 않는다.
pub fn request_id(room: &LocalRoom, author_id: &str, period: GreetingPeriod) -> String {
    let hash = json::hash(&format!(
        "{}:{}:{author_id}:{period}",
        room.profile, room.id
    ));
    format!("greet-{}", &hash[..40])
}
