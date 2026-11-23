//! 원본: `CommandRoomService.cs`. 1:1 제어방 설정·저장소, 명령 정책(`CommandPolicy`), 폴링 서비스.
//! 명령은 해석·실행 전에 소비해 기록한다. 중단돼도 변경을 다시 실행하지 않는다.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset, Utc};
use kakaorelay_ai::ai_service::AiService;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::relay_command::{self, RelayCommand};
use kakaorelay_ai::settings::{AiSettings, KnowledgeRoomSubscription, room_key};
use kakaorelay_ai::text::utf16_len;
use kakaorelay_ai::{ChatContext, Error, LocalMessage, LocalRoom, Result, json};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

use crate::api_send::ApiSendCommand;
use crate::auto_reply::{AutoReplySession, CommandSend};
use crate::delegations::{BackgroundDelegations, DelegationSend};
use crate::fs::write_atomic;
use crate::ids::MessageId;
use crate::reply_runtime::CancellableSend;
use crate::support::{BoxFuture, StatusEvent, chat_read, chat_rooms};

/// Web 형식 읽기에서 대소문자를 무시할 속성 이름들.
const KNOWN: &[&str] = &[
    "enabled",
    "naturalLanguage",
    "room",
    "operatorId",
    "generation",
    "reply",
    "knowledge",
    "forward",
    "provider",
    "profile",
    "id",
    "title",
    "readable",
    "directMemberId",
    "cursor",
    "pending",
    "history",
    "code",
    "command",
    "revision",
    "expires",
    "action",
    "target",
    "value",
    "messageId",
    "result",
    "at",
    "before",
    "after",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommandRoomSettings {
    pub enabled: bool,
    pub natural_language: bool,
    pub room: Option<LocalRoom>,
    pub operator_id: String,
    pub generation: Uuid,
    pub reply: bool,
    pub knowledge: bool,
    pub forward: bool,
    pub provider: bool,
}

impl Default for CommandRoomSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            natural_language: false,
            room: None,
            operator_id: String::new(),
            generation: Uuid::new_v4(),
            reply: true,
            knowledge: true,
            forward: false,
            provider: false,
        }
    }
}

impl CommandRoomSettings {
    pub fn owns(&self, room: &LocalRoom) -> bool {
        self.enabled
            && self
                .room
                .as_ref()
                .is_some_and(|r| r.profile == room.profile && r.id == room.id)
    }

