//! System.Text.Json과 바이트 단위로 같은 출력. 해시 키(방 키, 세션 폴더, 지문, 체크포인트 ID)와
//! 기존 설정·상태 파일 호환이 이 출력에 달려 있다(inventory §4-0).
//!
//! - `Escape::Default`: 옵션 없는 `JsonSerializer.Serialize`. 출력 가능한 ASCII만 그대로 두고
//!   `"&'+<>\`` `` ` ``과 나머지는 `\uXXXX`(대문자)로 바꾼다.
//! - `Escape::Prompt`: `PromptJson.Encoder`(`JavaScriptEncoder.Create(UnicodeRanges.All)`). 한글 등은 그대로 둔다.
//! - 짧은 이스케이프는 `\b \t \n \f \r \\`뿐이고 `"`는 `"`다. BMP 밖 문자는 항상 서로게이트 쌍으로 쓴다.
//! - 들여쓰기는 공백 2칸, 줄바꿈은 .NET `Environment.NewLine`과 같다.

use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone};
use serde::Serialize;
use serde_json::Value;

use crate::json_table::PROMPT_ALLOWED;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escape {
    Default,
    Prompt,
}

const NEW_LINE: &str = if cfg!(windows) { "\r\n" } else { "\n" };

fn allowed(c: char, escape: Escape) -> bool {
    let code = c as u32;
    match escape {
        Escape::Default => {
            (0x20..=0x7E).contains(&code)
                && !matches!(c, '"' | '&' | '\'' | '+' | '<' | '>' | '\\' | '`')
        }
        Escape::Prompt => {
            if code > 0xFFFF {
                return false;
            }
            let code = code as u16;
            let index = PROMPT_ALLOWED.partition_point(|&(_, end)| end < code);
            PROMPT_ALLOWED
                .get(index)
                .is_some_and(|&(start, _)| start <= code)
        }
    }
}

/// STJ는 `DateTimeOffset`·`DateOnly` 값을 인코더를 거치지 않고 쓴다(`+09:00`의 `+`가 그대로 남는다).
/// serde 값에는 형식 정보가 없어 .NET 시각 형식과 정확히 같은 문자열을 시각으로 본다.
/// 한계: 사용자가 쓴 본문이 이 형식과 완전히 같으면 .NET(`+`)과 달라진다.
fn is_dotnet_time(text: &str) -> bool {
    let b = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        b.get(range)
            .is_some_and(|s| s.iter().all(u8::is_ascii_digit))
    };
    if b.len() < 25
        || !digits(0..4)
        || b[4] != b'-'
        || !digits(5..7)
        || b[7] != b'-'
        || !digits(8..10)
        || b[10] != b'T'
    {
        return false;
    }
    if !digits(11..13) || b[13] != b':' || !digits(14..16) || b[16] != b':' || !digits(17..19) {
        return false;
    }
    let mut i = 19;
    if b[i] == b'.' {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if !(1..=7).contains(&(i - start)) {
            return false;
        }
    }
    b.len() == i + 6
        && b[i] == b'+'
        && digits(i + 1..i + 3)
        && b[i + 3] == b':'
        && digits(i + 4..i + 6)
}

pub fn write_string(out: &mut String, text: &str, escape: Escape) {
    if is_dotnet_time(text) {
        out.push('"');
        out.push_str(text);
        out.push('"');
        return;
    }
    out.push('"');
    for c in text.chars() {
        if allowed(c, escape) {
            out.push(c);
            continue;
        }
        match c {
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{:04X}", unit));
                }
            }
        }
    }
    out.push('"');
}

fn write_value(out: &mut String, value: &Value, escape: Escape, indent: Option<usize>) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(out, s, escape),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if let Some(level) = indent {
                    out.push_str(NEW_LINE);
                    out.push_str(&"  ".repeat(level + 1));
                }
                write_value(out, item, escape, indent.map(|l| l + 1));
            }
            if let Some(level) = indent {
                out.push_str(NEW_LINE);
                out.push_str(&"  ".repeat(level));
            }
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if let Some(level) = indent {
                    out.push_str(NEW_LINE);
                    out.push_str(&"  ".repeat(level + 1));
                }
                write_string(out, key, escape);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_value(out, item, escape, indent.map(|l| l + 1));
            }
            if let Some(level) = indent {
                out.push_str(NEW_LINE);
                out.push_str(&"  ".repeat(level));
            }
            out.push('}');
        }
    }
}

