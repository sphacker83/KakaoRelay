//! `ChatReader` 맥 구현. 원본 `LocalChatReader`(Windows)의 방 목록·메시지 매핑·오류 코드를 맞춘다.
//!
//! Windows와의 대응:
//! - `profile`은 활성 계정 DB 파일명, `id`는 `chatId`다.
//! - 1:1 판정은 Windows `type='DirectChat' AND activeMembersCount=2 AND linkId=0 AND directChatMemberId>0`과 같다.
//!   맥 `NTChatRoom.type`은 정수이며 실측상 0이 일반 1:1(225개 모두 상대 ID 있음, linkId 0)이다.
//! - 메시지 type 값은 카카오 공통 값(1 텍스트, 2 사진, 18 파일 …)이다. 삭제된 메시지는 맥에 별도 열이 없고
//!   type에 0x4000 비트가 더해져 남는다. 그래서 `kind`는 비트를 뺀 원래 type, `deleted`는 비트 여부로 만든다.
//! - 첨부 메타데이터는 `attachment` JSON 그대로다. 사진(`s`·`cs`·`url`)과 파일(`name`·`size`·`cs`·`url`) 키가 Windows와 같다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Local, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use kakaorelay_ai::{ApiFailure, ChatContext, ChatReader, LocalMessage, LocalRoom};

use crate::{db, device, key};

/// 삭제된 메시지 표시 비트.
pub const DELETED_FLAG: i64 = 0x4000;
/// 일반 1:1 방(`NTChatRoom.type`).
pub const DIRECT_CHAT: i64 = 0;
/// 나와의 채팅(`NTChatRoom.type`). 구성원이 나뿐이다.
pub const MEMO_CHAT: i64 = 5;
/// SHA512 대조 상한. 실측한 9자리 ID는 10코어에서 약 5초에 찾았다.
const BRUTE_FORCE_LIMIT: u64 = 10_000_000_000;

pub fn kind_and_deleted(raw: i64) -> (i32, bool) {
    ((raw & !DELETED_FLAG) as i32, raw & DELETED_FLAG != 0)
}

/// 방 제목. 이름 없는 방은 카카오톡처럼 구성원 이름으로 만든다.
pub fn room_title(
    chat_id: i64,
    kind: i64,
    chat_name: &str,
    direct_member: i64,
    members: &[i64],
    self_id: u64,
    names: &HashMap<i64, String>,
) -> String {
    if !chat_name.trim().is_empty() {
        return chat_name.to_owned();
    }
    let name = |id: &i64| names.get(id).filter(|n| !n.is_empty()).cloned();
    let joined = if kind == MEMO_CHAT {
        name(&(self_id as i64))
    } else if direct_member > 0 {
        name(&direct_member)
    } else {
        let list: Vec<String> = members
            .iter()
            .filter(|id| **id != self_id as i64)
            .filter_map(name)
            .collect();
        (!list.is_empty()).then(|| list.join(", "))
    };
    joined.unwrap_or_else(|| format!("방 {chat_id}"))
}

/// `displayMemberIds` BLOB은 정수 배열 bplist다.
pub fn member_ids(blob: Option<&[u8]>) -> Vec<i64> {
    let Some(bytes) = blob else { return Vec::new() };
    match plist::Value::from_reader(std::io::Cursor::new(bytes)) {
        Ok(plist::Value::Array(items)) => {
            items.iter().filter_map(|v| v.as_signed_integer()).collect()
        }
        _ => Vec::new(),
    }
}

/// 활성 계정. passphrase는 메모리에만 둔다.
struct Account {
    path: PathBuf,
    profile: String,
    user_id: u64,
    passphrase: String,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Account(..)")
    }
}

