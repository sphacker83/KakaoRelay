//! 제어방 하위 탭. 원본: `CommandRoomPanel.xaml.cs`. 2초 주기 명령 처리는 `RelayHost`가 맡는다.

use kakaorelay_ai::relay_command::HELP;
use kakaorelay_core::command_room::{CommandRoomSettings, CommandRoomStore};
use kakaorelay_core::readiness::prepare;
use serde_json::{Value, json};

use super::{App, CmdResult, fail, room_json};

fn history(state: &crate::state::AppState) -> String {
    let Some(host) = state.host.get() else {
        return String::new();
    };
    let items = host.commands().history().unwrap_or_default();
    items
        .iter()
        .rev()
        .map(|a| {
            format!(
                "{} · {}\n{}",
                a.at.with_timezone(&chrono::Local).format("%m/%d %H:%M:%S"),
                a.action,
                a.result
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 저장된 제어방 설정·상태·이력·명령어 도움말.
#[tauri::command(async)]
pub fn control_load(state: App<'_>) -> CmdResult<Value> {
    let draft = CommandRoomStore::new(None).load().map_err(fail)?;
    let status = match state.host.get() {
        Some(host) if !host.commands().status().is_empty() => host.commands().status(),
        _ if draft.enabled => "제어방 재개 중".into(),
        _ => "제어방 꺼짐".into(),
    };
    Ok(json!({
        "draft": draft,
        "room": draft.room.as_ref().map(room_json),
        "status": status,
        "history": history(&state),
        "help": HELP,
    }))
}

/// 원본 `Refresh_Click`: 상대방이 있는 1:1 방만 고른다.
#[tauri::command]
pub async fn control_rooms(state: App<'_>) -> CmdResult<Vec<Value>> {
    let rooms = state.ai.chats().rooms().await.map_err(|e| e.detail)?;
    Ok(rooms
        .iter()
        .filter(|r| r.direct_member_id.is_some())
        .map(room_json)
        .collect())
}

/// 원본 `Apply_Click`. 켤 때는 방을 다시 확인하고 그 방의 자동답장을 멈춘 뒤 새 세대로 저장한다.
#[tauri::command]
pub async fn control_apply(
    state: App<'_>,
    mut draft: CommandRoomSettings,
    room: Option<kakaorelay_ai::LocalRoom>,
) -> CmdResult<String> {
    let host = state.host().map_err(fail)?.clone();
    let mut selected = room;
    if draft.enabled {
        let room = selected
            .clone()
            .ok_or_else(|| "1:1 제어방을 선택하세요.".to_owned())?;
        let platform = state.platform.clone();
        let scan: kakaorelay_core::api_send::Scan =
            std::sync::Arc::new(move || platform.rooms().scan());
        let token = kakaorelay_ai::cancel::CancellationToken::new();
        let (_, ready) = prepare(state.ai.chats().as_ref(), &[room], &scan, &token)
            .await
            .map_err(fail)?;
        let room = ready
            .into_iter()
            .next()
            .ok_or_else(|| "1:1 제어방을 선택하세요.".to_owned())?;
        let self_id = state.ai.settings().load().map_err(fail)?.self_id;
        if room
            .direct_member_id
            .as_deref()
            .is_none_or(|m| m == self_id)
        {
            return Err("상대방이 있는 일반 1:1 방만 허용합니다.".into());
        }
        host.runtime().stop(Some(&room)).await;
        selected = Some(room);
    }
    draft.operator_id = selected
        .as_ref()
        .and_then(|r| r.direct_member_id.clone())
        .unwrap_or_default();
    draft.room = selected;
    draft.generation = uuid::Uuid::new_v4();
    CommandRoomStore::new(None).save(&draft).map_err(fail)?;
    Ok(if draft.enabled {
        "제어방 적용 · 새 명령부터 처리".into()
    } else {
        "제어방 꺼짐".into()
    })
}
