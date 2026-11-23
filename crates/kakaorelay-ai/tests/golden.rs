//! .NET 기준 출력(`tests/fixtures/dotnet-golden.json`, 생성기 `tools/golden/Program.cs`)과 바이트 비교.

use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use kakaorelay_ai::LocalMessage;
use kakaorelay_ai::conversation::AiConversation;
use kakaorelay_ai::json::{self, Escape};
use kakaorelay_ai::settings::{
    AiProviderSettings, AiSettings, AiSettingsStore, BotPersona, room_key,
};
use serde_json::{Value, json};

const TRICKY: &str = "한글 \"따옴표\" <tag> & ' + \\ 줄\n바꿈\t탭 😀 é \u{1} \u{2028}";

fn golden() -> Value {
    serde_json::from_str(include_str!("fixtures/dotnet-golden.json")).unwrap()
}

fn kst(h: u32, m: u32, s: u32) -> DateTime<FixedOffset> {
    FixedOffset::east_opt(9 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 9, 28, h, m, s)
        .unwrap()
}

fn times() -> [DateTime<FixedOffset>; 3] {
    let utc = FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
        .unwrap();
    [
        kst(13, 23, 0),
        kst(13, 23, 0) + Duration::milliseconds(120),
        utc + Duration::nanoseconds(1_234_568 * 100),
    ]
}

fn native(newline_text: &str) -> String {
    if cfg!(windows) {
        newline_text.replace('\n', "\r\n")
    } else {
        newline_text.to_string()
    }
}

fn actual_cases() -> Vec<(String, String)> {
    let mut cases = vec![
        ("roomKey.ascii".into(), room_key("ABCDEF0123", "18273645")),
        ("roomKey.tricky".into(), room_key(TRICKY, "1")),
        (
            "promptJson.tricky".into(),
            json::prompt(&json!({"Text": TRICKY, "List": ["가", "b"]})),
        ),
        (
            "default.tricky".into(),
            json::default(&json!({"Text": TRICKY})),
        ),
    ];
    for (i, time) in times().into_iter().enumerate() {
        let mut message = LocalMessage::new("9001", "77", "홍길동", time, 1, TRICKY, false);
        message.attachment_metadata = Some("{\"x\":1}".into());
        cases.push((
            format!("messageFingerprint.{i}"),
            json::default_pascal(&message),
        ));
    }
    let provider = AiProviderSettings {
        model: "gpt-6-luna".into(),
        effort: "low".into(),
        ..AiProviderSettings::new("codex")
    };
    cases.push((
        "conversationFolder.default".into(),
        AiConversation::identity(&provider, &BotPersona::default(), "ABCDEF0123", "18273645"),
    ));
    let persona2 = BotPersona {
        name: "봇".into(),
        role: TRICKY.into(),
        tone: "반말".into(),
        style: "짧게".into(),
        instructions: String::new(),
    };
    let provider2 = AiProviderSettings {
        enabled: false,
        executable: "/opt/homebrew/bin/claude".into(),
        model: "claude-opus-5-5[1m]".into(),
        ..AiProviderSettings::new("claude")
    };
    cases.push((
        "conversationFolder.custom".into(),
        AiConversation::identity(&provider2, &persona2, "ABCDEF0123", "18273645"),
    ));
    // 아래 Core 계열은 같은 직렬화기를 Phase 2가 그대로 쓰므로 여기서 함께 고정한다.
    cases.push((
        "testSendFingerprint".into(),
        json::default(&json!({"Recipient": "나와의 채팅", "Message": TRICKY, "ExpectedProcessId": 1234, "ExpectedWindowHandle": "0x1A2B", "VerifiedEmptyPlaceholder": true})),
    ));
    cases.push((
        "apiFingerprint.text".into(),
        json::default(&json!({"Recipient": "방", "Message": TRICKY})),
    ));
    cases.push((
        "apiFingerprint.image".into(),
        json::default(&json!({"Recipient": "방", "ImagePath": "/Users/x/그림 1.png"})),
    ));
    cases.push((
        "apiFingerprint.attachment".into(),
        json::default(&json!({"Recipient": "방", "Message": "", "Attachment": {"PersonaId": "default", "Provider": "astra", "ImageId": "0123456789abcdef0123456789abcdef"}})),
    ));
    let mut sources = vec![room_key("P", "2"), room_key("P", "10")];
    sources.sort();
    cases.push((
        "summaryIdentity".into(),
        json::prompt(&json!({"Id": "0123456789abcdef0123456789abcdef", "Sources": sources, "Destination": room_key("P", "3")})),
    ));
    cases.push((
        "summaryFactKey".into(),
        json::prompt(&json!({"Topic": kakaorelay_ai::text::nfc("출시 일정").trim(), "Sources": ["방A", "방B"]})),
    ));
    cases.push((
        "autoKnowledgeCheckpoint".into(),
        json::prompt(&json!({"Profile": "P", "RoomId": "2", "Generation": uuid::Uuid::parse_str("8f0d7c1e-6b1a-4c55-9b4e-0d7d0c3f2a11").unwrap(), "vault": "/Users/x/보관함", "SelfId": "12345"})),
    ));
    cases.push(("dailyReplyEntry".into(), json::default(&json!({"Day": chrono::NaiveDate::from_ymd_opt(2026, 11, 23).unwrap(), "FirstReply": "bot-ABC"}))));
    let mut state = AiConversation {
        session_id: Some("s-1".into()),
        instructions_date: chrono::NaiveDate::from_ymd_opt(2026, 11, 23),
        instructions_revision: Some("R".into()),
        ..Default::default()
    };
    state.seen.insert("H1".into());
    cases.push(("conversationState".into(), json::default(&state)));
    let report = json!({"startedAt": json::time::json(&times()[1]), "finishedAt": null, "inputCleared": true, "detail": TRICKY});
    cases.push((
        "reportOptions.time".into(),
        json::to_string(&report, Escape::Prompt, true),
    ));
    cases
}