    pub fn validate(&self) -> Result<()> {
        let operator_ok = MessageId::parse(&self.operator_id).is_ok_and(|id| id.is_positive());
        let invalid = self.generation.is_nil()
            || (self.enabled
                && match &self.room {
                    None => true,
                    Some(room) => {
                        !operator_ok
                            || room.direct_member_id.as_deref() != Some(self.operator_id.as_str())
                    }
                });
        if invalid {
            return Err(Error::argument("일반 1:1 방과 해당 상대방을 확인하세요."));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CommandRoomStore {
    root: PathBuf,
}

impl CommandRoomStore {
    pub fn default_root() -> PathBuf {
        kakaorelay_ai::paths::app_data().join("command-room")
    }

    pub fn new(folder: Option<PathBuf>) -> Self {
        Self {
            root: folder.unwrap_or_else(Self::default_root),
        }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn load(&self) -> Result<CommandRoomSettings> {
        Ok(self.read("settings.json")?.unwrap_or_default())
    }

    pub fn save(&self, value: &CommandRoomSettings) -> Result<()> {
        value.validate()?;
        self.write("settings.json", value)
    }

    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        write_atomic(
            &self.root.join(name),
            json::indented(value).as_bytes(),
            false,
        )
    }

    pub fn read<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let path = self.root.join(name);
        if !path.exists() {
            return Ok(None);
        }
        let mut value: serde_json::Value =
            serde_json::from_str(&kakaorelay_ai::text::read_text(&path)?)?;
        json::normalize_keys(&mut value, KNOWN, &[]);
        Ok(serde_json::from_value(value)?)
    }
}

#[derive(Debug, Clone)]
pub struct CommandPlan {
    pub command: RelayCommand,
    pub next: AiSettings,
    pub room: Option<LocalRoom>,
    pub description: String,
    pub revision: String,
}

/// 원본 `CommandPolicy` 정적 함수들.
pub mod policy {
    use super::*;

    /// 옵션 없는 직렬화의 해시. 확인 대기 중 설정이 바뀌었는지만 비교한다.
    pub fn revision(settings: &AiSettings) -> String {
        json::hash(&json::default_pascal(settings))
    }

    pub fn snapshot(s: &AiSettings) -> String {
        json::indented(&serde_json::json!({
            "selectedBotRooms": s.selected_bot_rooms,
            "roomReplyModes": s.room_reply_modes,
            "roomPersonas": s.room_personas,
            "autoKnowledge": s.auto_knowledge,
            "summaryForwards": s.summary_forwards,
            "provider": s.provider,
        }))
    }

    pub fn prepare(
        c: &RelayCommand,
        current: &AiSettings,
        rooms: &[LocalRoom],
        policy: &CommandRoomSettings,
    ) -> Result<CommandPlan> {
        if utf16_len(&c.target) > 500 || utf16_len(&c.value) > 500 {
            return Err(Error::argument("잘못된 명령입니다."));
        }
        let mut next = current.clone();
        let mut target: Option<LocalRoom> = None;
        let resolve = || -> Result<LocalRoom> {
            let matches: Vec<&LocalRoom> = rooms
                .iter()
                .filter(|r| {
                    r.title == c.target
                        || r.id == c.target
                        || room_key(&r.profile, &r.id) == c.target
                })
                .collect();
            match matches.as_slice() {
                [only] => Ok((*only).clone()),
                _ => Err(Error::argument(
                    "방을 하나로 특정할 수 없습니다. /상태에서 정확한 이름 또는 ID를 확인하세요.",
                )),
            }
        };
        let toggle = || match c.value.as_str() {
            "켜기" => Ok(true),
            "끄기" => Ok(false),
            _ => Err(Error::argument("켜기 또는 끄기를 지정하세요.")),
        };
        let minutes = |minimum: i32| match c.value.parse::<i32>() {
            Ok(n) if (minimum..=1440).contains(&n) => Ok(n),
            _ => Err(Error::argument(format!("주기는 {minimum}~1440분입니다."))),
        };
        let permit = |allowed: bool| {
            if allowed {
                Ok(())
            } else {
                Err(Error::invalid_operation(
                    "앱에서 이 제어 기능을 허용하지 않았습니다.",
                ))
            }
        };
        let description;
        match c.action.as_str() {
            "reply" => {
                permit(policy.reply)?;
                let on = toggle()?;
                if c.target == "전체" && !on {
                    description = "자동답장 전체 중지".to_string();
                } else {
                    let room = resolve()?;
                    if policy.owns(&room) {
                        return Err(Error::argument("제어방은 일반 자동답장과 분리됩니다."));
                    }
                    let key = room_key(&room.profile, &room.id);
                    if on {
                        if !next.selected_bot_rooms.contains(&key) {
                            next.selected_bot_rooms.push(key);
                        }
                    } else if let Some(i) = next.selected_bot_rooms.iter().position(|k| *k == key) {
                        next.selected_bot_rooms.remove(i);
                    }
                    description = format!("{} 자동답장 {}", room.title, c.value);
                    target = Some(room);
                }
            }
            "mode" => {
                permit(policy.reply)?;
                let room = resolve()?;
                let mode = match c.value.as_str() {
                    "즉시" => "immediate",
                    "호출" => "trigger",
                    "맥락" => "context",
                    _ => return Err(Error::argument("답변 방식은 즉시/호출/맥락입니다.")),
                };
                next.room_reply_modes
                    .insert(room_key(&room.profile, &room.id), mode.into());
                description = format!("{} 답변 방식 → {}", room.title, c.value);
                target = Some(room);
            }
            "persona" => {
                permit(policy.reply)?;
                let room = resolve()?;
                let people: Vec<_> = next
                    .personas
                    .iter()
                    .filter(|p| p.id == c.value || p.persona.name == c.value)
                    .cloned()
                    .collect();
                let [person] = people.as_slice() else {
                    return Err(Error::argument("봇이름이 없거나 중복됩니다."));
                };
                next.room_personas
                    .insert(room_key(&room.profile, &room.id), person.id.clone());
                description = format!("{} 봇 → {}", room.title, person.persona.name);
                target = Some(room);
            }
            "knowledge" => {
                permit(policy.knowledge)?;
                let room = resolve()?;
                let add = toggle()?;
                if add
                    && !std::path::Path::new(
                        &next
                            .resolve_persona(&room.profile, &room.id)?
                            .knowledge
                            .vault_path,
                    )
                    .is_dir()
                {
                    return Err(Error::argument(
                        "앱에서 이 방의 봇에 보관함을 먼저 연결하세요.",
                    ));
                }
                let config = &mut next.auto_knowledge;
                if add {
                    if !config
                        .rooms
                        .iter()
                        .any(|r| r.profile == room.profile && r.room_id == room.id)
                    {
                        config.rooms.push(KnowledgeRoomSubscription::new(
                            &room.profile,
                            &room.id,
                            &room.title,
                            Uuid::new_v4(),
                        ));
                    }
                    config.enabled = true;
                } else {
                    config
                        .rooms
                        .retain(|r| !(r.profile == room.profile && r.room_id == room.id));
                    if config.rooms.is_empty() {
                        config.enabled = false;
                    }
                }
                description = format!(
                    "{} 지식 자동 축적 {}{}",
                    room.title,
                    c.value,
                    if add {
                        " · 새로 추가하면 지금 이후 대화부터"
                    } else {
                        ""
                    }
                );
                target = Some(room);
            }
            "knowledge-interval" => {
                permit(policy.knowledge)?;
                next.auto_knowledge.interval_minutes = minutes(5)?;
                description = format!("지식 축적 주기 → {}분", c.value);
            }
            "knowledge-run" => {
                permit(policy.knowledge)?;
                if !next.auto_knowledge.enabled {
                    return Err(Error::argument("지식 자동 축적이 꺼져 있습니다."));
                }
                description = "등록된 방의 지식 정리 지금 실행".into();
            }
            "forward" | "forward-interval" => {
                permit(policy.forward)?;
                let matches: Vec<usize> = (0..next.summary_forwards.len())
                    .filter(|&i| next.summary_forwards[i].id == c.target)
                    .collect();
                let index = match matches.as_slice() {
                    [] => {
                        return Err(Error::argument(
                            "/상태에 나온 정확한 정리전달 규칙 ID를 지정하세요.",
                        ));
                    }
                    [i] => *i,
                    _ => {
                        return Err(Error::invalid_operation(
                            "Sequence contains more than one matching element",
                        ));
                    }
                };
                if c.action == "forward" {
                    let enable = toggle()?;
                    if enable && !next.summary_forwards[index].enabled {
                        return Err(Error::argument(
                            "꺼진 전달 경로 재활성화는 앱에서 수신방을 확인한 뒤 적용하세요.",
                        ));
                    }
                    next.summary_forwards[index].enabled = enable;
                } else {
                    next.summary_forwards[index].interval_minutes = minutes(1)?;
                }
                description = next.summary_forwards[index].label();
            }
            "provider" => {
                permit(policy.provider)?;
                if c.value != "auto" && !next.providers.iter().any(|p| p.id == c.value && p.enabled)
                {
                    return Err(Error::argument(
                        "앱에 등록되고 활성화된 프로바이더만 선택할 수 있습니다.",
                    ));
                }
                next.provider = c.value.clone();
                description = format!("프로바이더 → {}", c.value);
            }
            _ => {
                return Err(Error::argument(
                    "지원하지 않거나 관리 의도가 불명확한 명령입니다. /도움말",
                ));
            }
        }
        next.validate()?;
        Ok(CommandPlan {
            command: c.clone(),
            next,
            room: target,
            description,
            revision: revision(current),
        })
    }
}

fn serialize_command<S: Serializer>(
    c: &RelayCommand,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    serde_json::json!({"action": c.action, "target": c.target, "value": c.value}).serialize(s)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Audit {
    pub message_id: String,
    pub action: String,
    pub result: String,
    #[serde(with = "kakaorelay_ai::json::time")]
    pub at: DateTime<FixedOffset>,
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
}

impl Audit {
    fn new(message_id: &str, action: &str, result: &str) -> Self {
        Self {
            message_id: message_id.into(),
            action: action.into(),
            result: result.into(),
            at: Utc::now().fixed_offset(),
            before: None,
            after: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Confirmation {
    pub code: String,
    #[serde(serialize_with = "serialize_command")]
    pub command: RelayCommand,
    pub revision: String,
    #[serde(with = "kakaorelay_ai::json::time")]
    pub expires: DateTime<FixedOffset>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Progress {
    pub cursor: String,
    pub pending: Option<Confirmation>,
    pub history: Vec<Audit>,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            cursor: "0".into(),
            pending: None,
            history: vec![],
        }
    }
}

pub type SettingsFn = Arc<dyn Fn() -> AiSettings + Send + Sync>;
pub type ExecuteFn =
    Arc<dyn Fn(CommandPlan, CancellationToken) -> BoxFuture<'static, Result<String>> + Send + Sync>;
pub type RuntimeStatusFn = Arc<dyn Fn() -> String + Send + Sync>;

#[derive(Default)]
struct Conversation {
    session: Option<Arc<AutoReplySession>>,
    jobs: Option<Arc<BackgroundDelegations>>,
    generation: Uuid,
}

#[derive(Clone)]
struct Reporter {
    text: Arc<Mutex<String>>,
    event: StatusEvent,
}

impl Reporter {
    fn report(&self, s: &str) {
        *self.text.lock().unwrap_or_else(|e| e.into_inner()) = s.to_string();
        self.event.emit(s);
    }
}

pub struct CommandRoomService {
    ai: Arc<AiService>,
    store: CommandRoomStore,
    settings: SettingsFn,
    execute: ExecuteFn,
    runtime_status: RuntimeStatusFn,
    send: CancellableSend,
    gate: tokio::sync::Mutex<Conversation>,
    lifetime: CancellationToken,
    busy: AtomicBool,
    reporter: Reporter,
}

fn same_generation(store: &CommandRoomStore, config: &CommandRoomSettings) -> Result<bool> {
    let current = store.load()?;
    Ok(current.generation == config.generation && current.enabled)
}

impl CommandRoomService {
    pub fn new(
        ai: Arc<AiService>,
        store: CommandRoomStore,
        settings: SettingsFn,
        execute: ExecuteFn,
        runtime_status: RuntimeStatusFn,
        send: CancellableSend,
    ) -> Self {
        Self {
            ai,
            store,
            settings,
            execute,
            runtime_status,
            send,
            gate: tokio::sync::Mutex::new(Conversation::default()),
            lifetime: CancellationToken::new(),
            busy: AtomicBool::new(false),
            reporter: Reporter {
                text: Arc::new(Mutex::new("제어방 꺼짐".into())),
                event: StatusEvent::default(),
            },
        }
    }

    pub fn stop(&self) {
        self.lifetime.cancel();
    }

    pub async fn drain(&self) {
        let mut conversation = self.gate.lock().await;
        if let Some(jobs) = conversation.jobs.take() {
            jobs.dispose().await;
        }
    }

    pub fn on_status(&self, handler: impl Fn(&str) + Send + Sync + 'static) {
        self.reporter.event.subscribe(handler);
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> String {
        self.reporter
            .text
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn history(&self) -> Result<Vec<Audit>> {
        let config = self.store.load()?;
        Ok(self
            .store
            .read::<Progress>(&format!("{}.json", config.generation))?
            .map(|p| p.history)
            .unwrap_or_default())
    }

    async fn prepare_conversation(
        &self,
        conversation: &mut Conversation,
        config: &CommandRoomSettings,
    ) {
        if conversation.generation == config.generation && conversation.jobs.is_some() {
            return;
        }
        if let Some(jobs) = conversation.jobs.take() {
            jobs.dispose().await;
        }
        let room = config.room.clone().expect("검증된 설정");
        let (ai, store, send, lifetime, config_copy) = (
            self.ai.clone(),
            self.store.clone(),
            self.send.clone(),
            self.lifetime.clone(),
            config.clone(),
        );
        let send_conversation: CommandSend = Arc::new(move |command: ApiSendCommand| {
            let (ai, store, send, lifetime, config, room) = (
                ai.clone(),
                store.clone(),
                send.clone(),
                lifetime.clone(),
                config_copy.clone(),
                room.clone(),
            );
            Box::pin(async move {
                if lifetime.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                if !same_generation(&store, &config)? {
                    // 원본은 OperationCanceledException이지만 바깥 토큰이 취소되지 않았으므로 일반 실패로 처리된다.
                    return Err(Error::invalid_operation(
                        "제어방 설정 변경으로 답변을 중단했습니다.",
                    ));
                }
                let reader = ai.chats().clone();
                let rooms = chat_rooms(reader.as_ref(), &lifetime).await?;
                let fresh = rooms
                    .iter()
                    .filter(|r| r.profile == room.profile && r.id == room.id)
                    .collect::<Vec<_>>();
                let fresh = match fresh.as_slice() {
                    [only] => Some((*only).clone()),
                    [] => None,
                    _ => {
                        return Err(Error::invalid_operation(
                            "Sequence contains more than one matching element",
                        ));
                    }
                };
                let verified = match &fresh {
                    None => false,
                    Some(f) => {
                        reader.is_direct_peer(f, &config.operator_id).await?
                            && rooms.iter().filter(|r| r.title == f.title).count() == 1
                    }
                };
                let Some(fresh) = fresh.filter(|_| verified) else {
                    return Err(Error::invalid_operation("대화 수신방 검증 실패"));
                };
                send(
                    ApiSendCommand {
                        recipient: fresh.title,
                        ..command
                    },
                    lifetime,
                )
                .await
            })
        });
        let session = Arc::new(AutoReplySession::new(
            self.ai.clone(),
            send_conversation,
            None,
            None,
        ));
        let reporter = self.reporter.clone();
        session.status.subscribe(move |s| reporter.report(s));
        let delegate_session = session.clone();
        let delegate: DelegationSend = Arc::new(move |r, m, result, phase, ct| {
            let session = delegate_session.clone();
            Box::pin(async move { session.send_reply(&r, &m, &result, &ct, &phase).await })
        });
        let reporter = self.reporter.clone();
        let jobs = BackgroundDelegations::new(
            self.ai.clone(),
            delegate,
            Arc::new(move |s: &str| reporter.report(s)),
            &self.lifetime,
            None,
        );
        conversation.session = Some(session);
        conversation.jobs = Some(Arc::new(jobs));
        conversation.generation = config.generation;
    }

    pub async fn tick(&self, ct: &CancellationToken) -> Result<()> {
        // 바깥 토큰과 서비스 수명 토큰을 함께 따른다.
        let linked = self.lifetime.child_token();
        let link = {
            let (outer, linked) = (ct.clone(), linked.clone());
            tokio::spawn(async move {
                outer.cancelled().await;
                linked.cancel();
            })
        };
        let outcome = self.tick_linked(&linked).await;
        link.abort();
        outcome
    }

    async fn tick_linked(&self, ct: &CancellationToken) -> Result<()> {
        if ct.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let Ok(mut conversation) = self.gate.try_lock() else {
            return Ok(());
        };
        self.busy.store(true, Ordering::SeqCst);
        let outcome = self.process(&mut conversation, ct).await;
        self.busy.store(false, Ordering::SeqCst);
        match outcome {
            Err(Error::Cancelled) if ct.is_cancelled() => Err(Error::Cancelled),
            Err(error) => {
                self.reporter.report(&error.to_string());
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }

    fn status_reply(&self) -> String {
        let settings = (self.settings)();
        let knowledge = &settings.auto_knowledge;
        let rooms: Vec<String> = knowledge
            .rooms
            .iter()
            .map(|r| format!("지식: {}", r.title))
            .collect();
        let rules: Vec<String> = settings
            .summary_forwards
            .iter()
            .map(|r| format!("{} · {}", r.id, r.label()))
            .collect();
        format!(
            "{}\n지식 축적: {} · {}분\n{}\n{}",
            (self.runtime_status)(),
            if knowledge.enabled {
                "켜짐"
            } else {
                "꺼짐"
            },
            knowledge.interval_minutes,
            rooms.join("\n"),
            rules.join("\n")
        )
    }

    async fn process(&self, conversation: &mut Conversation, ct: &CancellationToken) -> Result<()> {
        let config = self.store.load()?;
        config.validate()?;
        if !config.enabled {
            return Ok(());
        }
        let room = config.room.clone().expect("검증된 설정");
        let reader = self.ai.chats().clone();
        let context: ChatContext =
            match chat_read(reader.as_ref(), &room.profile, &room.id, 500, ct).await {
                Err(Error::Api(failure)) if failure.code == "room_not_loaded" => {
                    chat_rooms(reader.as_ref(), ct).await?;
                    chat_read(reader.as_ref(), &room.profile, &room.id, 500, ct).await?
                }
                other => other?,
            };
        if !reader.is_direct_peer(&room, &config.operator_id).await?
            || config.operator_id == (self.settings)().self_id
        {
            return Err(Error::invalid_operation(
                "제어방의 1:1 상대 검증 실패. 앱에서 다시 지정하세요.",
            ));
        }
        let name = format!("{}.json", config.generation);
        let mut ordered = Vec::with_capacity(context.messages.len());
        for message in context.messages {
            ordered.push((MessageId::parse(&message.id)?, message));
        }
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        let Some(mut progress) = self.store.read::<Progress>(&name)? else {
            let cursor = ordered
                .last()
                .map_or_else(|| "0".to_string(), |(_, m)| m.id.clone());
            self.store.write(
                &name,
                &Progress {
                    cursor,
                    ..Default::default()
                },
            )?;
            self.reporter.report("제어방 시작 · 이후 새 명령부터 처리");
            return Ok(());
        };
        let cursor = MessageId::parse(&progress.cursor)?;
        if ordered.last().is_some_and(|(id, _)| *id < cursor) {
            return Err(Error::invalid_operation(
                "제어방 기록이 이전보다 줄었습니다. 앱에서 다시 적용하세요.",
            ));
        }
        if ordered.len() == 500 && ordered[0].0 > cursor {
            return Err(Error::invalid_operation(
                "제어방 새 메시지가 조회 범위를 초과했습니다. 앱에서 다시 적용하세요.",
            ));
        }
        for message in ordered
            .into_iter()
            .filter(|(id, _)| *id > cursor)
            .map(|(_, m)| m)
        {
            if ct.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if !same_generation(&self.store, &config)? {
                return Ok(());
            }
            progress.cursor = message.id.clone();
            if message.deleted
                || !matches!(message.kind, 1 | 26 | 2 | 18)
                || message.author_id != config.operator_id
            {
                self.store.write(&name, &progress)?;
                continue;
            }
            // 해석·실행 전에 소비한다. 중단돼도 변경을 다시 실행하지 않는다.
            progress.history.push(Audit::new(
                &message.id,
                "received",
                "처리 중 · 중단 시 자동 재실행 안 함",
            ));
            let keep = progress.history.len().saturating_sub(100);
            progress.history.drain(..keep);
            self.store.write(&name, &progress)?;
            let mut action: Option<String> = None;
            let mut before = None;
            let mut after = None;
            let mut conversation_handled = false;
            let outcome = self
                .handle(
                    conversation,
                    &config,
                    &room,
                    &message,
                    &name,
                    &mut progress,
                    &mut action,
                    &mut before,
                    &mut after,
                    &mut conversation_handled,
                    ct,
                )
                .await;
            let reply = match outcome {
                Ok(Some(reply)) => reply,
                Ok(None) => return Ok(()), // 처리 중 설정이 바뀌었다.
                Err(Error::Cancelled) if ct.is_cancelled() => return Err(Error::Cancelled),
                Err(error) => format!("실행하지 못했습니다: {error}"),
            };
            let last = progress.history.len() - 1;
            progress.history[last] = Audit {
                before,
                after,
                ..Audit::new(&message.id, action.as_deref().unwrap_or("parse"), &reply)
            };
            self.store.write(&name, &progress)?;
            if conversation_handled {
                self.reporter.report(&reply);
                continue;
            }
            // 인증된 방에만 회신한다. 보내기 전에 신원과 제목 유일성을 다시 확인한다.
            let fresh = chat_read(reader.as_ref(), &room.profile, &room.id, 1, ct).await?;
            let all = chat_rooms(reader.as_ref(), ct).await?;
            if !same_generation(&self.store, &config)? {
                return Ok(());
            }
            if !reader.is_direct_peer(&room, &config.operator_id).await?
                || all.iter().filter(|r| r.title == fresh.room.title).count() != 1
            {
                return Err(Error::invalid_operation(
                    "제어 결과 수신방을 확정하지 못했습니다. 이력에서 확인하세요.",
                ));
            }
            let request_id = format!(
                "command-{}",
                &json::hash(&format!("{}:{}", config.generation, message.id))[..40]
            );
            let sent = (self.send)(
                ApiSendCommand::new(request_id, &fresh.room.title, &reply),
                ct.clone(),
            )
            .await;
            match sent {
                Ok(receipt) if receipt.confirmed() => self.reporter.report(&reply),
                Err(Error::Cancelled) if ct.is_cancelled() => return Err(Error::Cancelled),
                _ => {
                    progress.history[last].result =
                        format!("{reply} [회신 불확실 · 자동 재발송 안 함]");
                    self.store.write(&name, &progress)?;
                    self.reporter
                        .report("명령 처리 결과는 이력에 저장됐지만 회신을 확인하지 못했습니다.");
                }
            }
        }
        Ok(())
    }

    /// 메시지 하나의 회신 문구를 만든다. `None`은 처리 중 제어방 설정이 바뀌어 중단했다는 뜻이다.
    #[allow(clippy::too_many_arguments)]
    async fn handle(
        &self,
        conversation: &mut Conversation,
        config: &CommandRoomSettings,
        room: &LocalRoom,
        message: &LocalMessage,
        name: &str,
        progress: &mut Progress,
        action: &mut Option<String>,
        before: &mut Option<String>,
        after: &mut Option<String>,
        conversation_handled: &mut bool,
        ct: &CancellationToken,
    ) -> Result<Option<String>> {
        let slash = matches!(message.kind, 1 | 26) && message.text.trim_start().starts_with('/');
        let chat = message.kind == 2 || message.kind == 18 || (!slash && !config.natural_language);
        if !chat {
            let mut command = if slash {
                relay_command::parse(&message.text)?
            } else {
                self.ai
                    .commands
                    .interpret(&message.text, &(self.settings)(), ct)
                    .await?
            };
            *action = Some(command.action.clone());
            if !slash && matches!(command.action.as_str(), "confirm" | "cancel") {
                return Err(Error::argument(
                    "확인은 /확인 번호, 취소는 /취소로 보내세요.",
                ));
            }
            if !same_generation(&self.store, config)? {
                return Ok(None);
            }
            if slash || command.action != "none" {
                let reply = match command.action.as_str() {
                    "status" => self.status_reply(),
                    "help" => relay_command::HELP.to_string(),
                    "cancel" => {
                        progress.pending = None;
                        "대기 중인 변경을 취소했습니다.".to_string()
                    }
                    _ => {
                        let mut confirmed = false;
                        if command.action == "confirm" {
                            let pending = progress.pending.take();
                            self.store.write(name, progress)?;
                            let valid = pending.filter(|p| {
                                p.code == command.value
                                    && p.expires >= Utc::now().fixed_offset()
                                    && p.revision == policy::revision(&(self.settings)())
                            });
                            let Some(pending) = valid else {
                                return Err(Error::argument(
                                    "확인번호가 없거나 만료됐거나 설정이 바뀌었습니다. 다시 요청하세요.",
                                ));
                            };
                            command = pending.command;
                            *action = Some(command.action.clone());
                            confirmed = true;
                        }
                        let available = chat_rooms(self.ai.chats().as_ref(), ct).await?;
                        let member = available
                            .iter()
                            .filter(|r| r.profile == room.profile && r.id == room.id)
                            .collect::<Vec<_>>();
                        let member = match member.as_slice() {
                            [only] => only.direct_member_id.clone(),
                            [] => None,
                            _ => {
                                return Err(Error::invalid_operation(
                                    "Sequence contains more than one matching element",
                                ));
                            }
                        };
                        if member.as_deref() != Some(config.operator_id.as_str()) {
                            return Err(Error::invalid_operation(
                                "제어방 상대방 검증이 변경되어 실행하지 않았습니다.",
                            ));
                        }
                        let plan =
                            policy::prepare(&command, &(self.settings)(), &available, config)?;
                        if !slash && !confirmed {
                            let mut bytes = [0u8; 3];
                            getrandom::fill(&mut bytes)
                                .map_err(|e| Error::invalid_operation(e.to_string()))?;
                            let code = json::upper_hex(&bytes);
                            let pinned = match &plan.room {
                                None => command.clone(),
                                Some(r) => RelayCommand {
                                    target: room_key(&r.profile, &r.id),
                                    ..command.clone()
                                },
                            };
                            progress.pending = Some(Confirmation {
                                code: code.clone(),
                                command: pinned,
                                revision: plan.revision.clone(),
                                expires: Utc::now().fixed_offset() + chrono::Duration::minutes(5),
                            });
                            format!(
                                "{}\n적용하려면 /확인 {code} (5분 이내). /취소",
                                plan.description
                            )
                        } else {
                            progress.pending = None;
                            self.store.write(name, progress)?;
                            *before = Some(policy::snapshot(&(self.settings)()));
                            let reply = (self.execute)(plan, ct.clone()).await?;
                            *after = Some(policy::snapshot(&(self.settings)()));
                            reply
                        }
                    }
                };
                return Ok(Some(reply));
            }
        }
        // 일반 대화: 페르소나 답장 규칙(이미지·위임 포함)을 그대로 쓴다.
        *action = Some("chat".into());
        *conversation_handled = true;
        self.prepare_conversation(conversation, config).await;
        let session = conversation.session.clone().expect("방금 준비함");
        let jobs = conversation.jobs.clone().expect("방금 준비함");
        let result = session.reply_to(room, message, &jobs, ct).await?;
        Ok(Some(result.text))
    }
}
