//! 원본: `AiSettings.cs`, `ObsidianKnowledge.cs`의 `KnowledgeSettings`, `AutoKnowledgeService.cs`의 `AutoKnowledgeSettings`.
//! `ai-settings.json`(inventory §4-1-a)의 스키마·검증·저장.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use indexmap::IndexMap;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::json;
use crate::model_catalog::{self, CliModel, CliModelCatalog};
use crate::summary_forward_settings::SummaryForwardRule;
use crate::text::{is_blank, utf16_len};

pub const ORDER: [&str; 3] = ["codex", "grok", "claude"];
pub const IMAGE_GPT_MODEL: &str = "gpt-6-astra";
pub const IMAGE_GROK_MODEL: &str = "grok-4.7";

/// `AiSettings.RoomKey`: 기본 옵션 `JsonSerializer.Serialize(new[] { profile, roomId })`.
pub fn room_key(profile: &str, room_id: &str) -> String {
    json::default(&[profile, room_id])
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BotPersona {
    pub name: String,
    pub role: String,
    pub tone: String,
    pub style: String,
    pub instructions: String,
}

impl Default for BotPersona {
    fn default() -> Self {
        Self {
            name: "그쫀쿠".into(),
            role: "대화 맥락을 이해하고 필요한 답변과 업무 정리를 돕는 AI 챗봇".into(),
            tone: "친절하고 차분한 존댓말".into(),
            style: "핵심부터 짧게 답하고, 불확실한 내용은 확인 질문을 한다. 과장하지 않는다."
                .into(),
            instructions:
                "대화에 없는 사실을 만들지 않는다. 민감한 정보는 답변에 불필요하게 반복하지 않는다."
                    .into(),
        }
    }
}

/// 프로바이더 설정. 모델 목록(`models`, `model_status`)은 실행 중에만 쓰고 저장하지 않는다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiProviderSettings {
    pub id: String,
    pub enabled: bool,
    pub executable: String,
    pub model: String,
    pub effort: String,
    /// 실행 중 조회한 모델 목록(저장 안 함).
    #[serde(skip)]
    pub models: Vec<CliModel>,
    #[serde(skip)]
    pub model_status_text: String,
}

impl Default for AiProviderSettings {
    fn default() -> Self {
        Self {
            id: "codex".into(),
            enabled: true,
            executable: String::new(),
            model: String::new(),
            effort: "default".into(),
            models: vec![],
            model_status_text: "모델 목록 확인 전".into(),
        }
    }
}

