//! Rust 전용: 앱 조립 서비스(`RelayHost`)의 발송 큐·런타임 API·종료 순서. 원본은 화면 코드라 대응 검사가 없다.

use std::path::Path;
use std::sync::{Arc, Mutex};

use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::cli_runner::AiRunner;
use kakaorelay_ai::settings::AiSettingsStore;
use kakaorelay_ai::{ChatReader, Error};
use kakaorelay_core::api_send::ApiSendCommand;
use kakaorelay_core::host::{HostPaths, RelayHost};
use kakaorelay_core::platform::{
    ConversationTarget, ImageTransport, Platform, RoomCatalog, TextTransport, TransportError,
};

use crate::support::{Check, FakeImage, FakeText, Http, Shared, shared};

struct Catalog;

impl RoomCatalog for Catalog {
    fn scan(&self) -> Result<Vec<ConversationTarget>, TransportError> {
        Ok(vec![ConversationTarget {
            title: "시험방".into(),
            process_id: 1,
            handle: "0x1".into(),
            ambiguous: false,
            room_id: None,
        }])
    }
}

struct FakePlatform {
    reader: Arc<dyn ChatReader>,
    text: Arc<Mutex<FakeText>>,
}

impl Platform for FakePlatform {
    fn rooms(&self) -> &dyn RoomCatalog {
        &Catalog
    }
    fn reader(&self) -> Arc<dyn ChatReader> {
        self.reader.clone()
    }
    fn text_transport(&self) -> Box<dyn TextTransport> {
        Box::new(Shared(self.text.clone()))
    }
    fn image_transport(&self) -> Box<dyn ImageTransport> {
        Box::new(FakeImage::default())
    }
    fn image_transport_name(&self) -> &'static str {
        "fake transport"
    }
}

struct Silent;

#[async_trait::async_trait]
impl AiRunner for Silent {
    async fn run(
        &self,
        _: &kakaorelay_ai::settings::AiProviderSettings,
        _: &str,
        _: i32,
        _: &CancellationToken,
    ) -> kakaorelay_ai::Result<String> {
        Ok(String::new())
    }
}

pub async fn run(root: &Path, check: &Check) {
    let reader: Arc<dyn ChatReader> = crate::fakes::ListReader::new(vec![]);
    let text = shared(FakeText::default());
    let platform = Arc::new(FakePlatform {
        reader: reader.clone(),
        text: text.clone(),
    });
    let store = Arc::new(AiSettingsStore::new(root.join("host-settings.json")));
    let ai = crate::fakes::ai(reader, store, Arc::new(Silent));
    let paths = HostPaths {
        ledger: root.join("ledger"),
        connection: root.join("connection.json"),
    };
    let host = RelayHost::start(platform, ai, paths.clone()).await.unwrap();
    let connection = host.connection().await.unwrap();
    let mut client = Http::new(&connection.base_url);
    client.token = Some(connection.token.clone());
    let (status, body) = client.get("/v1/ai/runtime").await;
    check.check(
        status == 200 && body.contains("\"activeRooms\":0") && body.contains("\"commandRoom\""),
        "Host runtime API reports reply rooms and command-room state",
    );
    let (status, body) = client.get("/v1/capabilities").await;
    check.check(
        status == 200 && body.contains("fake transport"),
        "Host capabilities name the platform image transport",
    );
    let none = CancellationToken::new();
    let receipt = host
        .coordinator()
        .send(ApiSendCommand::new("host-1", "시험방", "안녕"), &none)
        .await
        .unwrap();
    check.check(
        receipt.confirmed() && text.lock().unwrap().posts == 1,
        "Host send queue delivers through the platform transport",
    );
    host.shutdown().await;
    let refused = host
        .coordinator()
        .send(ApiSendCommand::new("host-2", "시험방", "안녕"), &none)
        .await;
    check.check(
        matches!(&refused, Err(Error::Api(f)) if f.status_code == 503 && f.code == "shutting_down")
            && text.lock().unwrap().posts == 1,
        "Shutdown refuses new sends with the retry-safe 503 response",
    );
    check.check(
        !paths.connection.exists() && host.connection().await.is_none(),
        "Shutdown stops the API and removes its connection file",
    );
    host.shutdown().await;
    check.check(true, "Repeated shutdown is harmless");
}