pub fn to_string(value: &Value, escape: Escape, indented: bool) -> String {
    let mut out = String::new();
    write_value(&mut out, value, escape, indented.then_some(0));
    out
}

pub fn value<T: Serialize + ?Sized>(value: &T) -> Value {
    serde_json::to_value(value).expect("직렬화 가능한 값")
}

/// 원본 C# 형식의 기본 이름 정책(PascalCase)으로 키를 바꾼다. camelCase 필드명의 첫 글자만 올린다.
/// 문자로 시작하는 사전 키가 있는 형식에는 쓰지 않는다(사전 키는 원본에서도 바뀌지 않는다).
pub fn pascal(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let mut chars = k.chars();
                    let key = match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => k,
                    };
                    (key, pascal(v))
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(pascal).collect()),
        other => other,
    }
}

/// `PromptJson.Serialize`. 익명 객체처럼 이름이 그대로인 값.
pub fn prompt<T: Serialize + ?Sized>(v: &T) -> String {
    to_string(&value(v), Escape::Prompt, false)
}

/// `PromptJson.Serialize(형식 인스턴스)`. C# 속성 이름(PascalCase)을 쓴다.
pub fn prompt_pascal<T: Serialize + ?Sized>(v: &T) -> String {
    to_string(&pascal(value(v)), Escape::Prompt, false)
}

/// 옵션 없는 `JsonSerializer.Serialize`. 이름은 값 그대로다.
pub fn default<T: Serialize + ?Sized>(v: &T) -> String {
    to_string(&value(v), Escape::Default, false)
}

/// 옵션 없는 `JsonSerializer.Serialize(형식 인스턴스)`. C# 속성 이름(PascalCase)을 쓴다.
pub fn default_pascal<T: Serialize + ?Sized>(v: &T) -> String {
    to_string(&pascal(value(v)), Escape::Default, false)
}

/// `JsonSerializerDefaults.Web` + 들여쓰기 + `PromptJson.Encoder`. 설정·라이브러리·정리 상태 파일 형식.
/// 같은 출력이 `ReportStore.JsonOptions`(camelCase + 들여쓰기 + `PromptJson.Encoder`)에도 맞다.
pub fn indented<T: Serialize + ?Sized>(v: &T) -> String {
    to_string(&value(v), Escape::Prompt, true)
}

/// `Convert.ToHexString(SHA256(UTF-8))`. 대문자 hex 64자.
pub fn hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    upper_hex(&Sha256::digest(text.as_bytes()))
}

