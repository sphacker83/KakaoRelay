//! 카카오톡 PC 로컬 DB 읽기. 원본: `LocalChatReader.cs`의 `LocalChatReader`.
//! 메모리 키 후보를 페이지 HMAC으로 파일별로 대조하고, 읽을 때마다 DB+WAL을 복호화해 메모리에서 조회한다.

use crate::catalog::culture_order;
use crate::cipher::{self, PAGE_SIZE};
use crate::memory_keys::{self, KeyCandidate};
use crate::sqlite::MemoryChatDb;
use async_trait::async_trait;
use chrono::{DateTime, Local};
use kakaorelay_ai::{ApiFailure, ChatContext, ChatReader, LocalMessage, LocalRoom};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

const MAX_FILE: u64 = 512 * 1024 * 1024;
const READ_ATTEMPTS: usize = 3;

/// 원본에서 `ApiFailure`가 아닌 예외는 API가 500 `unknown_result`로 응답했다. 상세 문구는 원래 예외 메시지를 쓴다.
fn internal(detail: impl ToString) -> ApiFailure {
    ApiFailure::new(500, "unknown_result", detail.to_string())
}

fn is_profile(name: &str) -> bool {
    name.len() == 40
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Default)]
struct State {
    keys: HashMap<PathBuf, KeyCandidate>,
    rooms: Vec<LocalRoom>,
}

pub struct LocalChatReader {
    root: PathBuf,
    /// 원본 `SemaphoreSlim(1)`. 조회는 한 번에 하나씩 한다.
    state: Arc<Mutex<State>>,
}

impl Default for LocalChatReader {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalChatReader {
    pub fn new() -> Self {
        let root = dirs::data_local_dir()
            .unwrap_or_default()
            .join("Kakao")
            .join("KakaoTalk")
            .join("users");
        Self {
            root,
            state: Arc::default(),
        }
    }

    async fn locked<T: Send + 'static>(
        &self,
        action: impl FnOnce(&Path, &mut State) -> Result<T, ApiFailure> + Send + 'static,
    ) -> Result<T, ApiFailure> {
        let (root, state) = (self.root.clone(), self.state.clone());
        tokio::task::spawn_blocking(move || {
            let mut guard = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            action(&root, &mut guard)
        })
        .await
        .map_err(internal)?
    }
}

fn profiles(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_profile)
        })
        .collect()
}

/// 다른 프로세스가 쓰는 중에도 읽는다. Rust 표준 File은 Windows에서 읽기·쓰기·삭제 공유로 연다.
fn shared_read(path: &Path) -> Result<Zeroizing<Vec<u8>>, ApiFailure> {
    let mut file = File::open(path).map_err(internal)?;
    let length = file.metadata().map_err(internal)?.len();
    if length > MAX_FILE {
        return Err(internal("지원하는 DB 크기를 초과했습니다."));
    }
    let mut bytes = Zeroizing::new(vec![0u8; length as usize]);
    file.read_exact(&mut bytes).map_err(internal)?;
    Ok(bytes)
}

fn first_page(path: &Path) -> Option<[u8; PAGE_SIZE]> {
    let mut file = File::open(path).ok()?;
    if file.metadata().ok()?.len() < PAGE_SIZE as u64 {
        return None;
    }
    let mut page = [0u8; PAGE_SIZE];
    file.read_exact(&mut page).ok()?;
    Some(page)
}

fn wal_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