fn locate(home: &Path, self_id: Option<u64>) -> Result<Account, ApiFailure> {
    let dir = db::database_dir(home);
    let databases = db::database_candidates(&dir)?;
    if databases.is_empty() {
        return Err(ApiFailure::new(
            404,
            "kakao_data_missing",
            "카카오톡 로컬 데이터가 없습니다. 맥용 카카오톡에 로그인했는지 확인하세요.",
        ));
    }
    let uuid = device::platform_uuid().ok_or_else(|| {
        ApiFailure::new(
            409,
            "device_id_unavailable",
            "이 맥의 기기 UUID를 읽지 못했습니다.",
        )
    })?;
    let try_id = |user_id: u64| {
        let passphrase = key::passphrase(&uuid, user_id);
        databases
            .iter()
            .find(|path| db::open(path, &passphrase).is_ok())
            .map(|path| Account {
                profile: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: path.clone(),
                user_id,
                passphrase,
            })
    };
    if let Some(account) = self_id.and_then(try_id) {
        return Ok(account);
    }
    let plist = db::preferences_plist(home);
    if let Err(error) = std::fs::metadata(&plist)
        && error.kind() == std::io::ErrorKind::PermissionDenied
    {
        return Err(db::full_disk_access_failure());
    }
    let hints = key::plist_hints(&plist).unwrap_or_default();
    for candidate in hints
        .candidates
        .iter()
        .copied()
        .filter(|n| hints.hashes.contains(&key::sha512_of(*n)))
    {
        if let Some(account) = try_id(candidate) {
            return Ok(account);
        }
    }
    let mut remaining = hints.hashes.clone();
    while let Some(candidate) = key::brute_force(&remaining, BRUTE_FORCE_LIMIT) {
        if let Some(account) = try_id(candidate) {
            return Ok(account);
        }
        remaining.remove(&key::sha512_of(candidate));
    }
    Err(ApiFailure::new(
        409,
        "user_id_unavailable",
        "카카오톡 DB를 열 사용자 ID를 찾지 못했습니다. 설정의 '내 사용자 ID'를 확인하세요.",
    ))
}

fn query_failure(error: rusqlite::Error) -> ApiFailure {
    ApiFailure::new(
        409,
        "local_unavailable",
        format!("카카오톡 DB를 읽지 못했습니다: {error}"),
    )
}

/// 열린 연결과 캐시. 연결은 폴링 사이에 재사용하고, 조회 오류가 나면 다음 호출에서 다시 연다.
#[derive(Default)]
struct State {
    account: Option<Account>,
    connection: Option<Connection>,
    rooms: Vec<LocalRoom>,
}

pub struct Store {
    home: PathBuf,
    self_id: Option<u64>,
    state: Mutex<State>,
}

/// 발송 확인에 쓰는 DB 조회 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub chat_id: i64,
    pub log_id: i64,
}

impl Store {
    pub fn new(home: PathBuf, self_id: Option<u64>) -> Self {
        Self {
            home,
            self_id,
            state: Mutex::new(State::default()),
        }
    }

