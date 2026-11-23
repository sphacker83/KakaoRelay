//! .NET 기준 출력(`kakaorelay-ai/tests/fixtures/dotnet-golden.json`, 생성기 `tools/golden/Program.cs`)과
//! core 공개 함수의 결과를 해시로 비교한다. ai 골든은 직렬화기만, 여기서는 실제 core 경로를 고정한다.

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone};
use kakaorelay_ai::json;
use kakaorelay_ai::persona_images::PersonaAttachment;
use kakaorelay_ai::summary::{SummaryEvidence, SummaryUpdate};
use kakaorelay_ai::summary_forward_settings::{SummaryForwardRule, SummaryRoom};
use kakaorelay_core::api_send::ApiSendCommand;
use kakaorelay_core::daily_reply::entry_json;
use kakaorelay_core::summary_forwarder::{fact_key, identity};
use kakaorelay_core::test_send::TestSendRequest;
use serde::Serialize;

const TRICKY: &str = "한글 \"따옴표\" <tag> & ' + \\ 줄\n바꿈\t탭 😀 é \u{1} \u{2028}";

fn expected(name: &str) -> String {
    let golden: serde_json::Value = serde_json::from_str(include_str!(
        "../../kakaorelay-ai/tests/fixtures/dotnet-golden.json"
    ))
    .unwrap();
    golden["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("골든 항목 없음: {name}"))["hash"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn test_send_fingerprint() {
    let expires = chrono::Utc::now().fixed_offset();
    let mut request = TestSendRequest::new("id", "나와의 채팅", TRICKY, 1234, "0x1A2B", expires);
    request.verified_empty_placeholder = true;
    assert_eq!(request.fingerprint(), expected("testSendFingerprint"));
}

#[test]
fn api_fingerprint_text_and_attachment() {
    assert_eq!(
        ApiSendCommand::new("id", "방", TRICKY).fingerprint(),
        expected("apiFingerprint.text")
    );
    let mut command = ApiSendCommand::new("id", "방", "");
    command.attachment = Some(PersonaAttachment {
        persona_id: "default".into(),
        provider: "astra".into(),
        image_id: "0123456789abcdef0123456789abcdef".into(),
    });
    assert_eq!(command.fingerprint(), expected("apiFingerprint.attachment"));
}

/// 기준 경로가 유닉스 절대 경로라 유닉스에서만 같은 전체 경로가 나온다.
#[cfg(unix)]
#[test]
fn api_fingerprint_image() {
    let mut command = ApiSendCommand::new("id", "방", "");
    command.image_path = Some("/Users/x/그림 1.png".into());
    assert_eq!(command.fingerprint(), expected("apiFingerprint.image"));
}

#[test]
fn summary_identity_and_fact_key() {
    let rule = SummaryForwardRule {
        id: "0123456789abcdef0123456789abcdef".into(),
        sources: vec![
            SummaryRoom::new("P", "10", "열"),
            SummaryRoom::new("P", "2", "둘"),
        ],
        destination: Some(SummaryRoom::new("P", "3", "셋")),
        ..Default::default()
    };
    assert_eq!(identity(&rule).unwrap(), expected("summaryIdentity"));
    let evidence = |source: &str| SummaryEvidence {
        source: source.into(),
        message_id: "1".into(),
        quote: "q".into(),
    };
    let item = SummaryUpdate {
        topic: " 출시 일정 ".into(),
        text: "t".into(),
        evidence: Some(vec![evidence("방B"), evidence("방A"), evidence("방B")]),
    };
    assert_eq!(fact_key(&item), expected("summaryFactKey"));
}

#[test]
fn daily_reply_entry() {
    let day = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
    assert_eq!(
        json::hash(&entry_json(day, "bot-ABC")),
        expected("dailyReplyEntry")
    );
}

/// core 상태 파일이 쓰는 시간 형식(`json::time` 속성 + 들여쓰기 + 한글 비이스케이프).
#[test]
fn report_options_time() {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Report {
        #[serde(with = "kakaorelay_ai::json::time")]
        started_at: DateTime<FixedOffset>,
        #[serde(with = "kakaorelay_ai::json::time::option")]
        finished_at: Option<DateTime<FixedOffset>>,
        input_cleared: Option<bool>,
        detail: &'static str,
    }
    let started = FixedOffset::east_opt(9 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 9, 28, 13, 23, 0)
        .unwrap()
        + chrono::Duration::milliseconds(120);
    let report = Report {
        started_at: started,
        finished_at: None,
        input_cleared: Some(true),
        detail: TRICKY,
    };
    assert_eq!(
        json::hash(&json::indented(&report)),
        expected("reportOptions.time")
    );
}
