//! 지식베이스 탭. 원본: `KnowledgePanel.xaml.cs`(자료·연결 설정), `AutoKnowledgePanel.xaml.cs`(대화 자동 정리).
//! 자동 정리의 1분 주기 실행은 `RelayHost` 주기 루프가 맡는다.

use std::io::Read;
use std::path::{Path, PathBuf};

use kakaorelay_ai::documents;
use kakaorelay_ai::obsidian::{self, MAX_FILE_BYTES};
use kakaorelay_ai::settings::{
    AutoKnowledgeSettings, KnowledgeRoomSubscription, KnowledgeSettings, room_key,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tauri::AppHandle;

use super::{App, CmdResult, emit, fail};
use crate::state::AppState;

/// 원본 `ConnectedVault`.
fn connected(vault: &str) -> CmdResult<PathBuf> {
    if vault.trim().is_empty() || !Path::new(vault).is_dir() {
        return Err("연결 설정에서 보관함을 선택하고 저장하세요.".into());
    }
    Ok(PathBuf::from(vault))
}

fn cancelled(error: kakaorelay_ai::Error) -> String {
    if error.is_cancelled() {
        "취소했습니다. 완료된 자료는 보관함에 남아 있습니다.".into()
    } else {
        error.to_string()
    }
}

async fn list(vault: &Path, token: &kakaorelay_ai::cancel::CancellationToken) -> CmdResult<Value> {
    let library = documents::list(vault, token).await.map_err(cancelled)?;
    let mut hint = format!("노트 {}개", library.notes.len());
    if library.skipped > 0 {
        hint.push_str(&format!(" · 제외 {}개", library.skipped));
    }
    if library.limited {
        hint.push_str(" · 한도 도달");
    }
    Ok(json!({
        "notes": library.notes.iter().map(|n| json!({ "path": n.path.to_string_lossy(), "source": n.source })).collect::<Vec<_>>(),
        "hint": hint,
    }))
}

/// 원본 `ReloadAsync`·`LoadNotes`.
#[tauri::command]
pub async fn notes_list(state: App<'_>, vault: String) -> CmdResult<Value> {
    let operation = state.ops.begin("knowledge").map_err(fail)?;
    list(&connected(&vault)?, &operation.token).await
}

/// 원본 `SelectedNote` 미리보기: 링크 경로·256KB 초과는 읽지 않고, 앞 2만 자만 보여 준다.
#[tauri::command(async)]
pub fn note_preview(path: String) -> String {
    let read = || -> std::io::Result<String> {
        let path = Path::new(&path);
        let mut dir = path.parent();
        while let Some(current) = dir {
            if obsidian::is_link(current) {
                return Err(std::io::Error::other("연결된 폴더는 읽을 수 없습니다."));
            }
            dir = current.parent();
        }
        if obsidian::is_link(path) {
            return Err(std::io::Error::other("연결된 파일은 읽을 수 없습니다."));
        }
        let mut file = std::fs::File::open(path)?;
        if file.metadata()?.len() > MAX_FILE_BYTES as u64 {
            return Err(std::io::Error::other("노트가 256KB를 넘습니다."));
        }
        let mut bytes = vec![];
        file.read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut chars = text.chars();
        let head: String = chars.by_ref().take(20000).collect();
        Ok(if chars.next().is_some() {
            format!("{head}\n\n… 미리보기 생략. 전체 내용은 옵시디언에서 확인하세요.")
        } else {
            head
        })
    };
    read().unwrap_or_else(|e| format!("노트를 읽을 수 없습니다: {e}"))
}

/// 원본 `Import`. 진행 상황은 `knowledge-panel-status` 이벤트로 보낸다.
#[tauri::command]
pub async fn notes_import(
    app: AppHandle,
    state: App<'_>,
    vault: String,
    paths: Vec<String>,
) -> CmdResult<Value> {
    if paths.len() > 50 {
        return Err("한 번에 50개 이하의 파일을 추가하세요.".into());
    }
    let vault = connected(&vault)?;
    let operation = state.ops.begin("knowledge").map_err(fail)?;
    let mut results = vec![];
    let mut done = 0;
    for path in &paths {
        if operation.token.is_cancelled() {
            return Err(cancelled(kakaorelay_ai::Error::Cancelled));
        }
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        emit(
            &app,
            "knowledge-panel-status",
            format!("자료 추가 중 · {done}/{} · {name}", paths.len()),
        );
        match documents::import(&vault, Path::new(path), &operation.token).await {
            Ok(added) => {
                done += 1;
                results.push(format!("완료 · {name} → 노트 {}개", added.len()));
            }
            Err(error) if error.is_cancelled() => return Err(cancelled(error)),
            Err(error) => results.push(format!("실패 · {name}: {error}")),
        }
    }
    let notes = list(&vault, &operation.token).await?;
    Ok(json!({
        "notes": notes,
        "preview": results.join("\n\n"),
        "status": format!("파일 {done}/{}개 추가 완료. 원본 파일은 그대로 유지됩니다.", paths.len()),
    }))
}

/// 원본 `Search`. AI 호출 없이 로컬에서 찾는다.
#[tauri::command]
pub async fn notes_search(state: App<'_>, vault: String, query: String) -> CmdResult<Value> {
    if query.trim().is_empty() {
        return Err("찾을 내용을 입력하세요.".into());
    }
    let settings = KnowledgeSettings {
        enabled: true,
        vault_path: connected(&vault)?.to_string_lossy().into_owned(),
    };
    let operation = state.ops.begin("knowledge").map_err(fail)?;
    let result = obsidian::search(&settings, &query, &operation.token)
        .await
        .map_err(cancelled)?;
    let preview = if result.excerpts.is_empty() {
        "관련 노트가 없습니다. 제목이나 다른 주제어로 검색해 보세요.".to_owned()
    } else {
        result
            .excerpts
            .iter()
            .map(|x| format!("[노트: {}] · 발췌 {}\n{}", x.source, x.part, x.text))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    Ok(json!({
        "preview": preview,
        "status": format!(
            "관련 발췌 {}개 · 읽은 노트 {}개 · 제외 {}개{} · AI 호출 없음",
            result.excerpts.len(), result.notes, result.skipped,
            if result.limited { " · 검색 한도 도달" } else { "" }
        ),
    }))
}

/// 원본 `Write_Click`의 저장 단계. 입력 검사(제목·내용·길이)는 화면 대화상자가 먼저 한다.
#[tauri::command]
pub async fn note_write(
    state: App<'_>,
    vault: String,
    title: String,
    body: String,
) -> CmdResult<Value> {
    let vault = connected(&vault)?;
    let operation = state.ops.begin("knowledge").map_err(fail)?;
    let settings = KnowledgeSettings {
        enabled: false,
        vault_path: vault.to_string_lossy().into_owned(),
    };
    let path = obsidian::save_note(&settings, &title, &body, None, &operation.token)
        .await
        .map_err(cancelled)?;
    let notes = list(&vault, &operation.token).await?;
    Ok(json!({ "notes": notes, "path": path.to_string_lossy() }))
}

/// 원본 `SaveConnection`. 보관함을 실제로 읽어 본 뒤에만 저장한다.
#[tauri::command]
pub async fn knowledge_save_connection(
    state: App<'_>,
    persona_id: String,
    all: bool,
    vault: String,
    enabled: bool,
) -> CmdResult<Value> {
    let operation = state.ops.begin("knowledge").map_err(fail)?;
    let candidate = KnowledgeSettings {
        vault_path: vault.trim().to_owned(),
        enabled,
    };
    candidate.validate().map_err(fail)?;
    let has_vault = !candidate.vault_path.trim().is_empty();
    if has_vault {
        documents::list(Path::new(&candidate.vault_path), &operation.token)
            .await
            .map_err(cancelled)?;
    }
    let mut settings = state.settings();
    for profile in settings
        .personas
        .iter_mut()
        .filter(|p| all || p.id == persona_id)
    {
        profile.knowledge = candidate.clone();
    }
    state.save_settings(settings).map_err(|_| {
        "연결을 저장하지 못했습니다. AI 챗봇 탭의 설정 오류를 확인하세요.".to_owned()
    })?;
    let notes = if has_vault {
        list(Path::new(&candidate.vault_path), &operation.token).await?
    } else {
        json!({ "notes": [], "hint": "노트 0개" })
    };
    Ok(json!({
        "settings": super::ai::settings_json(&state),
        "notes": notes,
        "status": if all { "모든 봇에 같은 보관함을 연결했습니다." } else { "이 봇의 연결 설정을 저장했습니다." },
    }))
}

/// 원본 `CreateVault_Click`.
#[tauri::command(async)]
pub fn create_vault(parent: String) -> CmdResult<String> {
    let path = Path::new(&parent).join(format!(
        "KakaoRelay-{}",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    std::fs::create_dir_all(&path).map_err(fail)?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn knowledge_cancel(state: App<'_>) {
    state.ops.cancel("knowledge");
}

fn destination(state: &AppState, profile: &str, id: &str) -> String {
    match state.settings().resolve_persona(profile, id) {
        Ok(persona) => format!(
            "{} · {}",
            persona.persona.name,
            if persona.knowledge.vault_path.trim().is_empty() {
                "보관함 연결 필요".to_owned()
            } else {
                Path::new(persona.knowledge.vault_path.trim_end_matches(['/', '\\']))
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }
        ),
        Err(error) => error.to_string(),
    }
}

fn choice(state: &AppState, profile: &str, id: &str, title: &str, selected: bool) -> Value {
    json!({
        "profile": profile, "id": id, "title": title, "selected": selected,
        "key": room_key(profile, id),
        "destination": destination(state, profile, id),
    })
}

/// 원본 `AutoKnowledgePanel`의 초기 목록(`Initialize`)과 `Refresh_Click`. `refresh`가 false면 저장된 방만 돌려준다.
#[tauri::command]
pub async fn auto_rooms(
    state: App<'_>,
    refresh: bool,
    previous: Vec<String>,
) -> CmdResult<Vec<Value>> {
    let saved = state.settings().auto_knowledge.rooms;
    let mut result = vec![];
    if refresh {
        let rooms = state.ai.chats().rooms().await.map_err(|e| e.detail)?;
        for room in rooms.iter().filter(|r| {
            r.readable
                || saved
                    .iter()
                    .any(|s| s.profile == r.profile && s.room_id == r.id)
        }) {
            let key = room_key(&room.profile, &room.id);
            let selected = previous.contains(&key)
                || saved
                    .iter()
                    .any(|s| s.profile == room.profile && s.room_id == room.id);
            result.push(choice(
                &state,
                &room.profile,
                &room.id,
                &room.title,
                selected,
            ));
        }
    }
    let listed: Vec<String> = result
        .iter()
        .map(|r| r["key"].as_str().unwrap_or("").to_owned())
        .collect();
    for missing in saved
        .iter()
        .filter(|s| !listed.contains(&room_key(&s.profile, &s.room_id)))
    {
        result.push(choice(
            &state,
            &missing.profile,
            &missing.room_id,
            &missing.title,
            true,
        ));
    }
    Ok(result)
}

#[derive(Deserialize)]
pub struct AutoRoom {
    profile: String,
    id: String,
    title: String,
}

/// 원본 `AutoKnowledgePanel.Apply_Click`. 새로 선택한 방은 현재 대화를 시작 기준으로 저장한다.
#[tauri::command(async)]
pub fn auto_apply(
    state: App<'_>,
    enabled: bool,
    auto_save: bool,
    interval_minutes: i32,
    rooms: Vec<AutoRoom>,
) -> CmdResult<String> {
    let mut settings = state.settings();
    let old = settings.auto_knowledge.clone();
    let mut next = AutoKnowledgeSettings {
        enabled,
        auto_save,
        interval_minutes,
        rooms: vec![],
    };
    for room in &rooms {
        if enabled {
            let vault = settings
                .resolve_persona(&room.profile, &room.id)
                .map_err(fail)?
                .knowledge
                .vault_path;
            if !Path::new(&vault).is_dir() {
                return Err(format!("{} 봇의 보관함을 먼저 연결하세요.", room.title));
            }
        }
        let existing = old
            .enabled
            .then(|| {
                old.rooms
                    .iter()
                    .find(|r| r.profile == room.profile && r.room_id == room.id)
                    .cloned()
            })
            .flatten();
        next.rooms.push(existing.unwrap_or_else(|| {
            KnowledgeRoomSubscription::new(
                &room.profile,
                &room.id,
                &room.title,
                uuid::Uuid::new_v4(),
            )
        }));
    }
    next.validate().map_err(fail)?;
    if enabled && settings.self_id.trim().is_empty() {
        return Err("AI 챗봇 탭에서 내 카카오 ID를 설정하세요.".into());
    }
    settings.auto_knowledge = next;
    state
        .save_settings(settings)
        .map_err(|_| "설정을 저장하지 못했습니다.".to_owned())?;
    Ok(if enabled {
        format!(
            "자동 정리 사용 · {}개 방 · {interval_minutes}분 주기. 새로 선택한 방은 현재 대화를 시작 기준으로 저장합니다.",
            rooms.len()
        )
    } else {
        "자동 정리를 껐습니다.".into()
    })
}

/// 원본 `Run_Click`(강제 실행). 진행 문구는 `knowledge-status` 이벤트로 온다.
#[tauri::command]
pub async fn auto_run(state: App<'_>, force: bool) -> CmdResult<()> {
    let knowledge = &state.ai.automatic_knowledge;
    if knowledge.busy() {
        return Ok(());
    }
    let operation = state.ops.begin("auto-knowledge").map_err(fail)?;
    let settings = state.ai.settings().load().map_err(fail)?;
    match knowledge.tick(&settings, force, &operation.token).await {
        Ok(()) => Ok(()),
        Err(error) if error.is_cancelled() => Err("자동 정리 작업을 중지했습니다.".into()),
        Err(error) => Err(error.to_string()),
    }
}

/// 원본 `Review_Click`: 보관함의 `검토필요` 폴더를 연다.
#[tauri::command]
pub fn auto_review_folder(vault: String) -> CmdResult<()> {
    if vault.trim().is_empty() || !Path::new(&vault).is_dir() {
        return Err("위에서 봇을 선택하고 보관함을 연결하세요.".into());
    }
    let path = Path::new(&vault).join("검토필요");
    std::fs::create_dir_all(&path).map_err(fail)?;
    crate::os::open(&path.to_string_lossy()).map_err(fail)
}

/// 원본 `SavedStatuses`(시작 시 처리 위치 보고).
#[tauri::command(async)]
pub fn auto_statuses(state: App<'_>) -> Value {
    json!({
        "statuses": state.ai.automatic_knowledge.saved_statuses(&state.settings()),
        "busy": state.ai.automatic_knowledge.busy() || state.ops.busy("auto-knowledge"),
    })
}
