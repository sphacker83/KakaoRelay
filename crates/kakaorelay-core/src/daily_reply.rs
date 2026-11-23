//! 원본: `DailyReplyHistory.cs`. 방별 "오늘 첫 답장" 기록. 날짜와 첫 답장 식별자만 저장하고 대화 내용은 남기지 않는다.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use kakaorelay_ai::settings::room_key;
use kakaorelay_ai::{Error, LocalRoom, Result, json};
use serde::Deserialize;

use crate::fs::write_atomic;
use crate::support::{Clock, SystemClock};

static SYNC: Mutex<()> = Mutex::new(());

/// 옵션 없는 기본 직렬화(PascalCase). 골든 `dailyReplyEntry`.
pub fn entry_json(day: NaiveDate, first_reply: &str) -> String {
    json::default(
        &serde_json::json!({"Day": day.format("%Y-%m-%d").to_string(), "FirstReply": first_reply}),
    )
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "Day")]
    day: NaiveDate,
    #[serde(rename = "FirstReply")]
    first_reply: String,
}

pub struct DailyReplyHistory {
    root: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DailyReplyHistory {
    pub fn default_root() -> PathBuf {
        kakaorelay_ai::paths::app_data().join("daily-replies")
    }

    pub fn new(root: impl Into<PathBuf>, clock: Option<Arc<dyn Clock>>) -> Self {
        Self {
            root: root.into(),
            clock: clock.unwrap_or_else(|| Arc::new(SystemClock)),
        }
    }

    /// 현지 날짜.
    pub fn today(&self) -> NaiveDate {
        self.clock.now().date_naive()
    }

    fn path_for(&self, room: &LocalRoom) -> PathBuf {
        self.root.join(format!(
            "{}.json",
            json::hash(&room_key(&room.profile, &room.id))
        ))
    }

    fn read(&self, room: &LocalRoom) -> Result<Option<Entry>> {
        let path = self.path_for(room);
        if !path.exists() {
            return Ok(None);
        }
        let entry: Option<Entry> = serde_json::from_str(&kakaorelay_ai::text::read_text(&path)?)?;
        entry
            .map(Some)
            .ok_or_else(|| Error::invalid_data("오늘의 답장 기록을 읽을 수 없습니다."))
    }

    /// 같은 답장을 다시 보낼 때는 처음 첨부와 멱등 키를 유지한다.
    pub fn is_first_reply(&self, room: &LocalRoom, day: NaiveDate, reply_id: &str) -> Result<bool> {
        let _sync = SYNC.lock().unwrap_or_else(|e| e.into_inner());
        Ok(match self.read(room)? {
            None => true,
            Some(entry) => entry.day != day || entry.first_reply == reply_id,
        })
    }

    pub fn record_sent(&self, room: &LocalRoom, day: NaiveDate, reply_id: &str) -> Result<()> {
        let _sync = SYNC.lock().unwrap_or_else(|e| e.into_inner());
        if self.read(room)?.is_some_and(|e| e.day == day) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root)?;
        write_atomic(
            &self.path_for(room),
            entry_json(day, reply_id).as_bytes(),
            false,
        )
    }
}