    /// 계정·연결을 준비해 조회한다. 열기·조회가 실패하면 계정(키)과 연결을 다시 준비해 한 번 더 시도한다.
    fn with<T>(
        &self,
        action: impl Fn(&Connection, &Account, &mut Vec<LocalRoom>) -> rusqlite::Result<T>,
    ) -> Result<T, ApiFailure> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut last = None;
        for _ in 0..2 {
            let State {
                account,
                connection,
                rooms,
            } = &mut *state;
            let account = match account {
                Some(account) => account,
                None => account.insert(locate(&self.home, self.self_id)?),
            };
            let connection = match connection {
                Some(connection) => connection,
                None => match db::open(&account.path, &account.passphrase) {
                    Ok(opened) => connection.insert(opened),
                    Err(error) => {
                        // 계정 전환이나 DB 교체일 수 있어 키를 다시 찾는다.
                        state.account = None;
                        last = Some(error);
                        continue;
                    }
                },
            };
            match action(connection, account, rooms) {
                Ok(value) => return Ok(value),
                Err(error) => {
                    state.connection = None;
                    last = Some(error);
                }
            }
        }
        Err(query_failure(last.expect("반복 중 실패를 기록했다")))
    }

    #[cfg(test)]
    fn for_account(path: PathBuf, user_id: u64, passphrase: &str) -> Self {
        let account = Account {
            profile: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            user_id,
            passphrase: passphrase.to_owned(),
        };
        Self {
            home: PathBuf::new(),
            self_id: None,
            state: Mutex::new(State {
                account: Some(account),
                ..State::default()
            }),
        }
    }

    pub fn user_id(&self) -> Result<u64, ApiFailure> {
        self.with(|_, account, _| Ok(account.user_id))
    }

    pub fn rooms(&self) -> Result<Vec<LocalRoom>, ApiFailure> {
        self.with(|connection, account, cache| {
            let found = load_rooms(connection, account)?;
            *cache = found.clone();
            Ok(found)
        })
    }

    pub fn read(
        &self,
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
        let room = {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state
                .rooms
                .iter()
                .find(|r| r.profile == profile && r.id == room_id)
                .cloned()
        }
        .ok_or_else(|| {
            ApiFailure::new(404, "room_not_loaded", "로컬 대화방을 먼저 새로고침하세요.")
        })?;
        let chat_id: i64 = room_id.parse().map_err(|_| {
            ApiFailure::new(404, "room_not_loaded", "로컬 대화방을 먼저 새로고침하세요.")
        })?;
        let messages = self.with(|connection, account, _| {
            if account.profile != profile {
                return Ok(None);
            }
            load_messages(connection, chat_id, limit).map(Some)
        })?;
        let messages = messages.ok_or_else(|| {
            ApiFailure::new(404, "room_not_loaded", "로컬 대화방을 먼저 새로고침하세요.")
        })?;
        Ok(ChatContext {
            room: LocalRoom {
                readable: true,
                ..room
            },
            messages,
            read_at: Local::now().fixed_offset(),
        })
    }

    /// 지정한 방들 중 가장 큰 logId. 발송 직전 기준점이다.
    pub fn last_log_id(&self, chat_ids: &[i64]) -> Result<i64, ApiFailure> {
        self.with(|connection, _, _| {
            let mut max = 0;
            for chat_id in chat_ids {
                let value: Option<i64> = connection
                    .query_row(
                        "SELECT max(logId) FROM NTChatMessage WHERE chatId=?1",
                        [chat_id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                max = max.max(value.unwrap_or(0));
            }
            Ok(max)
        })
    }

    /// 제목이 같은 방의 chatId. 발송 대상 창 제목을 DB 방으로 연결한다.
    pub fn chats_titled(&self, title: &str) -> Result<Vec<i64>, ApiFailure> {
        let rooms = self.rooms()?;
        Ok(rooms
            .iter()
            .filter(|r| r.title == title)
            .filter_map(|r| r.id.parse().ok())
            .collect())
    }

    /// 기준 logId 이후 내가 보낸 메시지 중 조건에 맞는 것.
    pub fn find_sent(
        &self,
        chat_ids: &[i64],
        after: i64,
        kinds: &[i32],
        text: Option<&str>,
    ) -> Result<Option<Sent>, ApiFailure> {
        self.with(|connection, account, _| {
            for chat_id in chat_ids {
                let mut statement = connection.prepare_cached(
                    "SELECT logId, type, coalesce(message,'') FROM NTChatMessage WHERE chatId=?1 AND logId>?2 AND authorId=?3 ORDER BY logId",
                )?;
                let rows = statement.query_map(params![chat_id, after, account.user_id as i64], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
                })?;
                for row in rows {
                    let (log_id, raw, message) = row?;
                    let (kind, deleted) = kind_and_deleted(raw);
                    if deleted || !kinds.contains(&kind) {
                        continue;
                    }
                    if text.is_none_or(|t| kakaorelay_core::platform::same_text(&message, t)) {
                        return Ok(Some(Sent { chat_id: *chat_id, log_id }));
                    }
                }
            }
            Ok(None)
        })
    }
}

