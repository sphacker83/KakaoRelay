//! 원본: `AiConversation.cs`. 방별 세션 상태 `state.json`(PascalCase 기본 옵션)과 해시 규칙.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};

use crate::ai_service::AiCommand;
use crate::chat_models::LocalMessage;
use crate::error::Result;
use crate::json;
use crate::settings::{AiProviderSettings, BotPersona};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AiConversation {
    pub session_id: Option<String>,
    pub knowledge_revision: Option<String>,
    pub instructions_date: Option<NaiveDate>,
    pub instructions_revision: Option<String>,
    pub seen: IndexSet<String>,
}

impl AiConversation {
    pub fn needs_instructions(&self, day: NaiveDate, revision: &str) -> bool {
        self.session_id.as_deref().is_none_or(crate::text::is_blank)
            || self.instructions_date != Some(day)
            || self.instructions_revision.as_deref() != Some(revision)
    }

    /// 메시지 지문: 기본 옵션 직렬화(PascalCase, 비 ASCII 이스케이프)의 SHA-256.
    pub fn fingerprint(message: &LocalMessage) -> String {
        json::hash(&json::default_pascal(message))
    }

    pub fn hash(text: &str) -> String {
        json::hash(text)
    }

    /// `data/providers/<p>/conversations/<해시>`. 해시 입력은 원본 익명 객체 `{Profile, RoomId, provider, persona}`.
    pub fn folder(
        provider: &AiProviderSettings,
        persona: &BotPersona,
        command: &AiCommand,
    ) -> Result<PathBuf> {
        let hash = json::hash(&Self::identity(
            provider,
            persona,
            &command.profile,
            &command.room_id,
        ));
        crate::provider_storage::conversation_folder(&provider.id, &hash)
    }

    /// 세션 폴더 해시 입력 문자열.
    pub fn identity(
        provider: &AiProviderSettings,
        persona: &BotPersona,
        profile: &str,
        room_id: &str,
    ) -> String {
        let identity = serde_json::json!({
            "Profile": profile,
            "RoomId": room_id,
            "provider": json::pascal(json::value(provider)),
            "persona": json::pascal(json::value(persona)),
        });
        json::to_string(&identity, json::Escape::Default, false)
    }

    pub fn load(folder: &Path, knowledge_revision: Option<&str>) -> AiConversation {
        let path = folder.join("state.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return AiConversation::default();
        };
        match serde_json::from_str::<Option<AiConversation>>(&text) {
            Ok(Some(state)) => match knowledge_revision {
                Some(revision) if state.knowledge_revision.as_deref() != Some(revision) => {
                    AiConversation::default()
                }
                _ => state,
            },
            _ => AiConversation::default(),
        }
    }

    pub fn save(&self, folder: &Path) -> Result<()> {
        std::fs::create_dir_all(folder)?;
        let path = folder.join("state.json");
        let temp = crate::settings::path_with_suffix(&path, ".tmp");
        std::fs::write(&temp, json::default(self))?;
        std::fs::rename(&temp, &path)?;
        Ok(())
    }
}