/// 키를 다시 찾는다. 원본 `Discover`.
fn discover(root: &Path, state: &mut State) -> Result<(), ApiFailure> {
    state.keys.clear();
    if !root.is_dir() {
        return Err(ApiFailure::new(
            404,
            "kakao_data_missing",
            "카카오톡 로컬 데이터가 없습니다.",
        ));
    }
    let mut pages = Vec::new();
    for profile in profiles(root) {
        let chat = profile.join("chat_data");
        if !chat.is_dir() {
            continue;
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&chat)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("chatLogs_") && n.ends_with(".edb"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        files.push(chat.join("chatListInfo.edb"));
        files.push(profile.join("TalkUserDB.edb"));
        for path in files {
            if let Some(page) = first_page(&path) {
                pages.push((path, page));
            }
        }
    }
    for candidate in memory_keys::read()? {
        for (path, page) in &pages {
            if !state.keys.contains_key(path) && cipher::verify_key(page, candidate.as_ref()) {
                state.keys.insert(path.clone(), candidate.clone());
            }
        }
    }
    Ok(())
}

/// DB와 WAL을 두 번 읽어 같을 때만 복호화한다. 바뀌는 중이면 세 번까지 다시 시도한다. 원본 `Open`.
fn open(state: &State, path: &Path) -> Result<MemoryChatDb, ApiFailure> {
    let Some(key) = state.keys.get(path) else {
        return Err(ApiFailure::new(
            409,
            "key_unavailable",
            "카카오톡에서 해당 방을 연 뒤 로컬 대화방을 새로고침하세요.",
        ));
    };
    let wal_path = wal_of(path);
    let read_wal = || {
        if wal_path.exists() {
            shared_read(&wal_path)
        } else {
            Ok(Zeroizing::new(Vec::new()))
        }
    };
    for _ in 0..READ_ATTEMPTS {
        let database = shared_read(path)?;
        let wal = read_wal()?;
        if *database != *shared_read(path)? || *wal != *read_wal()? {
            continue;
        }
        let plain = cipher::decrypt(&database, &wal, key.as_ref()).map_err(internal)?;
        return MemoryChatDb::open(plain).map_err(internal);
    }
    Err(ApiFailure::new(
        409,
        "database_busy",
        "대화 데이터가 변경 중입니다. 잠시 후 다시 읽어주세요.",
    ))
}

fn direct_expression(db: &MemoryChatDb) -> Result<&'static str, ApiFailure> {
    let columns: Vec<String> = db
        .query("PRAGMA table_info(chatRoomList)")
        .map_err(internal)?
        .into_iter()
        .filter_map(|row| row.into_iter().nth(1))
        .collect();
    let has_all = ["type", "activeMembersCount", "directChatMemberId", "linkId"]
        .iter()
        .all(|c| columns.iter().any(|x| x == c));
    Ok(if has_all {
        "CASE WHEN type='DirectChat' AND activeMembersCount=2 AND coalesce(linkId,0)=0 AND directChatMemberId>0 THEN CAST(directChatMemberId AS TEXT) ELSE '' END"
    } else {
        "''"
    })
}

fn load_rooms(root: &Path, state: &mut State) -> Result<Vec<LocalRoom>, ApiFailure> {
    discover(root, state)?;
    let mut found = Vec::new();
    for path in profiles(root) {
        let profile = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let list = path.join("chat_data").join("chatListInfo.edb");
        if !state.keys.contains_key(&list) {
            continue;
        }
        let db = open(state, &list)?;
        let direct = direct_expression(&db)?;
        for row in db
            .query(&format!(
                "SELECT chatId,chatRoomTitle,{direct} FROM chatRoomList"
            ))
            .map_err(internal)?
        {
            let [id, title, member] =
                <[String; 3]>::try_from(row).map_err(|_| internal("SQLite 조회 실패"))?;
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let log = path.join("chat_data").join(format!("chatLogs_{id}.edb"));
            if log.exists() {
                found.push(LocalRoom {
                    profile: profile.clone(),
                    title: if title.trim().is_empty() {
                        format!("방 {id}")
                    } else {
                        title
                    },
                    readable: state.keys.contains_key(&log),
                    direct_member_id: (!member.is_empty()).then_some(member),
                    id,
                });
            }
        }
    }
    found.sort_by(|a, b| {
        b.readable
            .cmp(&a.readable)
            .then_with(|| culture_order(&a.title, &b.title))
    });
    state.rooms = found.clone();
    Ok(found)
}