#[test]
fn hash_inputs_match_dotnet_bytes() {
    let golden = golden();
    let expected = golden["cases"].as_array().unwrap();
    let actual = actual_cases();
    assert_eq!(actual.len(), expected.len());
    for ((name, input), case) in actual.iter().zip(expected) {
        assert_eq!(name, case["name"].as_str().unwrap());
        assert_eq!(
            input,
            &native(case["input"].as_str().unwrap()),
            "{name} 직렬화 입력"
        );
        assert_eq!(
            json::hash(input),
            case["hash"].as_str().unwrap(),
            "{name} 해시"
        );
    }
}

#[test]
fn settings_files_match_dotnet_bytes() {
    let golden = golden();
    let dir = kakaorelay_testkit::TempDir::new("golden-settings");
    let path = dir.path().join("ai-settings.json");
    AiSettingsStore::new(&path)
        .save(&mut AiSettings::default())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        native(golden["settingsDefault"].as_str().unwrap())
    );

    std::fs::write(&path, "{\"persona\":{\"name\":\"예전봇\",\"role\":\"r\",\"tone\":\"t\",\"style\":\"s\",\"instructions\":\"i\"},\"provider\":\"grok\",\"pollSeconds\":7}").unwrap();
    let mut legacy = AiSettingsStore::new(&path).load().unwrap();
    AiSettingsStore::new(&path).save(&mut legacy).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        native(golden["settingsLegacyResaved"].as_str().unwrap())
    );
}

#[test]
fn legacy_persona_order_follows_document_order() {
    // `personas` 뒤에 오는 `persona`는 첫 슬롯을 덮어쓴다(.NET 속성 적용 순서).
    let text = "{\"personas\":[{\"id\":\"a\",\"label\":\"A\"},{\"id\":\"b\",\"label\":\"B\"},{\"id\":\"c\",\"label\":\"C\"}],\"Persona\":{\"name\":\"뒤\"},\"POLLSECONDS\":\"9\"}";
    let settings: AiSettings = serde_json::from_str(text).unwrap();
    assert_eq!(settings.personas[0].persona.name, "뒤");
    assert_eq!(settings.poll_seconds, 9);
}

/// .NET 10.0.2에서 실제로 직렬화한 결과(`PromptJson` 인코더, 기본 인코더).
#[test]
fn encoder_edge_cases_match_dotnet() {
    let cases: &[(u32, &str, &str)] = &[
        (0x60, "\"\\u0060\"", "\"\\u0060\""),
        (0x7F, "\"\\u007F\"", "\"\\u007F\""),
        (0x80, "\"\\u0080\"", "\"\\u0080\""),
        (0xA0, "\"\\u00A0\"", "\"\\u00A0\""),
        (0xA9, "\"©\"", "\"\\u00A9\""),
        (0xAD, "\"\u{AD}\"", "\"\\u00AD\""),
        (0x378, "\"\\u0378\"", "\"\\u0378\""),
        (0x200B, "\"\u{200B}\"", "\"\\u200B\""),
        (0x2029, "\"\\u2029\"", "\"\\u2029\""),
        (0x3000, "\"\\u3000\"", "\"\\u3000\""),
        (0xE000, "\"\\uE000\"", "\"\\uE000\""),
        (0xFEFF, "\"\\uFEFF\"", "\"\\uFEFF\""),
        (0xFFFD, "\"�\"", "\"\\uFFFD\""),
        (0xFFFF, "\"\\uFFFF\"", "\"\\uFFFF\""),
        (0x08, "\"\\b\"", "\"\\b\""),
        (0x0C, "\"\\f\"", "\"\\f\""),
        (0x0D, "\"\\r\"", "\"\\r\""),
        (0x2F, "\"/\"", "\"/\""),
        (0x3164, "\"ㅤ\"", "\"\\u3164\""),
        (0xD7A3, "\"힣\"", "\"\\uD7A3\""),
        (0x1F600, "\"\\uD83D\\uDE00\"", "\"\\uD83D\\uDE00\""),
        (0x1D400, "\"\\uD835\\uDC00\"", "\"\\uD835\\uDC00\""),
        (0xFDD0, "\"\\uFDD0\"", "\"\\uFDD0\""),
    ];
    for &(code, prompt, default) in cases {
        let text = char::from_u32(code).unwrap().to_string();
        assert_eq!(json::prompt(&text), prompt, "Prompt U+{code:04X}");
        assert_eq!(json::default(&text), default, "Default U+{code:04X}");
    }
    let t = FixedOffset::east_opt(9 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
        .unwrap()
        + Duration::milliseconds(120);
    assert_eq!(
        json::time::round_trip(&t),
        "2026-01-02T03:04:05.1200000+09:00"
    );
    assert_eq!(
        json::time::json(&json::time::min_value()),
        "0001-01-01T00:00:00+00:00"
    );
}
