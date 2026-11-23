//! 정리감시 하위 탭. 원본: `SummaryForwardPanel.xaml.cs`. 15초 주기 실행은 `RelayHost`가 맡는다.

use kakaorelay_ai::summary_forward_settings::{SummaryForwardRule, SummaryRoom};
use kakaorelay_core::readiness::open_rooms;
use serde_json::{Value, json};

use super::{App, CmdResult, fail};

fn summary_room(profile: &str, id: &str, title: &str) -> SummaryRoom {
    SummaryRoom::new(profile, id, title)
}

/// 원본 `RefreshRooms`. 감시 원본은 읽을 수 있는 방과 이미 고른 방, 수신방은 열린 방이다.
#[tauri::command]
pub async fn forward_rooms(
    state: App<'_>,
    selected: Vec<SummaryRoom>,
    retained: Vec<SummaryRoom>,
    destination: Option<SummaryRoom>,
) -> CmdResult<Value> {
    let rooms = state.ai.chats().rooms().await.map_err(|e| e.detail)?;
    let platform = state.platform.clone();
    let windows = tokio::task::spawn_blocking(move || platform.rooms().scan())
        .await
        .map_err(fail)?
        .map_err(fail)?;
    let chosen =
        |profile: &str, id: &str| selected.iter().any(|s| s.profile == profile && s.id == id);
    let mut sources: Vec<Value> = rooms
        .iter()
        .filter(|r| r.readable || chosen(&r.profile, &r.id))
        .map(|room| {
            let source = summary_room(&room.profile, &room.id, &room.title);
            json!({
                "room": source,
                "key": source.key(),
                "selected": chosen(&room.profile, &room.id),
                "availability": if room.readable { "" } else { "읽기 대기 · 카카오톡에서 방을 한 번 열어주세요" },
            })
        })
        .collect();
    let listed: Vec<String> = sources
        .iter()
        .map(|c| c["key"].as_str().unwrap_or("").to_owned())
        .collect();
    for source in retained.iter().filter(|r| !listed.contains(&r.key())) {
        sources.push(json!({
            "room": source, "key": source.key(), "selected": true,
            "availability": "목록에서 찾지 못함 · 감시 설정 유지",
        }));
    }
    let mut destinations: Vec<SummaryRoom> = open_rooms(&rooms, &windows)
        .iter()
        .map(|r| summary_room(&r.profile, &r.id, &r.title))
        .collect();
    if let Some(dest) = destination
        && !destinations.iter().any(|d| d.key() == dest.key())
    {
        destinations.push(dest);
    }
    Ok(
        json!({ "sources": sources, "destinations": destinations.iter().map(|d| json!({ "room": d, "key": d.key() })).collect::<Vec<_>>() }),
    )
}

fn rules_json(rules: &[SummaryForwardRule]) -> Vec<Value> {
    rules
        .iter()
        .map(|r| json!({ "rule": r, "label": r.label() }))
        .collect()
}

/// 원본 `Apply_Click`. 주기만 바꾸면 진행 위치를 유지하고, 새 경로·다시 켜기는 새 기준에서 시작한다.
#[tauri::command(async)]
pub fn forward_apply(
    state: App<'_>,
    old_id: Option<String>,
    enabled: bool,
    interval: String,
    sources: Vec<SummaryRoom>,
    destination: Option<SummaryRoom>,
) -> CmdResult<Value> {
    let interval: i32 = interval
        .trim()
        .parse()
        .map_err(|_| "주기는 1~1440분의 숫자로 입력하세요.".to_owned())?;
    let mut settings = state.settings();
    let mut next = SummaryForwardRule {
        enabled,
        interval_minutes: interval,
        sources,
        destination,
        ..SummaryForwardRule::default()
    };
    next.validate().map_err(fail)?;
    let old = old_id
        .as_ref()
        .and_then(|id| settings.summary_forwards.iter().find(|r| &r.id == id))
        .cloned();
    if let Some(old) = &old {
        let keys = |rule: &SummaryForwardRule| {
            let mut keys: Vec<String> = rule.sources.iter().map(|s| s.key()).collect();
            keys.sort();
            keys
        };
        if (!next.enabled || old.enabled)
            && old.destination.as_ref().map(|d| d.key())
                == next.destination.as_ref().map(|d| d.key())
            && keys(old) == keys(&next)
        {
            next.id = old.id.clone();
        }
    }
    let old_rule_id = old.map(|r| r.id);
    settings
        .summary_forwards
        .retain(|r| Some(&r.id) != old_rule_id.as_ref());
    settings.summary_forwards.push(next.clone());
    state
        .save_settings(settings)
        .map_err(|_| "저장 실패".to_owned())?;
    Ok(json!({
        "rules": rules_json(&state.settings().summary_forwards),
        "selected": next.id,
        "status": if next.enabled { "사용 · 추가사항만 지정 주기로 전달합니다. 수신방을 열어두세요." } else { "전달 꺼짐" },
    }))
}

#[tauri::command(async)]
pub fn forward_delete(state: App<'_>, id: String) -> CmdResult<Value> {
    let mut settings = state.settings();
    settings.summary_forwards.retain(|r| r.id != id);
    state.save_settings(settings).map_err(fail)?;
    Ok(json!({ "rules": rules_json(&state.settings().summary_forwards) }))
}