fn names(connection: &Connection) -> rusqlite::Result<HashMap<(i64, i64), String>> {
    let mut statement = connection.prepare(
        "SELECT userId, linkId, coalesce(nullif(friendNickName,''), nullif(nickName,''), nullif(displayName,''), '') FROM NTUser",
    )?;
    let rows = statement.query_map([], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?;
    rows.collect()
}

/// 방의 linkId(오픈채팅 프로필) 이름을 우선하고, 없으면 일반 프로필 이름을 쓴다.
fn names_for(all: &HashMap<(i64, i64), String>, link_id: i64) -> HashMap<i64, String> {
    let mut map: HashMap<i64, String> = HashMap::new();
    for ((user, link), name) in all {
        if *link == 0 && !map.contains_key(user) {
            map.insert(*user, name.clone());
        }
    }
    if link_id != 0 {
        for ((user, link), name) in all {
            if *link == link_id && !name.is_empty() {
                map.insert(*user, name.clone());
            }
        }
    }
    map
}

fn load_rooms(connection: &Connection, account: &Account) -> rusqlite::Result<Vec<LocalRoom>> {
    let all = names(connection)?;
    let mut statement = connection.prepare(
        // 카카오톡 화면의 방 이름 순서: chatName → 방 이름 메타(NTChatMeta type 3) → 오픈채팅 링크 이름(1:1 제외) → 구성원 이름.
        // 2026-11-23 실측: chatName이 빈 단체방 28개는 메타에, 오픈채팅 37개는 NTOpenLink.linkName에 이름이 있다.
        "SELECT chatId, linkId, type,
                coalesce(nullif(chatName,''),
                         (SELECT nullif(m.groupNickname,'') FROM NTChatMeta m WHERE m.chatId=r.chatId AND m.type=3 ORDER BY m.revision DESC LIMIT 1),
                         CASE WHEN coalesce(directChatMemberUserId,0)=0 THEN (SELECT nullif(o.linkName,'') FROM NTOpenLink o WHERE o.linkId=r.linkId) END,
                         ''),
                coalesce(activeMembersCount,0), coalesce(directChatMemberUserId,0), displayMemberIds
         FROM NTChatRoom r WHERE EXISTS (SELECT 1 FROM NTChatMessage m WHERE m.chatId=r.chatId)",
    )?;
    let mut rooms: Vec<LocalRoom> = statement
        .query_map([], |r| {
            let chat_id: i64 = r.get(0)?;
            let link_id: i64 = r.get::<_, Option<i64>>(1)?.unwrap_or(0);
            let kind: i64 = r.get(2)?;
            let chat_name: String = r.get(3)?;
            let active: i64 = r.get(4)?;
            let direct: i64 = r.get(5)?;
            let blob: Option<Vec<u8>> = r.get(6)?;
            let names = names_for(&all, link_id);
            let title = room_title(
                chat_id,
                kind,
                &chat_name,
                direct,
                &member_ids(blob.as_deref()),
                account.user_id,
                &names,
            );
            let mut room =
                LocalRoom::new(account.profile.clone(), chat_id.to_string(), title, true);
            if kind == DIRECT_CHAT && active == 2 && link_id == 0 && direct > 0 {
                room.direct_member_id = Some(direct.to_string());
            }
            Ok(room)
        })?
        .collect::<rusqlite::Result<_>>()?;
    rooms.sort_by(|a, b| {
        b.readable
            .cmp(&a.readable)
            .then_with(|| a.title.cmp(&b.title))
    });
    Ok(rooms)
}

fn load_messages(
    connection: &Connection,
    chat_id: i64,
    limit: usize,
) -> rusqlite::Result<Vec<LocalMessage>> {
    let link_id: i64 = connection
        .query_row(
            "SELECT coalesce(linkId,0) FROM NTChatRoom WHERE chatId=?1",
            [chat_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let names = names_for(&names(connection)?, link_id);
    let mut statement = connection.prepare(
        "SELECT logId, authorId, CAST(coalesce(sentAt,0) AS INTEGER), type, coalesce(message,''), coalesce(attachment,'{}')
         FROM NTChatMessage WHERE chatId=?1 ORDER BY logId DESC LIMIT ?2",
    )?;
    let mut messages: Vec<LocalMessage> = statement
        .query_map(params![chat_id, limit as i64], |r| {
            let author: i64 = r.get(1)?;
            let (kind, deleted) = kind_and_deleted(r.get(3)?);
            let mut message = LocalMessage::new(
                r.get::<_, i64>(0)?.to_string(),
                author.to_string(),
                names
                    .get(&author)
                    .filter(|n| !n.is_empty())
                    .cloned()
                    .unwrap_or_else(|| author.to_string()),
                unix_time(r.get(2)?),
                kind,
                r.get::<_, String>(4)?,
                deleted,
            );
            message.attachment_metadata = Some(r.get(5)?);
            Ok(message)
        })?
        .collect::<rusqlite::Result<_>>()?;
    messages.reverse();
    Ok(messages)
}

/// 원본 `DateTimeOffset.FromUnixTimeSeconds`처럼 UTC(+00:00)로 만든다.
pub fn unix_time(seconds: i64) -> DateTime<FixedOffset> {
    Utc.timestamp_opt(seconds, 0)
        .single()
        .unwrap_or_default()
        .fixed_offset()
}

/// `ChatReader` 구현. DB 작업은 블로킹이라 `spawn_blocking`으로 돌린다(원본 `Task.Run`).
#[derive(Clone)]
pub struct MacChatReader {
    store: Arc<Store>,
}

impl MacChatReader {
    /// `self_id`는 설정의 '내 사용자 ID'다. 없으면 plist와 SHA512 대조로 찾는다.
    pub fn new(self_id: Option<u64>) -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        Self::with_store(Arc::new(Store::new(home, self_id)))
    }

    pub(crate) fn with_store(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub(crate) fn store(&self) -> Arc<Store> {
        self.store.clone()
    }

    async fn blocking<T: Send + 'static>(
        &self,
        action: impl FnOnce(&Store) -> Result<T, ApiFailure> + Send + 'static,
    ) -> Result<T, ApiFailure> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || action(&store))
            .await
            .map_err(|error| {
                ApiFailure::new(
                    500,
                    "local_unavailable",
                    format!("카카오톡 DB 작업이 중단되었습니다: {error}"),
                )
            })?
    }
}

