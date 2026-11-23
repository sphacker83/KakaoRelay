//! 원본: `AutoReplyReadiness.cs`. 열린 방 판정(로컬 방 ∩ 발송 가능한 창)과 시작 전 검증.

use std::collections::HashMap;

use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::{ChatReader, Error, LocalRoom, Result};

use crate::api_send::Scan;
use crate::platform::ConversationTarget;
use crate::support::{chat_read, chat_rooms, scan};

/// 로컬 목록에서 제목이 하나뿐이고, 같은 제목의 창이 정확히 하나이며 선택 가능한 방만 고른다.
pub fn open_rooms(rooms: &[LocalRoom], windows: &[ConversationTarget]) -> Vec<LocalRoom> {
    let mut room_titles: HashMap<&str, usize> = HashMap::new();
    for room in rooms {
        *room_titles.entry(room.title.as_str()).or_default() += 1;
    }
    let mut window_titles: HashMap<&str, Vec<&ConversationTarget>> = HashMap::new();
    for window in windows {
        window_titles
            .entry(window.title.as_str())
            .or_default()
            .push(window);
    }
    rooms
        .iter()
        .filter(|r| {
            room_titles.get(r.title.as_str()) == Some(&1)
                && window_titles
                    .get(r.title.as_str())
                    .is_some_and(|w| w.len() == 1 && w[0].selectable())
        })
        .cloned()
        .collect()
}

/// 원본 `PrepareAsync`. 반환은 (갱신된 전체 로컬 방, 시작할 방)이다.
pub async fn prepare(
    reader: &dyn ChatReader,
    selected: &[LocalRoom],
    windows: &Scan,
    cancel: &CancellationToken,
) -> Result<(Vec<LocalRoom>, Vec<LocalRoom>)> {
    if selected.is_empty() {
        return Err(Error::invalid_operation("자동 답장할 방을 선택하세요."));
    }
    let mut rooms = chat_rooms(reader, cancel).await?;
    let mut ready = vec![];
    for old in selected {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut room = rooms
            .iter()
            .find(|r| r.profile == old.profile && r.id == old.id)
            .cloned()
            .ok_or_else(|| {
                Error::invalid_operation(format!(
                    "{} · 현재 로컬 목록에서 이 방을 찾지 못했습니다. 카카오톡에서 방을 열어주세요.",
                    old.title
                ))
            })?;
        // 앞서 받은 방 목록의 읽기 가능 여부는 그때의 상태일 뿐 시작 금지 사유가 아니다.
        match chat_read(reader, &room.profile, &room.id, 1, cancel).await {
            Ok(context) => room = context.room,
            Err(Error::Api(failure)) => {
                return Err(Error::invalid_operation(format!(
                    "{} · 대화 읽기 확인 실패: {}",
                    room.title, failure.detail
                )));
            }
            Err(error) => return Err(error),
        }
        if !room.readable {
            return Err(Error::invalid_operation(format!(
                "{} · 대화 읽기를 확인하지 못했습니다. 방을 연 뒤 다시 시작하세요.",
                room.title
            )));
        }
        if let Some(slot) = rooms
            .iter_mut()
            .find(|r| r.profile == room.profile && r.id == room.id)
        {
            *slot = room.clone();
        }
        ready.push(room);
    }
    let windows = scan(windows, cancel).await?;
    for room in &ready {
        if rooms.iter().filter(|r| r.title == room.title).count() != 1 {
            return Err(Error::invalid_operation(format!(
                "{} · 로컬 목록에 같은 제목의 방이 여러 개 있어 발송 대상을 구분할 수 없습니다. 방 이름을 다르게 설정하세요.",
                room.title
            )));
        }
        let matches: Vec<_> = windows.iter().filter(|w| w.title == room.title).collect();
        if matches.is_empty() {
            return Err(Error::invalid_operation(format!(
                "{} · 대화는 읽을 수 있지만 발송할 열린 창이 없습니다. 카카오톡에서 방을 별도 창으로 열고 최소화를 해제하세요.",
                room.title
            )));
        }
        if matches.len() != 1 || !matches[0].selectable() {
            return Err(Error::invalid_operation(format!(
                "{} · 같은 제목의 열린 창이 여러 개여서 발송할 창을 구분할 수 없습니다.",
                room.title
            )));
        }
    }
    Ok((rooms, ready))
}