fn read_room(
    root: &Path,
    state: &mut State,
    profile: &str,
    room_id: &str,
    limit: usize,
) -> Result<ChatContext, ApiFailure> {
    if !(1..=500).contains(&limit) {
        return Err(ApiFailure::new(
            400,
            "invalid_limit",
            "메시지 수는 1~500입니다.",
        ));
    }
    let Some(index) = state
        .rooms
        .iter()
        .position(|r| r.profile == profile && r.id == room_id)
    else {
        return Err(ApiFailure::new(
            404,
            "room_not_loaded",
            "로컬 대화방을 먼저 새로고침하세요.",
        ));
    };
    let folder = root.join(profile);
    let log = folder
        .join("chat_data")
        .join(format!("chatLogs_{room_id}.edb"));
    if !state.keys.contains_key(&log) {
        discover(root, state)?;
    }
    let mut names = HashMap::new();
    let users = folder.join("TalkUserDB.edb");
    if state.keys.contains_key(&users) {
        let people = open(state, &users)?;
        for row in people
            .query("SELECT userId,coalesce(nullif(friendNickName,''),nickName,'') FROM talkUser")
            .map_err(internal)?
        {
            let [id, name] =
                <[String; 2]>::try_from(row).map_err(|_| internal("SQLite 조회 실패"))?;
            names.insert(id, name);
        }
    }
    let db = open(state, &log)?;
    // room_id는 rooms에 있는 숫자 ID이고 limit는 위에서 범위를 확인했다.
    let sql = format!(
        "SELECT logId,authorId,sendAt,type,message,coalesce(deleted,0),coalesce(attachement,'{{}}') FROM chatLogs ORDER BY logId DESC LIMIT {limit}"
    );
    let mut messages = Vec::new();
    for row in db.query(&sql).map_err(internal)? {
        let [id, author_id, sent_at, kind, text, deleted, attachment] =
            <[String; 7]>::try_from(row).map_err(|_| internal("SQLite 조회 실패"))?;
        let seconds: i64 = sent_at.parse().map_err(internal)?;
        let time = DateTime::from_timestamp(seconds, 0)
            .ok_or_else(|| internal("메시지 시각이 범위를 벗어났습니다."))?;
        let author = names
            .get(&author_id)
            .cloned()
            .unwrap_or_else(|| author_id.clone());
        let kind = kind.parse().map_err(internal)?;
        let mut message = LocalMessage::new(
            id,
            author_id,
            author,
            time.fixed_offset(),
            kind,
            text,
            deleted != "0",
        );
        message.attachment_metadata = Some(attachment);
        messages.push(message);
    }
    messages.reverse();
    state.rooms[index].readable = true;
    Ok(ChatContext {
        room: state.rooms[index].clone(),
        messages,
        read_at: Local::now().fixed_offset(),
    })
}

fn direct_peer(
    root: &Path,
    state: &mut State,
    room: &LocalRoom,
    peer: &str,
) -> Result<bool, ApiFailure> {
    let Some(known) = state
        .rooms
        .iter()
        .find(|r| r.profile == room.profile && r.id == room.id)
        .cloned()
    else {
        return Ok(false);
    };
    let listing = open(
        state,
        &root
            .join(&known.profile)
            .join("chat_data")
            .join("chatListInfo.edb"),
    )?;
    let direct = direct_expression(&listing)?;
    let rows = listing
        .query(&format!(
            "SELECT {direct} FROM chatRoomList WHERE chatId={}",
            known.id
        ))
        .map_err(internal)?;
    if rows.len() > 1 {
        return Err(internal("Sequence contains more than one element"));
    }
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .is_some_and(|v| v == peer))
}

#[async_trait]
impl ChatReader for LocalChatReader {
    async fn rooms(&self) -> Result<Vec<LocalRoom>, ApiFailure> {
        self.locked(load_rooms).await
    }

    async fn read(
        &self,
        profile: &str,
        room_id: &str,
        limit: usize,
    ) -> Result<ChatContext, ApiFailure> {
        let (profile, room_id) = (profile.to_string(), room_id.to_string());
        self.locked(move |root, state| read_room(root, state, &profile, &room_id, limit))
            .await
    }

    async fn is_direct_peer(&self, room: &LocalRoom, peer: &str) -> Result<bool, ApiFailure> {
        let (room, peer) = (room.clone(), peer.to_string());
        self.locked(move |root, state| direct_peer(root, state, &room, &peer))
            .await
    }
}
