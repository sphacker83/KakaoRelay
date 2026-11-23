//! 원본: `src/KakaoRelay.AI/ChatModels.cs`. 모든 OS 어댑터가 이 타입으로 대화를 반환한다.

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::attachment::AgentAttachment;

/// HTTP 상태 코드와 오류 코드를 가진 실패. API 응답 `{code, detail}`로 그대로 변환된다.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{detail}")]
pub struct ApiFailure {
    pub status_code: u16,
    pub code: String,
    pub detail: String,
}

impl ApiFailure {
    pub fn new(status_code: u16, code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            status_code,
            code: code.into(),
            detail: detail.into(),
        }
    }
}

/// 로컬 대화방. `profile`은 Windows에서 카카오톡 사용자 폴더(40hex), 맥에서는 활성 DB 식별자다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRoom {
    pub profile: String,
    pub id: String,
    pub title: String,
    pub readable: bool,
    /// 1:1 방의 상대 사용자 ID. 제어방 상대 검증에 쓴다.
    #[serde(default)]
    pub direct_member_id: Option<String>,
}

impl LocalRoom {
    pub fn new(
        profile: impl Into<String>,
        id: impl Into<String>,
        title: impl Into<String>,
        readable: bool,
    ) -> Self {
        Self {
            profile: profile.into(),
            id: id.into(),
            title: title.into(),
            readable,
            direct_member_id: None,
        }
    }

    /// C# `Display` 계산 속성. API 응답에도 직렬화되므로 직렬화 시 함께 쓴다(inventory §3-6 #4).
    pub fn display(&self) -> String {
        format!(
            "{} · {}",
            self.title,
            if self.readable {
                "읽기 가능"
            } else {
                "카카오톡에서 방을 열어주세요"
            }
        )
    }
}

/// 로컬 메시지. `attachment_metadata`/`attachment`는 C#에서 `JsonIgnore`라 직렬화하지 않는다.
/// `time`은 .NET `DateTimeOffset` 형식으로 쓴다(메시지 지문 해시가 이 문자열에 달려 있다).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMessage {
    pub id: String,
    pub author_id: String,
    pub author: String,
    #[serde(with = "crate::json::time")]
    pub time: DateTime<FixedOffset>,
    /// 카카오톡 메시지 type 값(1 텍스트, 2 사진, 18 파일 등). OS별 값 차이는 어댑터가 맞춘다.
    #[serde(rename = "type")]
    pub kind: i32,
    pub text: String,
    pub deleted: bool,
    /// 어댑터가 채우는 카카오 첨부 메타데이터 JSON(`name`, `size`/`s`, `cs`, `url`).
    #[serde(skip)]
    pub attachment_metadata: Option<String>,
    /// `ChatReader::read_attachment`가 원본을 받아 둔 경우의 에이전트 입력용 파일.
    #[serde(skip)]
    pub attachment: Option<AgentAttachment>,
}

impl LocalMessage {
    pub fn new(
        id: impl Into<String>,
        author_id: impl Into<String>,
        author: impl Into<String>,
        time: DateTime<FixedOffset>,
        kind: i32,
        text: impl Into<String>,
        deleted: bool,
    ) -> Self {
        Self {
            id: id.into(),
            author_id: author_id.into(),
            author: author.into(),
            time,
            kind,
            text: text.into(),
            deleted,
            attachment_metadata: None,
            attachment: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatContext {
    pub room: LocalRoom,
    pub messages: Vec<LocalMessage>,
    #[serde(with = "crate::json::time")]
    pub read_at: DateTime<FixedOffset>,
}

/// 원본 `IChatReader`. OS 어댑터(Windows 메모리 키 SQLCipher 4, 맥 compat 3)와 가져온 대화 리더가 구현한다.
/// 실패는 `ApiFailure`(409 `local_unavailable` 등)로 반환해 API 계약을 유지한다.
#[async_trait]
pub trait ChatReader: Send + Sync {
    async fn rooms(&self) -> Result<Vec<LocalRoom>, ApiFailure>;
    async fn read(
        &self,
        profile: &str,
        room_id: &str,
        limit: usize,
    ) -> Result<ChatContext, ApiFailure>;

    async fn is_direct_peer(&self, room: &LocalRoom, peer: &str) -> Result<bool, ApiFailure> {
        Ok(self.rooms().await?.iter().any(|r| {
            r.profile == room.profile
                && r.id == room.id
                && r.direct_member_id.as_deref() == Some(peer)
        }))
    }

    /// 첨부 메시지(type 2·18)의 원본을 받아 `attachment`나 본문(`[첨부 본문 …]`)을 채운다.
    /// 기본은 변경 없음이며, 실제 어댑터는 `ReceivedAttachments::read`에 위임한다. 실패는 본문 표기로 돌려준다.
    async fn read_attachment(&self, message: LocalMessage) -> Result<LocalMessage, ApiFailure> {
        Ok(message)
    }
}