#[async_trait]
impl ChatReader for MacChatReader {
    async fn rooms(&self) -> Result<Vec<LocalRoom>, ApiFailure> {
        self.blocking(Store::rooms).await
    }

    async fn read(
        &self,
        profile: &str,
        room_id: &str,
        limit: usize,
    ) -> Result<ChatContext, ApiFailure> {
        let (profile, room_id) = (profile.to_owned(), room_id.to_owned());
        self.blocking(move |store| store.read(&profile, &room_id, limit))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: i64 = 11;
    const PASS: &str = "test-passphrase";

    /// 카카오톡처럼 compat 3 + WAL로 쓰는 연결. 열어 둔 채로 두어야 WAL이 체크포인트·삭제되지 않는다.
    fn writer(path: &Path) -> Connection {
        let connection = Connection::open(path).unwrap();
        connection.pragma_update(None, "key", PASS).unwrap();
        connection
            .pragma_update(None, "cipher_compatibility", 3)
            .unwrap();
        connection
            .pragma_update(None, "wal_autocheckpoint", 0)
            .unwrap();
        let mode: String = connection
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        connection
            .execute_batch(
                "CREATE TABLE NTChatRoom (chatId INTEGER, linkId INTEGER, type INTEGER, chatName TEXT, activeMembersCount INTEGER,
                   directChatMemberUserId INTEGER, displayMemberIds BLOB, PRIMARY KEY (chatId, linkId));
                 CREATE TABLE NTChatMessage (chatId INTEGER, logId INTEGER, msgId INTEGER, authorId INTEGER, type INTEGER,
                   attachment TEXT, message TEXT, sentAt INTEGER, PRIMARY KEY (chatId, logId, msgId));
                 CREATE TABLE NTUser (userId INTEGER, linkId INTEGER, friendNickName TEXT, nickName TEXT, displayName TEXT,
                   PRIMARY KEY (userId, linkId));
                 CREATE TABLE NTChatMeta (chatId INTEGER, type INTEGER, revision INTEGER, content TEXT, groupNickname TEXT,
                   PRIMARY KEY (chatId, type));
                 CREATE TABLE NTOpenLink (linkId INTEGER PRIMARY KEY, linkName TEXT);",
            )
            .unwrap();
        connection
    }

