//! 자동인사(사용자 요청 2026-11-23, 원본에 없음) 검사.

use std::path::Path;

use chrono::{FixedOffset, NaiveDate, TimeZone};
use kakaorelay_ai::{LocalMessage, LocalRoom};
use kakaorelay_core::greetings::{
    GreetingPeriod, GreetingStore, birth_year, first_speakers, greeting_period, greeting_text,
    request_id, short_name,
};

use crate::support::Check;

fn at(id: &str, author_id: &str, nickname: &str, day: u32, hour: u32, kind: i32) -> LocalMessage {
    let kst = FixedOffset::east_opt(9 * 3600).unwrap();
    let time = kst.with_ymd_and_hms(2026, 11, day, hour, 0, 0).unwrap();
    LocalMessage::new(id, author_id, nickname, time, kind, "글", false)
}

pub fn run(root: &Path, check: &Check) {
    check.check(
        short_name("하늘83검단") == "하늘",
        "Greeting uses the first two Hangul syllables of the nickname",
    );
    check.check(
        short_name("🔥바다/85/김포") == "바다",
        "Greeting skips leading emoji and symbols",
    );
    check.check(
        birth_year("하늘83검단") == Some(83) && birth_year("3040 모임").is_none(),
        "Birth year is a standalone two-digit number",
    );
    check.check(
        greeting_text("하늘83검단") == "🤖 하늘 안녕!",
        "Born in 83 or later gets a casual hello",
    );
    check.check(
        greeting_text("바다82") == "🤖 바다 안녕하세요",
        "Born in 82 or earlier gets a polite hello",
    );
    check.check(
        greeting_text("바다") == "🤖 바다 안녕하세요",
        "Unknown birth year gets a polite hello",
    );
    check.check(
        greeting_text("민지05") == "🤖 민지 안녕!",
        "Two-digit years below 30 count as the 2000s",
    );

    let kst = FixedOffset::east_opt(9 * 3600).unwrap();
    let day = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
    let period =
        |d: u32, h: u32| greeting_period(kst.with_ymd_and_hms(2026, 11, d, h, 0, 0).unwrap());
    check.check(
        period(23, 0)
            == GreetingPeriod {
                day,
                afternoon: false,
            }
            && greeting_period(kst.with_ymd_and_hms(2026, 11, 23, 11, 59, 0).unwrap())
                == GreetingPeriod {
                    day,
                    afternoon: false,
                }
            && period(23, 12)
                == GreetingPeriod {
                    day,
                    afternoon: true,
                },
        "Greeting periods split each KST day at 00:00 and 12:00",
    );
    let messages = [
        at("1", "a", "오전사람", 23, 9, 1),
        at("2", "b", "먼저말함", 23, 12, 1),
        at("3", "me", "나", 23, 13, 1),
        at("4", "b", "먼저말함", 23, 14, 1),
        at("5", "a", "오전사람", 23, 15, 1),
        at("6", "c", "시스템", 23, 15, 0),
        at("7", "a", "오전사람", 23, 16, 1),
        at("8", "d", "새사람", 23, 17, 1),
    ];
    let ids: Vec<String> = first_speakers(&messages, "3", "me", period(23, 13))
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    check.check(
        ids == ["5", "8"],
        "Greets each person once per period; morning messages do not block an afternoon greeting; skips people who already spoke this period before the bot started, myself and system messages",
    );

    let room = LocalRoom::new("p", "1", "방", true);
    check.check(
        request_id(&room, "a", period(23, 9)) == request_id(&room, "a", period(23, 10))
            && request_id(&room, "a", period(23, 9)) != request_id(&room, "a", period(23, 13)),
        "Greeting request id is stable per room, person and period so the ledger prevents duplicates",
    );

    let store = GreetingStore::new(root.join("auto-greetings.json"));
    check.check(
        !store.enabled("k").unwrap(),
        "Greetings are off when no settings file exists",
    );
    store.set("k", true).unwrap();
    store.set("k", true).unwrap();
    check.check(
        store.enabled("k").unwrap() && store.load().unwrap().rooms.len() == 1,
        "Turning a room on twice keeps one entry",
    );
    store.set("k", false).unwrap();
    check.check(
        !store.enabled("k").unwrap(),
        "Greetings can be turned off per room",
    );
}