pub fn upper_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// `JsonSerializerDefaults.Web` 역직렬화의 대소문자 무시를 흉내 낸다. 객체 키를 `known`의 표기로 맞춘다.
/// `dictionaries`에 든 필드의 값은 사전이라 그 키는 바꾸지 않는다.
pub fn normalize_keys(value: &mut Value, known: &[&str], dictionaries: &[&str]) {
    match value {
        Value::Object(map) => {
            let entries = std::mem::take(map);
            for (key, mut item) in entries {
                let canonical = known
                    .iter()
                    .find(|k| k.eq_ignore_ascii_case(&key))
                    .map(|k| (*k).to_string())
                    .unwrap_or(key);
                if dictionaries.contains(&canonical.as_str()) {
                    if let Value::Object(dictionary) = &mut item {
                        for nested in dictionary.values_mut() {
                            normalize_keys(nested, known, dictionaries);
                        }
                    }
                } else {
                    normalize_keys(&mut item, known, dictionaries);
                }
                map.insert(canonical, item);
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|v| normalize_keys(v, known, dictionaries)),
        _ => {}
    }
}

/// .NET `DateTimeOffset` 문자열 형식.
pub mod time {
    use super::*;

    fn fraction(dt: &DateTime<FixedOffset>) -> String {
        // .NET 틱(100ns) 정밀도. 나머지 나노초는 버린다.
        format!("{:07}", dt.timestamp_subsec_nanos() / 100)
    }

    /// JSON 직렬화 형식: 소수 초 끝의 0을 생략하고, 0이면 소수부를 쓰지 않는다.
    pub fn json(dt: &DateTime<FixedOffset>) -> String {
        let fraction = fraction(dt);
        let trimmed = fraction.trim_end_matches('0');
        let base = dt.format("%Y-%m-%dT%H:%M:%S").to_string();
        let offset = dt.format("%:z").to_string();
        if trimmed.is_empty() {
            format!("{base}{offset}")
        } else {
            format!("{base}.{trimmed}{offset}")
        }
    }

    /// 형식 문자열 `O`(라운드트립): 소수 7자리를 항상 쓴다.
    pub fn round_trip(dt: &DateTime<FixedOffset>) -> String {
        format!(
            "{}.{}{}",
            dt.format("%Y-%m-%dT%H:%M:%S"),
            fraction(dt),
            dt.format("%:z")
        )
    }

    /// `DateTimeOffset` 문자열 읽기. 오프셋이 없으면 로컬 시간대로 본다(.NET과 같다).
    pub fn parse(text: &str) -> Option<DateTime<FixedOffset>> {
        let text = text.trim();
        if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
            return Some(dt);
        }
        for format in [
            "%Y-%m-%dT%H:%M:%S%.f%:z",
            "%Y-%m-%d %H:%M:%S%.f%:z",
            "%Y-%m-%dT%H:%M%:z",
            "%Y-%m-%d %H:%M%:z",
        ] {
            if let Ok(dt) = DateTime::parse_from_str(text, format) {
                return Some(dt);
            }
        }
        for format in [
            "%Y-%m-%dT%H:%M:%S%.f",
            "%Y-%m-%d %H:%M:%S%.f",
            "%Y-%m-%dT%H:%M",
            "%Y-%m-%d %H:%M",
        ] {
            if let Ok(naive) = NaiveDateTime::parse_from_str(text, format) {
                return local(naive);
            }
        }
        NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .and_then(|d| local(d.and_hms_opt(0, 0, 0)?))
    }

    fn local(naive: NaiveDateTime) -> Option<DateTime<FixedOffset>> {
        Local
            .from_local_datetime(&naive)
            .earliest()
            .map(|dt| dt.fixed_offset())
    }

    /// `default(DateTimeOffset)` = `0001-01-01T00:00:00+00:00`.
    pub fn min_value() -> DateTime<FixedOffset> {
        let naive = NaiveDate::from_ymd_opt(1, 1, 1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .expect("0001-01-01");
        FixedOffset::east_opt(0)
            .expect("0 오프셋")
            .from_utc_datetime(&naive)
    }

    pub fn serialize<S: serde::Serializer>(
        dt: &DateTime<FixedOffset>,
        s: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&json(dt))
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<DateTime<FixedOffset>, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(d)?;
        parse(&text).ok_or_else(|| {
            serde::de::Error::custom(format!("DateTimeOffset 형식이 아닙니다: {text}"))
        })
    }

    pub mod option {
        use super::*;
        pub fn serialize<S: serde::Serializer>(
            dt: &Option<DateTime<FixedOffset>>,
            s: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            match dt {
                Some(dt) => s.serialize_str(&json(dt)),
                None => s.serialize_none(),
            }
        }
        pub fn deserialize<'de, D: serde::Deserializer<'de>>(
            d: D,
        ) -> std::result::Result<Option<DateTime<FixedOffset>>, D::Error> {
            let text = <Option<String> as serde::Deserialize>::deserialize(d)?;
            text.map(|t| {
                parse(&t).ok_or_else(|| {
                    serde::de::Error::custom(format!("DateTimeOffset 형식이 아닙니다: {t}"))
                })
            })
            .transpose()
        }
    }
}

/// `JsonSerializerDefaults.Web`의 `NumberHandling.AllowReadingFromString`. 정수 필드가 문자열이어도 읽는다.
pub mod web_int {
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<i32, D::Error> {
        use serde::de::Error;
        match <serde_json::Value as serde::Deserialize>::deserialize(d)? {
            serde_json::Value::Number(n) => n
                .as_i64()
                .and_then(|v| i32::try_from(v).ok())
                .ok_or_else(|| D::Error::custom("정수 범위를 벗어났습니다.")),
            serde_json::Value::String(s) => s.trim().parse::<i32>().map_err(D::Error::custom),
            other => Err(D::Error::custom(format!("정수가 아닙니다: {other}"))),
        }
    }
}