    fn members(ids: &[i64]) -> Vec<u8> {
        let mut bytes = Vec::new();
        plist::Value::Array(
            ids.iter()
                .map(|id| plist::Value::Integer((*id).into()))
                .collect(),
        )
        .to_writer_binary(&mut bytes)
        .unwrap();
        bytes
    }

    fn seed(connection: &Connection) {
        connection
            .execute_batch(&format!(
                "INSERT INTO NTUser VALUES ({ME},0,'','나','나 표시'),(22,0,'친구B','B',''),(33,0,'','C',''),(44,0,'','D',''),(44,7,'','오픈D','');
                 INSERT INTO NTChatRoom VALUES (100,0,0,'',2,22,NULL),(200,0,5,NULL,1,0,NULL),(400,7,4,'오픈방',3,0,NULL),(500,0,1,'',3,0,NULL);
                 INSERT INTO NTChatMessage VALUES
                   (100,1,1,22,1,NULL,'안녕',1700000000),
                   (100,2,2,{ME},2,'{{\"s\":3,\"cs\":\"ab\",\"url\":\"https://x.kakaocdn.net/a\"}}','사진',1700000001),
                   (100,3,3,22,16385,NULL,'지운 글',1700000002),
                   (200,5,5,{ME},1,NULL,'메모',1700000003),
                   (400,6,6,44,1,NULL,'오픈',1700000004),
                   (500,7,7,33,1,NULL,'그룹',1700000005),
                   (600,8,8,33,1,NULL,'메타',1700000006),
                   (700,9,9,44,1,NULL,'오픈 이름',1700000007),
                   (800,10,10,44,1,NULL,'오픈 1:1',1700000008);
                 INSERT INTO NTChatRoom VALUES (600,0,1,'',3,0,NULL),(700,8,4,'',3,0,NULL),(800,9,4,'',2,44,NULL);
                 INSERT INTO NTChatMeta VALUES (600,3,1,'옛 이름','방 별명'),(600,1,2,'공지','');
                 INSERT INTO NTOpenLink VALUES (8,'오픈채팅 이름'),(9,'남의 오픈 프로필');"
            ))
            .unwrap();
        connection
            .execute(
                "INSERT INTO NTChatRoom VALUES (300,0,1,'',3,0,?1)",
                [members(&[ME, 22, 33])],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO NTChatMessage VALUES (300,4,4,33,1,NULL,'그룹 글',1700000003)",
                [],
            )
            .unwrap();
    }

    fn temp_db(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("kr-macos-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db");
        (dir, path)
    }

    fn fingerprint(path: &Path) -> Vec<(u64, Vec<u8>)> {
        ["", "-wal", "-shm"]
            .iter()
            .map(|suffix| {
                let file = PathBuf::from(format!("{}{suffix}", path.display()));
                let bytes = std::fs::read(&file).unwrap();
                (bytes.len() as u64, sha2::Sha256::digest(&bytes).to_vec())
            })
            .collect()
    }

    use sha2::Digest;

