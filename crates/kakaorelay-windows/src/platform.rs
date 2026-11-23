//! `kakaorelay_core::platform::Platform`의 Windows 구현.

use crate::catalog::{MESSAGE_EDITOR_ID, select_targets};
use crate::image_transport::KakaoImageTransport;
use crate::reader::LocalChatReader;
use crate::text_transport::KakaoTextTransport;
use crate::{native, process};
use kakaorelay_ai::ChatReader;
use kakaorelay_core::platform::{
    ConversationTarget, ImageTransport, Platform, RoomCatalog, TextTransport, TransportError,
};
use std::collections::HashSet;
use std::sync::Arc;
use windows::Win32::UI::WindowsAndMessaging::GetDlgCtrlID;

/// 현재 사용자 세션의 카카오톡 열린 대화방. 원본 `ConversationCatalog.Scan`.
pub struct WindowsRoomCatalog;

impl RoomCatalog for WindowsRoomCatalog {
    fn scan(&self) -> Result<Vec<ConversationTarget>, TransportError> {
        let pids = process::find_by_name(process::KAKAO_PROCESS)
            .map_err(|e| TransportError::Failed(e.message()))?;
        let session = process::current_session();
        let ids: HashSet<i32> = pids
            .into_iter()
            .filter(|&pid| session.is_some() && process::session_of(pid) == session)
            .map(|pid| pid as i32)
            .collect();
        let windows = native::enumerate(&ids);
        Ok(select_targets(&windows, |child| {
            // SAFETY: 창 ID 조회 함수다.
            native::parse(&child.handle)
                .is_ok_and(|h| unsafe { GetDlgCtrlID(h) } == MESSAGE_EDITOR_ID)
        }))
    }
}

pub struct WindowsPlatform {
    rooms: WindowsRoomCatalog,
    reader: Arc<LocalChatReader>,
}

impl WindowsPlatform {
    pub fn new() -> Self {
        Self {
            rooms: WindowsRoomCatalog,
            reader: Arc::new(LocalChatReader::new()),
        }
    }
}

impl Default for WindowsPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for WindowsPlatform {
    fn rooms(&self) -> &dyn RoomCatalog {
        &self.rooms
    }

    fn reader(&self) -> Arc<dyn ChatReader> {
        self.reader.clone()
    }

    fn text_transport(&self) -> Box<dyn TextTransport> {
        Box::new(KakaoTextTransport::new())
    }

    fn image_transport(&self) -> Box<dyn ImageTransport> {
        Box::new(KakaoImageTransport::new())
    }

    fn image_transport_name(&self) -> &'static str {
        "WM_DROPFILES + HWND"
    }
}
