//! KakaoRelay macos 어댑터. `kakaorelay_core::platform`의 trait을 구현한다.
#![cfg(target_os = "macos")]

mod app;
mod ax;
pub mod catalog;
pub mod db;
mod device;
mod input;
pub mod key;
mod pasteboard;
pub mod permissions;
pub mod reader;
mod transport;

use std::sync::Arc;

use kakaorelay_core::platform::{ChatReader, ImageTransport, Platform, RoomCatalog, TextTransport};

pub use app::{MacRoomCatalog, bring_to_front, frontmost_pid};
pub use device::platform_uuid;
pub use permissions::{DataAccess, PermissionStatus, permission_status};
pub use reader::MacChatReader;
pub use transport::{MacImageTransport, MacTextTransport};

/// 맥 OS 어댑터 묶음.
pub struct MacPlatform {
    catalog: MacRoomCatalog,
    reader: MacChatReader,
}

impl MacPlatform {
    /// `self_id`는 설정의 '내 사용자 ID'(`selfId`)다. DB 키 유도와 발신 메시지 확인에 쓴다.
    pub fn new(self_id: Option<u64>) -> Self {
        Self {
            catalog: MacRoomCatalog,
            reader: MacChatReader::new(self_id),
        }
    }
}

impl Platform for MacPlatform {
    fn rooms(&self) -> &dyn RoomCatalog {
        &self.catalog
    }

    fn reader(&self) -> Arc<dyn ChatReader> {
        Arc::new(self.reader.clone())
    }

    fn text_transport(&self) -> Box<dyn TextTransport> {
        Box::new(MacTextTransport::new(self.reader.store()))
    }

    fn image_transport(&self) -> Box<dyn ImageTransport> {
        Box::new(MacImageTransport::new(self.reader.store()))
    }

    fn image_transport_name(&self) -> &'static str {
        "clipboard paste + window menu"
    }

    fn detected_self_id(&self) -> Option<String> {
        self.reader.store().user_id().ok().map(|id| id.to_string())
    }
}
