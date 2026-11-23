//! 원본: `SummaryForwardSettings.cs`. 정리·전달 규칙 스키마와 검증.

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use crate::error::{Error, Result};
use crate::settings::room_key;
use crate::text::{is_blank, utf16_len};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryRoom {
    pub profile: String,
    pub id: String,
    pub title: String,
}

impl SummaryRoom {
    pub fn new(
        profile: impl Into<String>,
        id: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self {
            profile: profile.into(),
            id: id.into(),
            title: title.into(),
        }
    }
    pub fn key(&self) -> String {
        room_key(&self.profile, &self.id)
    }
}

/// 읽기 전용 계산 속성 `Key`도 원본처럼 직렬화한다.
impl Serialize for SummaryRoom {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("SummaryRoom", 4)?;
        st.serialize_field("profile", &self.profile)?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("title", &self.title)?;
        st.serialize_field("key", &self.key())?;
        st.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SummaryForwardRule {
    pub id: String,
    pub enabled: bool,
    #[serde(deserialize_with = "crate::json::web_int::deserialize")]
    pub interval_minutes: i32,
    pub sources: Vec<SummaryRoom>,
    pub destination: Option<SummaryRoom>,
}

impl Default for SummaryForwardRule {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            enabled: false,
            interval_minutes: 30,
            sources: vec![],
            destination: None,
        }
    }
}

/// 읽기 전용 계산 속성 `Label`도 원본처럼 직렬화한다.
impl Serialize for SummaryForwardRule {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("SummaryForwardRule", 6)?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("enabled", &self.enabled)?;
        st.serialize_field("intervalMinutes", &self.interval_minutes)?;
        st.serialize_field("sources", &self.sources)?;
        st.serialize_field("destination", &self.destination)?;
        st.serialize_field("label", &self.label())?;
        st.end()
    }
}

/// `Guid.TryParseExact(id, "N")`.
pub fn is_guid_n(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

impl SummaryForwardRule {
    pub fn label(&self) -> String {
        let sources: Vec<&str> = self.sources.iter().map(|r| r.title.as_str()).collect();
        format!(
            "{} → {} · {}분{}",
            sources.join(", "),
            self.destination
                .as_ref()
                .map(|d| d.title.as_str())
                .unwrap_or(""),
            self.interval_minutes,
            if self.enabled { "" } else { " · 꺼짐" }
        )
    }

    pub fn validate(&self) -> Result<()> {
        let fail = || Error::argument("정리할 방과 다른 수신방, 주기(1~1440분)를 지정하세요.");
        let Some(destination) = &self.destination else {
            return Err(fail());
        };
        let keys: Vec<String> = self.sources.iter().map(SummaryRoom::key).collect();
        let mut distinct = keys.clone();
        distinct.sort();
        distinct.dedup();
        if !is_guid_n(&self.id)
            || !(1..=1440).contains(&self.interval_minutes)
            || self.sources.is_empty()
            || self.sources.iter().any(|r| !valid(r))
            || !valid(destination)
            || distinct.len() != keys.len()
            || keys.contains(&destination.key())
        {
            return Err(fail());
        }
        Ok(())
    }
}

fn valid(r: &SummaryRoom) -> bool {
    !is_blank(&r.profile)
        && !is_blank(&r.id)
        && !is_blank(&r.title)
        && utf16_len(&r.profile) <= 200
        && utf16_len(&r.id) <= 200
        && utf16_len(&r.title) <= 500
}