    /// 카카오톡 역할의 작성 프로세스. 같은 프로세스의 연결은 `-shm` 매핑을 공유하므로 반드시 별도 프로세스여야 한다.
    #[test]
    #[ignore = "reads_wal_commits_without_touching_files가 자식 프로세스로 실행한다"]
    fn wal_writer_process() {
        use std::io::{BufRead, Write};
        let Some(path) = std::env::var_os("KR_WAL_WRITER") else {
            return;
        };
        let writer = writer(Path::new(&path));
        seed(&writer);
        let mut lines = std::io::stdin().lock().lines();
        println!("kr-ready");
        std::io::stdout().flush().unwrap();
        assert_eq!(lines.next().unwrap().unwrap(), "insert");
        writer
            .execute(
                &format!(
                    "INSERT INTO NTChatMessage VALUES (200,9,9,{ME},1,NULL,'새 메모',1700000009)"
                ),
                [],
            )
            .unwrap();
        println!("kr-inserted");
        std::io::stdout().flush().unwrap();
        assert_eq!(lines.next().unwrap().unwrap(), "quit");
        // 카카오톡이 강제 종료된 상황: 연결을 닫지 않고 끝내 WAL·SHM을 남긴다.
        std::process::exit(0);
    }

    #[test]
    fn reads_wal_commits_without_touching_files() {
        use std::io::{BufRead, Write};
        let (dir, path) = temp_db("wal");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "reader::tests::wal_writer_process",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("KR_WAL_WRITER", &path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let mut wait_for = |marker: &str| {
            for line in stdout.by_ref() {
                if line.unwrap().ends_with(marker) {
                    return;
                }
            }
            panic!("작성 프로세스가 {marker}에 도달하지 못했다");
        };
        wait_for("kr-ready");
        let wal = std::fs::metadata(format!("{}-wal", path.display()))
            .unwrap()
            .len();
        assert!(wal > 0, "커밋이 WAL에 남아 있어야 한다");
        let before = fingerprint(&path);

        let store = Store::for_account(path.clone(), ME as u64, PASS);
        let rooms = store.rooms().unwrap();
        assert_eq!(rooms.len(), 8, "WAL에만 있는 커밋을 읽는다");
        let profile = rooms[0].profile.clone();
        assert_eq!(store.read(&profile, "200", 10).unwrap().messages.len(), 1);
        assert_eq!(
            fingerprint(&path),
            before,
            "읽기 전용 연결은 DB·WAL·SHM을 바꾸지 않는다"
        );

        // 연결을 재사용한 다음 폴링에서 새 커밋이 보여야 한다.
        writeln!(stdin, "insert").unwrap();
        wait_for("kr-inserted");
        let after_insert = fingerprint(&path);
        let context = store.read(&profile, "200", 10).unwrap();
        assert_eq!(
            context
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>(),
            ["메모", "새 메모"]
        );
        assert_eq!(
            store.find_sent(&[200], 5, &[1], Some("새 메모")).unwrap(),
            Some(Sent {
                chat_id: 200,
                log_id: 9
            })
        );
        assert_eq!(
            fingerprint(&path),
            after_insert,
            "새 커밋을 읽은 뒤에도 파일을 바꾸지 않는다"
        );

        writeln!(stdin, "quit").unwrap();
        assert!(child.wait().unwrap().success());
        drop(store);

        // 다른 프로세스가 없을 때도 읽고 닫는 동안 체크포인트·WAL 삭제를 하지 않는다.
        let orphaned = fingerprint(&path);
        let store = Store::for_account(path.clone(), ME as u64, PASS);
        assert_eq!(store.rooms().unwrap().len(), 8);
        assert_eq!(store.read(&profile, "200", 10).unwrap().messages.len(), 2);
        drop(store);
        assert!(
            db::open(&path, "wrong").is_err(),
            "잘못된 키는 열리지 않는다"
        );
        assert_eq!(
            fingerprint(&path),
            orphaned,
            "단독으로 열고 닫아도 DB·WAL·SHM을 바꾸지 않는다"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn maps_schema_like_windows_reader() {
        let (dir, path) = temp_db("schema");
        let writer = writer(&path);
        seed(&writer);
        let store = Store::for_account(path.clone(), ME as u64, PASS);
        let rooms = store.rooms().unwrap();
        let by_id = |id: &str| rooms.iter().find(|r| r.id == id).unwrap().clone();
        assert_eq!(by_id("100").title, "친구B");
        assert_eq!(by_id("100").direct_member_id.as_deref(), Some("22"));
        assert_eq!(by_id("200").title, "나", "나와의 채팅은 내 이름이다");
        assert_eq!(by_id("200").direct_member_id, None);
        assert_eq!(
            by_id("300").title,
            "친구B, C",
            "이름 없는 그룹은 나를 뺀 구성원 이름이다"
        );
        assert_eq!(by_id("400").title, "오픈방");
        assert_eq!(
            by_id("600").title,
            "방 별명",
            "chatName이 비면 방 이름 메타(type 3)를 쓴다"
        );
        assert_eq!(
            by_id("700").title,
            "오픈채팅 이름",
            "chatName이 빈 오픈채팅은 링크 이름을 쓴다"
        );
        assert_ne!(
            by_id("800").title,
            "남의 오픈 프로필",
            "1:1 오픈채팅은 링크 이름 대신 상대 이름을 쓴다"
        );
        assert_eq!(
            by_id("500").title,
            "방 500",
            "구성원 정보가 없으면 원본처럼 '방 <id>'다"
        );
        assert!(rooms.iter().all(|r| r.readable && r.profile == "db"));
        assert_eq!(store.chats_titled("친구B").unwrap(), [100]);
        assert_eq!(
            store.find_sent(&[200], 0, &[1], Some("메모")).unwrap(),
            Some(Sent {
                chat_id: 200,
                log_id: 5
            })
        );
        assert_eq!(
            store.find_sent(&[200], 5, &[1], Some("메모")).unwrap(),
            None,
            "기준 logId 이전은 제외"
        );
        assert_eq!(
            store.find_sent(&[100], 0, &[1], Some("안녕")).unwrap(),
            None,
            "다른 사람이 쓴 글은 발신으로 보지 않는다"
        );
        assert_eq!(
            store.find_sent(&[100], 0, &[2, 27, 18], None).unwrap(),
            Some(Sent {
                chat_id: 100,
                log_id: 2
            })
        );
        assert_eq!(store.last_log_id(&[100, 200]).unwrap(), 5);

        let context = store.read("db", "100", 500).unwrap();
        assert!(context.room.readable);
        let kinds: Vec<(i32, bool)> = context
            .messages
            .iter()
            .map(|m| (m.kind, m.deleted))
            .collect();
        assert_eq!(
            kinds,
            [(1, false), (2, false), (1, true)],
            "오래된 순, 삭제 비트는 deleted로 분리"
        );
        let photo = &context.messages[1];
        assert_eq!(photo.author_id, ME.to_string());
        assert_eq!(photo.author, "나");
        assert_eq!(
            photo.attachment_metadata.as_deref(),
            Some(r#"{"s":3,"cs":"ab","url":"https://x.kakaocdn.net/a"}"#)
        );
        assert_eq!(
            context.messages[0].attachment_metadata.as_deref(),
            Some("{}")
        );
        assert_eq!(
            context.messages[0].time.to_rfc3339(),
            "2023-11-14T22:13:20+00:00"
        );
        assert_eq!(
            store.read("db", "100", 1).unwrap().messages.len(),
            1,
            "최근 N개"
        );
        assert_eq!(
            store.read("db", "400", 5).unwrap().messages[0].author,
            "오픈D",
            "오픈채팅은 방 프로필 이름"
        );

        assert_eq!(
            store.read("db", "100", 0).unwrap_err().code,
            "invalid_limit"
        );
        assert_eq!(store.read("db", "100", 501).unwrap_err().status_code, 400);
        assert_eq!(
            store.read("db", "999", 5).unwrap_err().code,
            "room_not_loaded"
        );
        assert_eq!(
            store.read("other", "100", 5).unwrap_err().code,
            "room_not_loaded"
        );
        drop(writer);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleted_bit_and_member_blob() {
        assert_eq!(kind_and_deleted(16386), (2, true));
        assert_eq!(kind_and_deleted(18), (18, false));
        assert_eq!(member_ids(Some(&members(&[1, 2]))), [1, 2]);
        assert!(member_ids(Some(b"not plist")).is_empty());
        assert!(member_ids(None).is_empty());
    }
}