impl AiProviderSettings {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }
    pub fn model_choices(&self) -> Vec<String> {
        model_catalog::choices(&self.models, &self.model)
    }
    pub fn model_status(&self) -> String {
        let unverified = !self.model.is_empty() && !self.models.iter().any(|m| m.id == self.model);
        format!(
            "{}{}",
            self.model_status_text,
            if unverified {
                " · 현재 선택은 목록 미확인"
            } else {
                ""
            }
        )
    }
    /// 목록을 갱신해도 저장된 선택(`model`)은 바꾸지 않는다.
    pub fn set_model_catalog(&mut self, catalog: Option<&CliModelCatalog>, status: &str) {
        self.models = catalog.map(|c| c.models.clone()).unwrap_or_default();
        self.model_status_text = status.into();
    }
    /// 실행 파일이 바뀌면 이전 목록은 의미가 없으므로 비운다.
    pub fn set_executable(&mut self, value: &str) {
        if self.executable == value {
            return;
        }
        self.executable = value.into();
        self.set_model_catalog(None, "실행 파일 변경 · 모델 목록을 새로고침하세요.");
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KnowledgeSettings {
    pub enabled: bool,
    pub vault_path: String,
}

impl KnowledgeSettings {
    pub fn validate(&self) -> Result<()> {
        let invalid_chars: &[char] = if cfg!(windows) {
            &['"', '<', '>', '|', '\0']
        } else {
            &['\0']
        };
        let has_invalid = self
            .vault_path
            .chars()
            .any(|c| invalid_chars.contains(&c) || (cfg!(windows) && (c as u32) < 32));
        if utf16_len(&self.vault_path) > 4096
            || has_invalid
            || (!is_blank(&self.vault_path) && !Path::new(&self.vault_path).is_absolute())
            || (self.enabled && is_blank(&self.vault_path))
        {
            return Err(Error::argument(
                "지식베이스 보관함의 전체 폴더 경로를 입력하세요.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PersonaProfile {
    pub id: String,
    pub label: String,
    pub persona: BotPersona,
    pub attach_images: bool,
    pub knowledge: KnowledgeSettings,
}

impl Default for PersonaProfile {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4().simple().to_string(),
            label: "새 페르소나".into(),
            persona: BotPersona::default(),
            attach_images: false,
            knowledge: KnowledgeSettings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeRoomSubscription {
    pub profile: String,
    pub room_id: String,
    pub title: String,
    pub generation: Uuid,
}

impl KnowledgeRoomSubscription {
    pub fn new(
        profile: impl Into<String>,
        room_id: impl Into<String>,
        title: impl Into<String>,
        generation: Uuid,
    ) -> Self {
        Self {
            profile: profile.into(),
            room_id: room_id.into(),
            title: title.into(),
            generation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoKnowledgeSettings {
    pub enabled: bool,
    #[serde(deserialize_with = "crate::json::web_int::deserialize")]
    pub interval_minutes: i32,
    pub auto_save: bool,
    pub rooms: Vec<KnowledgeRoomSubscription>,
}

impl Default for AutoKnowledgeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: 30,
            auto_save: true,
            rooms: vec![],
        }
    }
}

impl AutoKnowledgeSettings {
    pub fn validate(&self) -> Result<()> {
        let mut keys: Vec<String> = self
            .rooms
            .iter()
            .map(|r| room_key(&r.profile, &r.room_id))
            .collect();
        keys.sort();
        keys.dedup();
        if !(5..=1440).contains(&self.interval_minutes)
            || self.rooms.len() > 20
            || self.rooms.iter().any(|r| {
                is_blank(&r.profile)
                    || is_blank(&r.room_id)
                    || utf16_len(&r.profile) > 200
                    || utf16_len(&r.room_id) > 200
                    || utf16_len(&r.title) > 500
                    || r.generation.is_nil()
            })
            || keys.len() != self.rooms.len()
            || (self.enabled && self.rooms.is_empty())
        {
            return Err(Error::argument(
                "자동 정리할 방(최대 20개)과 주기(5~1440분)를 확인하세요.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AiSettings {
    pub self_id: String,
    pub poll_seconds: i32,
    pub image_gpt_model: String,
    pub image_grok_model: String,
    pub reply_mode: String,
    pub room_reply_modes: IndexMap<String, String>,
    pub selected_bot_rooms: Vec<String>,
    pub auto_knowledge: AutoKnowledgeSettings,
    pub summary_forwards: Vec<SummaryForwardRule>,
    pub selected_chat_room: Option<String>,
    pub provider: String,
    pub context_messages: i32,
    pub timeout_seconds: i32,
    pub grok_delegation_enabled: bool,
    pub grok_timeout_seconds: i32,
    legacy_persona: BotPersona,
    pub personas: Vec<PersonaProfile>,
    pub room_personas: IndexMap<String, String>,
    pub providers: Vec<AiProviderSettings>,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            self_id: String::new(),
            poll_seconds: 5,
            image_gpt_model: IMAGE_GPT_MODEL.into(),
            image_grok_model: IMAGE_GROK_MODEL.into(),
            reply_mode: "trigger".into(),
            room_reply_modes: IndexMap::new(),
            selected_bot_rooms: vec![],
            auto_knowledge: AutoKnowledgeSettings::default(),
            summary_forwards: vec![],
            selected_chat_room: None,
            provider: "auto".into(),
            context_messages: 100,
            timeout_seconds: 180,
            grok_delegation_enabled: false,
            grok_timeout_seconds: 600,
            legacy_persona: BotPersona::default(),
            personas: vec![],
            room_personas: IndexMap::new(),
            providers: ORDER.iter().map(|id| AiProviderSettings::new(id)).collect(),
        }
    }
}

const SETTINGS_FIELDS: &[&str] = &[
    "selfId",
    "trigger",
    "pollSeconds",
    "imageGptModel",
    "imageGrokModel",
    "replyMode",
    "roomReplyModes",
    "selectedBotRooms",
    "autoKnowledge",
    "enabled",
    "intervalMinutes",
    "autoSave",
    "rooms",
    "profile",
    "roomId",
    "title",
    "generation",
    "summaryForwards",
    "id",
    "sources",
    "destination",
    "label",
    "key",
    "selectedChatRoom",
    "provider",
    "contextMessages",
    "timeoutSeconds",
    "grokDelegationEnabled",
    "grokTimeoutSeconds",
    "persona",
    "name",
    "role",
    "tone",
    "style",
    "instructions",
    "personas",
    "attachImages",
    "knowledge",
    "vaultPath",
    "roomPersonas",
    "providers",
    "executable",
    "model",
    "effort",
];
const SETTINGS_DICTIONARIES: &[&str] = &["roomReplyModes", "roomPersonas"];

fn field<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(Error::from)
}

fn int_field(value: Value) -> Result<i32> {
    crate::json::web_int::deserialize(value).map_err(Error::from)
}

impl AiSettings {
    /// 원본 `Persona`: 마이그레이션 뒤에도 첫 슬롯을 가리킨다.
    pub fn persona(&self) -> &BotPersona {
        self.personas
            .first()
            .map(|p| &p.persona)
            .unwrap_or(&self.legacy_persona)
    }
    pub fn persona_mut(&mut self) -> &mut BotPersona {
        match self.personas.first_mut() {
            Some(first) => &mut first.persona,
            None => &mut self.legacy_persona,
        }
    }
    pub fn set_persona(&mut self, persona: BotPersona) {
        if let Some(first) = self.personas.first_mut() {
            first.persona = persona.clone();
        }
        self.legacy_persona = persona;
    }
    /// 원본 `Trigger`: 저장된 값과 무관하게 첫 페르소나 이름으로 계산한다.
    pub fn trigger(&self) -> String {
        format!("@{}", self.persona().name.trim())
    }
    pub fn reply_mode_for_room(&self, profile: &str, room_id: &str) -> String {
        self.room_reply_modes
            .get(&room_key(profile, room_id))
            .cloned()
            .unwrap_or_else(|| self.reply_mode.clone())
    }

    pub fn ensure_personas(&mut self) {
        if !self.personas.is_empty() {
            return;
        }
        self.personas = default_personas(self.persona().clone());
    }

    pub fn resolve_persona(&self, profile: &str, room_id: &str) -> Result<PersonaProfile> {
        let owned;
        let personas = if self.personas.is_empty() {
            owned = default_personas(self.persona().clone());
            &owned
        } else {
            &self.personas
        };
        match self.room_personas.get(&room_key(profile, room_id)) {
            Some(id) => {
                let matches: Vec<&PersonaProfile> =
                    personas.iter().filter(|p| &p.id == id).collect();
                if matches.len() != 1 {
                    return Err(Error::invalid_operation(
                        "채팅방에 연결된 페르소나가 없습니다.",
                    ));
                }
                Ok(matches[0].clone())
            }
            None => Ok(personas[0].clone()),
        }
    }

    pub fn trigger_for_room(&self, profile: &str, room_id: &str) -> Result<String> {
        Ok(format!(
            "@{}",
            self.resolve_persona(profile, room_id)?.persona.name.trim()
        ))
    }

    pub fn image_root(persona_id: &str) -> Result<PathBuf> {
        if !valid_persona_id(persona_id) {
            return Err(Error::argument("페르소나 ID를 확인하세요."));
        }
        Ok(crate::persona_images::PersonaImageStore::default_root().join(persona_id))
    }

    pub fn provider(&self, id: &str) -> Result<&AiProviderSettings> {
        single(self.providers.iter().filter(|p| p.id == id))
    }
    pub fn provider_mut(&mut self, id: &str) -> Result<&mut AiProviderSettings> {
        let mut found = self.providers.iter_mut().filter(|p| p.id == id);
        match (found.next(), found.next()) {
            (Some(p), None) => Ok(p),
            _ => Err(Error::invalid_operation("프로바이더를 확인하세요.")),
        }
    }

    pub fn validate(&mut self) -> Result<()> {
        self.auto_knowledge.validate()?;
        let mut ids: Vec<&str> = self
            .summary_forwards
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        ids.sort();
        ids.dedup();
        if ids.len() != self.summary_forwards.len() {
            return Err(Error::argument("정리·전달 규칙을 확인하세요."));
        }
        for rule in &self.summary_forwards {
            rule.validate()?;
        }
        if [&self.image_gpt_model, &self.image_grok_model]
            .iter()
            .any(|m| !image_model_valid(m))
        {
            return Err(Error::argument("이미지 CLI 모델 이름을 확인하세요."));
        }
        if !(1..=300).contains(&self.poll_seconds) || !valid_mode(&self.reply_mode) {
            return Err(Error::argument(
                "폴링은 1~300초이며 답변 방식을 선택해야 합니다.",
            ));
        }
        if self.room_reply_modes.values().any(|m| !valid_mode(m)) {
            return Err(Error::argument("채팅방별 답변 방식을 확인하세요."));
        }
        if self.provider != "auto" && !ORDER.contains(&self.provider.as_str()) {
            return Err(Error::argument("프로바이더를 확인하세요."));
        }
        if !(1..=500).contains(&self.context_messages)
            || !(10..=900).contains(&self.timeout_seconds)
        {
            return Err(Error::argument(
                "문맥은 1~500개, 제한 시간은 10~900초입니다.",
            ));
        }
        if !(10..=1800).contains(&self.grok_timeout_seconds) {
            return Err(Error::argument("Grok 작업 제한 시간은 10~1800초입니다."));
        }
        let mut provider_ids: Vec<&str> = self.providers.iter().map(|p| p.id.as_str()).collect();
        provider_ids.sort();
        let mut expected = ORDER.to_vec();
        expected.sort();
        if self.providers.len() != 3 || provider_ids != expected {
            return Err(Error::argument("AI 설정 형식이 올바르지 않습니다."));
        }
        let persona = self.persona();
        if is_blank(&persona.name) || persona_texts(persona).iter().any(|t| utf16_len(t) > 10000) {
            return Err(Error::argument("페르소나 이름과 지침을 확인하세요."));
        }
        self.ensure_personas();
        let mut persona_ids: Vec<&str> = self.personas.iter().map(|p| p.id.as_str()).collect();
        persona_ids.sort();
        persona_ids.dedup();
        if self.personas.len() != 3 || persona_ids.len() != 3 {
            return Err(Error::argument("페르소나 3개의 ID가 서로 달라야 합니다."));
        }
        for p in &self.personas {
            p.knowledge.validate()?;
            Self::image_root(&p.id)?;
            if is_blank(&p.label)
                || is_blank(&p.persona.name)
                || utf16_len(&p.label) > 10000
                || persona_texts(&p.persona)
                    .iter()
                    .any(|t| utf16_len(t) > 10000)
            {
                return Err(Error::argument("페르소나 이름과 지침을 확인하세요."));
            }
        }
        if self
            .room_personas
            .values()
            .any(|id| !self.personas.iter().any(|p| &p.id == id))
        {
            return Err(Error::argument("채팅방에 연결된 페르소나가 없습니다."));
        }
        for p in &self.providers {
            if (!p.model.is_empty() && !model_catalog::valid_id(&p.model))
                || !valid_effort(&p.effort)
            {
                return Err(Error::argument("모델 또는 effort 값을 확인하세요."));
            }
        }
        Ok(())
    }

    /// 원본 역직렬화 순서 재현: 문서에 나온 순서대로 속성을 적용한다. 이전 형식(`personas` 없음)의
    /// `persona`는 `legacy_persona`에 들어갔다가 `ensure_personas`에서 첫 슬롯이 된다.
    pub fn from_value(mut value: Value) -> Result<Self> {
        json::normalize_keys(&mut value, SETTINGS_FIELDS, SETTINGS_DICTIONARIES);
        let Value::Object(map) = value else {
            return Err(Error::invalid_data("AI 설정을 읽을 수 없습니다."));
        };
        let mut s = Self::default();
        for (key, item) in map {
            match key.as_str() {
                "selfId" => s.self_id = field(item)?,
                "pollSeconds" => s.poll_seconds = int_field(item)?,
                "imageGptModel" => s.image_gpt_model = field(item)?,
                "imageGrokModel" => s.image_grok_model = field(item)?,
                "replyMode" => s.reply_mode = field(item)?,
                "roomReplyModes" => s.room_reply_modes = field(item)?,
                "selectedBotRooms" => s.selected_bot_rooms = field(item)?,
                "autoKnowledge" => s.auto_knowledge = field(item)?,
                "summaryForwards" => s.summary_forwards = field(item)?,
                "selectedChatRoom" => s.selected_chat_room = field(item)?,
                "provider" => s.provider = field(item)?,
                "contextMessages" => s.context_messages = int_field(item)?,
                "timeoutSeconds" => s.timeout_seconds = int_field(item)?,
                "grokDelegationEnabled" => s.grok_delegation_enabled = field(item)?,
                "grokTimeoutSeconds" => s.grok_timeout_seconds = int_field(item)?,
                "persona" => {
                    let persona: Option<BotPersona> = field(item)?;
                    s.set_persona(
                        persona
                            .ok_or_else(|| Error::argument("AI 설정 형식이 올바르지 않습니다."))?,
                    );
                }
                "personas" => s.personas = field(item)?,
                "roomPersonas" => s.room_personas = field(item)?,
                "providers" => s.providers = field(item)?,
                _ => {} // `trigger`는 읽기 전용, 그 밖의 속성은 무시한다.
            }
        }
        Ok(s)
    }
}

impl Serialize for AiSettings {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("AiSettings", 20)?;
        st.serialize_field("selfId", &self.self_id)?;
        st.serialize_field("trigger", &self.trigger())?;
        st.serialize_field("pollSeconds", &self.poll_seconds)?;
        st.serialize_field("imageGptModel", &self.image_gpt_model)?;
        st.serialize_field("imageGrokModel", &self.image_grok_model)?;
        st.serialize_field("replyMode", &self.reply_mode)?;
        st.serialize_field("roomReplyModes", &self.room_reply_modes)?;
        st.serialize_field("selectedBotRooms", &self.selected_bot_rooms)?;
        st.serialize_field("autoKnowledge", &self.auto_knowledge)?;
        st.serialize_field("summaryForwards", &self.summary_forwards)?;
        st.serialize_field("selectedChatRoom", &self.selected_chat_room)?;
        st.serialize_field("provider", &self.provider)?;
        st.serialize_field("contextMessages", &self.context_messages)?;
        st.serialize_field("timeoutSeconds", &self.timeout_seconds)?;
        st.serialize_field("grokDelegationEnabled", &self.grok_delegation_enabled)?;
        st.serialize_field("grokTimeoutSeconds", &self.grok_timeout_seconds)?;
        st.serialize_field("persona", self.persona())?;
        st.serialize_field("personas", &self.personas)?;
        st.serialize_field("roomPersonas", &self.room_personas)?;
        st.serialize_field("providers", &self.providers)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for AiSettings {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        Self::from_value(value).map_err(serde::de::Error::custom)
    }
}

fn default_personas(first: BotPersona) -> Vec<PersonaProfile> {
    let base = BotPersona::default();
    vec![
        PersonaProfile {
            id: "default".into(),
            label: "기본".into(),
            persona: first,
            ..PersonaProfile::default()
        },
        PersonaProfile {
            id: "work".into(),
            label: "업무".into(),
            persona: BotPersona {
                name: "업무 도우미".into(),
                tone: "정중하고 간결한 존댓말".into(),
                style: "할 일과 결론부터 명확하게 정리한다.".into(),
                ..base.clone()
            },
            ..PersonaProfile::default()
        },
        PersonaProfile {
            id: "friend".into(),
            label: "친구".into(),
            persona: BotPersona {
                name: "친구".into(),
                tone: "따뜻하고 편안한 말투".into(),
                style: "상대방의 감정에 공감하고 자연스럽게 대화한다.".into(),
                ..base
            },
            ..PersonaProfile::default()
        },
    ]
}

fn persona_texts(p: &BotPersona) -> [&str; 5] {
    [&p.name, &p.role, &p.tone, &p.style, &p.instructions]
}

fn valid_mode(mode: &str) -> bool {
    matches!(mode, "immediate" | "trigger" | "context")
}

fn valid_effort(effort: &str) -> bool {
    matches!(
        effort,
        "default"
            | "none"
            | "minimal"
            | "low"
            | "medium"
            | "high"
            | "xhigh"
            | "max"
            | "ultra"
            | "ultracode"
    )
}

/// `\A[a-zA-Z0-9._:/-]{1,150}\z`.
fn image_model_valid(model: &str) -> bool {
    (1..=150).contains(&model.len())
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b))
}

/// `\A[a-zA-Z0-9_-]{1,80}\z`.
pub fn valid_persona_id(id: &str) -> bool {
    (1..=80).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn single<'a, T>(mut items: impl Iterator<Item = &'a T>) -> Result<&'a T> {
    match (items.next(), items.next()) {
        (Some(item), None) => Ok(item),
        _ => Err(Error::invalid_operation("항목이 정확히 하나가 아닙니다.")),
    }
}

/// 원본 `AiSettingsStore`. 저장은 `.tmp`에 쓰고 기존 파일을 `.bak`으로 복사한 뒤 바꾼다.
pub struct AiSettingsStore {
    path: PathBuf,
    sync: Mutex<()>,
}

impl AiSettingsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            sync: Mutex::new(()),
        }
    }
    pub fn default_path() -> PathBuf {
        crate::paths::app_data().join("ai-settings.json")
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<AiSettings> {
        let _guard = self.sync.lock().unwrap_or_else(|e| e.into_inner());
        if !self.path.exists() {
            let mut initial = AiSettings::default();
            initial.ensure_personas();
            return Ok(initial);
        }
        let text = std::fs::read_to_string(&self.path)?;
        let value: Value = serde_json::from_str(&text)?;
        if value.is_null() {
            return Err(Error::invalid_data("AI 설정을 읽을 수 없습니다."));
        }
        let mut settings = AiSettings::from_value(value)?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn save(&self, settings: &mut AiSettings) -> Result<()> {
        settings.validate()?;
        let _guard = self.sync.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path_with_suffix(&self.path, ".tmp");
        std::fs::write(&temp, json::indented(settings))?;
        if self.path.exists() {
            std::fs::copy(&self.path, path_with_suffix(&self.path, ".bak"))?;
        }
        std::fs::rename(&temp, &self.path)?;
        Ok(())
    }
}

/// `path + suffix`(확장자 교체가 아니라 덧붙이기).
pub fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text = path.as_os_str().to_owned();
    text.push(suffix);
    PathBuf::from(text)
}
