//! 원본: 두 OS 리더가 각자 `ReadAttachmentAsync`에서 `ReceivedAttachments.ReadAsync`를 부르던 부분.
//! 어댑터의 `ChatReader`를 감싸 첨부(종류 2·18) 원본을 `ReceivedAttachments`로 채운다. 나머지 호출은 그대로 넘긴다.

use std::sync::Arc;

use async_trait::async_trait;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::received_attachments::ReceivedAttachments;
use kakaorelay_ai::{ApiFailure, ChatContext, ChatReader, Error, LocalMessage, LocalRoom};

pub struct AttachmentReader {
    inner: Arc<dyn ChatReader>,
    attachments: Arc<ReceivedAttachments>,
    /// `ChatReader::read_attachment`는 취소 토큰을 받지 않는다. 앱 종료 시 진행 중인 다운로드를 끊는 데 쓴다.
    lifetime: CancellationToken,
}

impl AttachmentReader {
    pub fn new(
        inner: Arc<dyn ChatReader>,
        attachments: Arc<ReceivedAttachments>,
        lifetime: CancellationToken,
    ) -> Self {
        Self {
            inner,
            attachments,
            lifetime,
        }
    }
}

#[async_trait]
impl ChatReader for AttachmentReader {
    async fn rooms(&self) -> Result<Vec<LocalRoom>, ApiFailure> {
        self.inner.rooms().await
    }

    async fn read(
        &self,
        profile: &str,
        room_id: &str,
        limit: usize,
    ) -> Result<ChatContext, ApiFailure> {
        self.inner.read(profile, room_id, limit).await
    }

    async fn is_direct_peer(&self, room: &LocalRoom, peer: &str) -> Result<bool, ApiFailure> {
        self.inner.is_direct_peer(room, peer).await
    }

    async fn read_attachment(&self, message: LocalMessage) -> Result<LocalMessage, ApiFailure> {
        // 다운로드·추출 실패는 본문 표기로 돌아온다. 밖으로 나오는 오류는 종료 취소뿐이다.
        match self.attachments.read(message, &self.lifetime).await {
            Ok(message) => Ok(message),
            Err(Error::Api(failure)) => Err(failure),
            Err(error) => Err(ApiFailure::new(503, "shutting_down", error.to_string())),
        }
    }
}
