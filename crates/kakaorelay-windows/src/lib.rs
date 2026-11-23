//! KakaoRelay windows 어댑터. `kakaorelay_core::platform`의 trait을 구현한다.
//!
//! OS 비의존 로직(SQLCipher 4 복호화·WAL 반영, 메모리 키 후보 판정, 열린 방 선택 규칙)은 cfg 없이 두어 모든 OS에서
//! 테스트한다. Win32 호출은 모두 `cfg(windows)` 모듈에 있다. 실기 검증 절차는 `VERIFY.md`를 따른다.

pub mod catalog;
pub mod cipher;
pub mod memory_keys;

#[cfg(windows)]
pub mod diagnostics;
#[cfg(windows)]
mod image_transport;
#[cfg(windows)]
pub mod instance;
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod platform;
#[cfg(windows)]
mod process;
#[cfg(windows)]
mod reader;
#[cfg(windows)]
mod sqlite;
#[cfg(windows)]
mod text_transport;

#[cfg(windows)]
pub use image_transport::KakaoImageTransport;
#[cfg(windows)]
pub use platform::{WindowsPlatform, WindowsRoomCatalog};
#[cfg(windows)]
pub use reader::LocalChatReader;
#[cfg(windows)]
pub use text_transport::KakaoTextTransport;
