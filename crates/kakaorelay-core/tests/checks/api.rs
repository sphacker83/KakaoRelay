//! 원본: `ApiChecks.cs`. 실제 HTTP 서버를 127.0.0.1에 띄워 인증·오류 본문·멱등을 확인한다.

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use kakaorelay_ai::ai_service::AiService;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::cli_runner::AiRunner;
use kakaorelay_ai::imported_reader::ImportedChatReader;
use kakaorelay_ai::settings::{AiProviderSettings, AiSettings, AiSettingsStore};
use kakaorelay_ai::{ChatReader, Error, Result, json};
use kakaorelay_core::api_send::{ApiSendCommand, ApiSendService, ImageFactory, Scan, TextFactory};
use kakaorelay_core::platform::{ConversationTarget, SendTarget, TextTransport, TransportError};
use kakaorelay_core::relay_api::{RelayApi, RelayApiConfig, SendHandler};
use kakaorelay_core::test_send::{self, TestSendRequest, read_history, receipt_path};
use serde_json::{Value, json};

use crate::support::{Check, FakeImage, Http, PNG, Shared, in_minutes, parse, shared};

struct FakeRunner;

#[async_trait]
impl AiRunner for FakeRunner {
    async fn run(
        &self,
        _: &AiProviderSettings,
        _: &str,
        _: i32,
        _: &CancellationToken,
    ) -> Result<String> {
        Ok("회의는 금요일입니다.".into())
    }
}

/// 원본 `ApiChecks.FakeTransport`. 빈 안내 문구가 있는 입력창에서 시작한다.
struct Fake {
    posts: usize,
    draft: String,
    before_write: Option<Box<dyn FnMut() + Send>>,
    order: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn new(order: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            posts: 0,
            draft: "메시지 입력".into(),
            before_write: None,
            order,
        }
    }
}

impl TextTransport for Fake {
    fn foreground_window(&self) -> String {
        "0x1".into()
    }
    fn validate_target(&mut self, _: &dyn SendTarget) -> std::result::Result<(), TransportError> {
        Ok(())
    }
    fn read_draft(&mut self) -> std::result::Result<Option<String>, TransportError> {
        Ok(Some(self.draft.clone()))
    }
    fn write_draft(&mut self, value: &str) -> std::result::Result<(), TransportError> {
        if let Some(hook) = &mut self.before_write {
            hook();
        }
        self.draft = value.into();
        Ok(())
    }
    fn post_enter(&mut self) -> std::result::Result<(), TransportError> {
        self.posts += 1;
        self.order.lock().unwrap().push(self.draft.clone());
        self.draft.clear();
        Ok(())
    }
    fn confirm_after_enter(
        &mut self,
        t: &dyn SendTarget,
    ) -> std::result::Result<bool, TransportError> {
        Ok(t.is_empty_draft(&self.draft))
    }
}

/// 원본 `DelayedTransport`. Enter 전에 입력 내용을 읽으면 실패로 센다.
#[derive(Default)]
struct Delayed {
    writes: usize,
    posts: usize,
    early_reads: usize,
    configured_delay: Option<u32>,
    input_failure: Option<TransportError>,
    change_target: bool,
    written_at: Option<Instant>,
    elapsed_at_post: u128,
}

impl Delayed {
    fn failing(error: TransportError) -> Self {
        Self {
            input_failure: Some(error),
            ..Default::default()
        }
    }
}

impl TextTransport for Delayed {
    fn foreground_window(&self) -> String {
        "0x1".into()
    }
    fn validate_target(
        &mut self,
        request: &dyn SendTarget,
    ) -> std::result::Result<(), TransportError> {
        self.configured_delay = request.send_delay_ms();
        if self.writes > 0 && self.change_target {
            return Err(TransportError::Failed("Target changed during delay".into()));
        }
        Ok(())
    }
    fn read_draft(&mut self) -> std::result::Result<Option<String>, TransportError> {
        if self.writes > 0 && self.posts == 0 {
            self.early_reads += 1;
            return Err(TransportError::Failed(
                "Inserted text must not be read before Enter".into(),
            ));
        }
        Ok(Some(String::new()))
    }
    fn write_draft(&mut self, _: &str) -> std::result::Result<(), TransportError> {
        self.writes += 1;
        self.written_at = Some(Instant::now());
        match self.input_failure.take() {
            Some(TransportError::InputTimeout(m)) => {
                self.input_failure = Some(TransportError::InputTimeout(m.clone()));
                Err(TransportError::InputTimeout(m))
            }
            Some(other) => Err(other),
            None => Ok(()),
        }
    }
    fn post_enter(&mut self) -> std::result::Result<(), TransportError> {
        self.posts += 1;
        self.elapsed_at_post = self.written_at.map_or(0, |t| t.elapsed().as_millis());
        Ok(())
    }
    fn confirm_after_enter(
        &mut self,
        t: &dyn SendTarget,
    ) -> std::result::Result<bool, TransportError> {
        Ok(t.is_empty_draft(&self.read_draft()?.unwrap_or_default()))
    }
}

fn rooms_of(list: &Arc<Mutex<Vec<ConversationTarget>>>) -> Scan {
    let list = list.clone();
    Arc::new(move || Ok(list.lock().unwrap().clone()))
}

fn room() -> ConversationTarget {
    ConversationTarget {
        title: "시험방".into(),
        process_id: 1,
        handle: "0x1".into(),
        ambiguous: false,
        room_id: None,
    }
}

fn handler(service: Arc<ApiSendService>) -> SendHandler {
    Arc::new(move |command| Box::pin(service.send(command)))
}

fn body(value: Value) -> String {
    value.to_string()
}

pub async fn run(root: &Path, check: &Check) {
    delayed_text(root, check).await;
    queue(root, check).await;
    let ledger = root.join("api-ledger");
    let order = Arc::new(Mutex::new(vec![]));
    let fake = shared(Fake::new(order));
    let rooms = Arc::new(Mutex::new(vec![room()]));
    let image = shared(FakeImage::default());
    let (fake_factory, image_factory) = (fake.clone(), image.clone());
    let service = Arc::new(ApiSendService::new(
        &ledger,
        rooms_of(&rooms),
        Arc::new(move || Box::new(Shared(fake_factory.clone())) as Box<dyn TextTransport>)
            as TextFactory,
        Some(Arc::new(move || {
            Box::new(Shared(image_factory.clone()))
                as Box<dyn kakaorelay_core::platform::ImageTransport>
        }) as ImageFactory),
        Some(root.join("api-image-transfers")),
    ));
    let path = root.join("connection.json");
    let history_ledger = ledger.clone();
    let mut config = RelayApiConfig::new(
        &path,
        rooms_of(&rooms),
        Arc::new(move || read_history(&history_ledger)),
        handler(service.clone()),
    );
    config.image_transport = "fixture transport".into();
    let api = RelayApi::start(config).await.unwrap();
    let mut client = Http::new(&api.connection().base_url);
    check.check(
        client.get("/v1/rooms").await.0 == 401,
        "API rejects unauthenticated requests",
    );
    client.token = Some(api.connection().token.clone());
    client.origin = true;
    check.check(
        client.get("/v1/rooms").await.0 == 401,
        "API rejects browser-origin requests even with a token",
    );
    client.origin = false;
    let image_path = root.join("api-image.png");
    std::fs::write(&image_path, PNG).unwrap();
    let image_command = json!({"requestId": "api-image-once", "recipient": "시험방", "imagePath": image_path.to_string_lossy()});
    let (status, text) = client
        .post("/v1/send-image", &body(image_command.clone()))
        .await;
    let receipt = parse(&text);
    check.check(
        status == 200
            && receipt["kind"] == "image"
            && receipt["attachmentQueued"] == true
            && receipt["enterPosted"] == true
            && image.lock().unwrap().attachments == 1
            && fake.lock().unwrap().posts == 0,
        "Image HTTP endpoint routes a local file through the image adapter only",
    );
    std::fs::remove_file(&image_path).unwrap();
    let duplicate = client
        .post("/v1/send-image", &body(image_command.clone()))
        .await;
    check.check(
        duplicate.0 == 200 && image.lock().unwrap().attachments == 1,
        "Image API retry returns receipt even after the source file is gone",
    );
    let mut wrong = image_command.clone();
    wrong["imagePath"] = json!(root.join("different.png").to_string_lossy());
    check.check(
        client.post("/v1/send-image", &body(wrong)).await.0 == 409
            && image.lock().unwrap().attachments == 1,
        "Image request ID cannot be reused for another file",
    );
    let missing = client
        .post(
            "/v1/send-image",
            &body(json!({"requestId": "image-empty", "recipient": "시험방"})),
        )
        .await;
    let relative = client
        .post("/v1/send-image", &body(json!({"requestId": "image-relative", "recipient": "시험방", "imagePath": "relative.png"})))
        .await;
    check.check(
        missing.0 == 400 && relative.0 == 400 && image.lock().unwrap().attachments == 1,
        "Image API rejects missing or relative image sources before dispatch",
    );
    let (_, found) = client.get("/v1/rooms").await;
    let found = parse(&found);
    check.check(
        found.as_array().is_some_and(|a| {
            a.len() == 1 && a[0]["title"] == "시험방" && a[0]["selectable"] == true
        }),
        "API returns available rooms as JSON",
    );
    check.check(
        client.get("/v1/rooms").await.1.contains("시험방"),
        "HTTP JSON responses contain literal Korean instead of Unicode escapes",
    );
    check.check(
        json::indented(&json!({"text": "한글 발송 이력"})).contains("한글 발송 이력"),
        "Reports and send history share readable Korean JSON encoding",
    );
    let malformed = client.post("/v1/send", "{bad").await;
    check.check(
        malformed.0 == 400
            && parse(&malformed.1) == json!({"code": "invalid_request"})
            && fake.lock().unwrap().posts == 0,
        "Malformed API JSON never dispatches",
    );
    let invalid = client
        .post(
            "/v1/send",
            &body(json!({"requestId": "../bad", "recipient": "시험방", "message": "hi"})),
        )
        .await;
    check.check(
        invalid.0 == 400 && fake.lock().unwrap().posts == 0,
        "API validates IDs before filesystem or native access",
    );
    for delay in [-1, 30001] {
        let response = client
            .post("/v1/send", &body(json!({"requestId": "invalid-delay", "recipient": "시험방", "message": "hi", "sendDelayMs": delay})))
            .await;
        check.check(
            response.0 == 400
                && parse(&response.1)["code"] == "invalid_send_delay"
                && fake.lock().unwrap().posts == 0,
            "HTTP rejects invalid send delays before touching the editor",
        );
    }
    let long = format!("{}\r\n끝", "가".repeat(12000));
    let command = json!({"requestId": "long-message", "recipient": "시험방", "message": long});
    let (status, text) = client.post("/v1/send", &body(command.clone())).await;
    let receipt = parse(&text);
    check.check(
        status == 200 && receipt["enterPosted"] == true && fake.lock().unwrap().posts == 1,
        "API accepts long Korean messages and empty cue without a checkbox",
    );
    check.check(
        receipt["sendDelayMs"] == 1000 && receipt["inputResponseTimedOut"] == false,
        "API defaults to a one-second delay and records the effective delay",
    );
    rooms.lock().unwrap().clear();
    check.check(
        client.post("/v1/send", &body(command.clone())).await.0 == 200
            && fake.lock().unwrap().posts == 1,
        "Duplicate API request returns stored result even after room closes",
    );
    let mut changed = command.clone();
    changed["message"] = json!("changed");
    let conflict = client.post("/v1/send", &body(changed)).await;
    check.check(
        conflict.0 == 409
            && parse(&conflict.1)["code"] == "request_id_conflict"
            && fake.lock().unwrap().posts == 1,
        "Changed content cannot reuse an API request ID",
    );
    let result = parse(&client.get("/v1/requests/long-message").await.1);
    check.check(
        result["status"] == "needs-review",
        "API distinguishes queued Enter from confirmed delivery",
    );
    check.check(
        files(&ledger).iter().all(|f| {
            !std::fs::read_to_string(f)
                .unwrap_or_default()
                .contains(&"가".repeat(50))
        }),
        "API reservations and receipts do not persist message bodies",
    );
    std::fs::remove_file(receipt_path(&ledger, "long-message")).unwrap();
    let interrupted = client.post("/v1/send", &body(command.clone())).await;
    check.check(
        interrupted.0 == 409
            && parse(&interrupted.1)["code"] == "uncertain"
            && fake.lock().unwrap().posts == 1,
        "Interrupted reserved request is never automatically replayed",
    );
    let missing = client
        .post(
            "/v1/send",
            &body(json!({"requestId": "missing", "recipient": "시험방", "message": "hello"})),
        )
        .await;
    check.check(
        missing.0 == 409
            && parse(&missing.1)["code"] == "room_unavailable"
            && fake.lock().unwrap().posts == 1,
        "Missing recipient never dispatches",
    );
    let capabilities = parse(&client.get("/v1/capabilities").await.1);
    check.check(
        capabilities["maxMessageCharacters"].is_null()
            && capabilities["recentMessages"]["supported"] == false
            && capabilities["imageSend"]["transport"] == "fixture transport",
        "Capabilities declare no app character cap, unsupported recent-message reading and the platform image transport",
    );
    api.stop().await;
    check.check(
        !path.exists(),
        "API removes its discovery credentials on shutdown",
    );

    let reader = Arc::new(ImportedChatReader::new());
    let local = reader
        .import(
            "api.txt",
            "--------------- 2026년 9월 28일 ---------------\n[테스트] [오후 1:00] 회의는 금요일입니다.",
        )
        .unwrap();
    let ai = Arc::new(AiService::new(
        reader.clone() as Arc<dyn ChatReader>,
        Arc::new(AiSettingsStore::new(root.join("api-ai-settings.json"))),
        Arc::new(FakeRunner),
        None,
        None,
    ));
    let mut config = RelayApiConfig::new(
        &path,
        rooms_of(&rooms),
        Arc::new(|| Ok(vec![])),
        handler(service.clone()),
    );
    config.ai = Some(ai.clone());
    let api = RelayApi::start(config).await.unwrap();
    let mut client = Http::new(&api.connection().base_url);
    check.check(
        client.get("/v1/local/rooms").await.0 == 401,
        "Local conversation API requires the same bearer protection",
    );
    check.check(
        client.get("/v1/ai/delegations").await.0 == 401,
        "Background task history requires authentication",
    );
    client.token = Some(api.connection().token.clone());
    let capabilities = parse(&client.get("/v1/capabilities").await.1);
    check.check(
        capabilities["recentMessages"]["supported"] == true && capabilities["aiChat"] == true,
        "Configured API advertises local reading and AI chat",
    );
    let (_, text) = client
        .get(&format!(
            "/v1/local/messages?profile=import&roomId={}&limit=10",
            local.id
        ))
        .await;
    let context = parse(&text);
    check.check(
        context["messages"]
            .as_array()
            .is_some_and(|m| m.len() == 1 && m[0]["text"] == "회의는 금요일입니다.")
            && context["room"]["display"].is_string(),
        "Local messages endpoint returns actual context without dispatching",
    );
    check.check(
        client
            .get(&format!(
                "/v1/local/messages?profile=import&roomId={}&limit=501",
                local.id
            ))
            .await
            .0
            == 400,
        "Message limit validation is exposed through HTTP",
    );
    let mut settings =
        AiSettings::from_value(parse(&client.get("/v1/ai/settings").await.1)).unwrap();
    settings.persona_mut().name = "API 봇".into();
    settings.grok_delegation_enabled = true;
    let saved = client
        .put("/v1/ai/settings", &json::value(&settings).to_string())
        .await;
    check.check(
        saved.0 == 200 && ai.settings().load().unwrap().persona().name == "API 봇",
        "AI settings endpoint persists persona changes",
    );
    let posts = fake.lock().unwrap().posts;
    let (status, text) = client
        .post(
            "/v1/ai/generate",
            &body(json!({"profile": local.profile, "roomId": local.id, "mode": "reply"})),
        )
        .await;
    let answer = parse(&text);
    check.check(
        status == 200
            && answer["provider"] == "codex"
            && answer["delegation"].is_null()
            && fake.lock().unwrap().posts == posts,
        "AI generation returns a draft without sending or launching background delegation",
    );
    check.check(
        client.get("/v1/ai/delegations").await.0 == 200,
        "Authorized clients can read background task history",
    );
    settings.provider = "bogus".into();
    check.check(
        client
            .put("/v1/ai/settings", &json::value(&settings).to_string())
            .await
            .0
            == 400,
        "Invalid provider cannot overwrite saved settings",
    );
    api.stop().await;
}

fn files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut result = vec![];
    for entry in std::fs::read_dir(root).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            result.extend(files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            result.push(path);
        }
    }
    result
}

async fn delayed_text(root: &Path, check: &Check) {
    let ledger = root.join("delayed-api-ledger");
    let current = shared(shared(Delayed::default()));
    let factory = current.clone();
    let service = ApiSendService::new(
        &ledger,
        Arc::new(|| Ok(vec![room()])),
        Arc::new(move || {
            Box::new(Shared(factory.lock().unwrap().clone())) as Box<dyn TextTransport>
        }),
        None,
        None,
    );
    let swap = |value: Delayed| {
        let fresh = shared(value);
        *current.lock().unwrap() = fresh.clone();
        fresh
    };
    let fake = swap(Delayed::default());
    let mut command = ApiSendCommand::new("delayed-normal", "시험방", "fixture");
    command.send_delay_ms = 70;
    let receipt = service.send(command.clone()).await.unwrap();
    {
        let f = fake.lock().unwrap();
        check.check(
            receipt.enter_posted
                && f.posts == 1
                && f.early_reads == 0
                && f.configured_delay == Some(70)
                && f.elapsed_at_post >= 60,
            "API text waits for its configured delay then posts once without reading inserted text",
        );
    }
    let zero = ApiSendCommand {
        send_delay_ms: 0,
        ..command.clone()
    };
    service.send(zero).await.unwrap();
    check.check(
        fake.lock().unwrap().posts == 1 && fake.lock().unwrap().writes == 1,
        "Changing only the delay never replays an existing request ID",
    );
    let fake = swap(Delayed::failing(TransportError::InputTimeout(
        "1460".into(),
    )));
    let timeout = ApiSendCommand {
        request_id: "delayed-timeout".into(),
        ..command.clone()
    };
    let receipt = service.send(timeout.clone()).await.unwrap();
    service.send(timeout).await.unwrap();
    {
        let f = fake.lock().unwrap();
        check.check(
            receipt.enter_posted
                && receipt.input_response_timed_out
                && f.posts == 1
                && f.writes == 1
                && f.early_reads == 0
                && f.elapsed_at_post >= 60,
            "Text insertion timeout waits then posts once in the same request without retyping or read-back",
        );
    }
    for failure in [
        TransportError::PermissionDenied {
            code: "accessibility_denied".into(),
            detail: "권한 없음".into(),
        },
        TransportError::Failed("selection timeout".into()),
        TransportError::Failed("editor closed".into()),
    ] {
        let fake = swap(Delayed::failing(failure));
        let receipt = service
            .send(ApiSendCommand {
                request_id: uuid::Uuid::new_v4().simple().to_string(),
                send_delay_ms: 0,
                ..command.clone()
            })
            .await
            .unwrap();
        check.check(
            !receipt.enter_posted
                && !receipt.input_response_timed_out
                && fake.lock().unwrap().posts == 0,
            "Access, selection timeout and editor failures still prevent Enter",
        );
    }
    let fake = swap(Delayed {
        change_target: true,
        ..Default::default()
    });
    let receipt = service
        .send(ApiSendCommand {
            request_id: "delayed-target-change".into(),
            ..command.clone()
        })
        .await
        .unwrap();
    check.check(
        !receipt.enter_posted && fake.lock().unwrap().posts == 0,
        "A target change during the delay prevents Enter",
    );
    let fake = swap(Delayed::default());
    let receipt = service
        .send(ApiSendCommand {
            request_id: "delayed-zero".into(),
            send_delay_ms: 0,
            ..command.clone()
        })
        .await
        .unwrap();
    check.check(
        receipt.enter_posted
            && fake.lock().unwrap().configured_delay == Some(0)
            && fake.lock().unwrap().early_reads == 0,
        "An explicit zero delay keeps API read-back disabled",
    );
    let mut manual = Delayed::failing(TransportError::InputTimeout("1460".into()));
    let request =
        TestSendRequest::new("manual-timeout", "fixture", "text", 1, "0x1", in_minutes(5));
    let receipt = test_send::send_once(&request, &mut manual, &ledger).unwrap();
    check.check(
        !receipt.enter_posted && manual.posts == 0,
        "Manual sends retain their previous stop-on-timeout behavior",
    );
}

async fn queue(root: &Path, check: &Check) {
    let order = Arc::new(Mutex::new(vec![]));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let mut entered = Some(entered_tx);
    let gate = release.clone();
    let mut fake = Fake::new(order.clone());
    fake.before_write = Some(Box::new(move || {
        if let Some(tx) = entered.take() {
            let _ = tx.send(());
        }
        let (lock, condvar) = &*gate;
        let guard = lock.lock().unwrap();
        let _ = condvar
            .wait_timeout_while(guard, std::time::Duration::from_secs(10), |released| {
                !*released
            })
            .unwrap();
    }));
    let fake = shared(fake);
    let image = shared(FakeImage::default());
    let image_order = order.clone();
    let (fake_factory, image_factory) = (fake.clone(), image.clone());
    struct Ordered(Shared<FakeImage>, Arc<Mutex<Vec<String>>>);
    impl kakaorelay_core::platform::ImageTransport for Ordered {
        fn validate(&mut self, t: &dyn SendTarget) -> std::result::Result<(), TransportError> {
            self.0.validate(t)
        }
        fn attach(&mut self, p: &Path) -> std::result::Result<(), TransportError> {
            self.1.lock().unwrap().push("image".into());
            self.0.attach(p)
        }
        fn confirm_preview(&mut self) -> std::result::Result<bool, TransportError> {
            self.0.confirm_preview()
        }
    }
    let service = ApiSendService::new(
        root.join("queue-ledger"),
        Arc::new(|| Ok(vec![room()])),
        Arc::new(move || Box::new(Shared(fake_factory.clone())) as Box<dyn TextTransport>),
        Some(Arc::new(move || {
            Box::new(Ordered(Shared(image_factory.clone()), image_order.clone())) as _
        })),
        Some(root.join("queue-images")),
    );
    let image_path = root.join("queue.png");
    std::fs::write(&image_path, PNG).unwrap();
    let command = ApiSendCommand::new("queue-first", "시험방", "first");
    let first = tokio::spawn(service.send(command.clone()));
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx).await;
    let duplicate = tokio::spawn(service.send(command.clone()));
    let conflict = tokio::spawn(service.send(ApiSendCommand {
        message: "changed".into(),
        ..command.clone()
    }));
    let mut attachment = ApiSendCommand::new("queue-image", "시험방", "");
    attachment.image_path = Some(image_path.to_string_lossy().into_owned());
    let attachment = tokio::spawn(service.send(attachment));
    let missing =
        tokio::spawn(service.send(ApiSendCommand::new("queue-missing", "닫힌방", "missing")));
    let last = tokio::spawn(service.send(ApiSendCommand::new("queue-last", "시험방", "last")));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    check.check(
        [&first, &duplicate, &conflict, &attachment, &missing, &last]
            .iter()
            .all(|t| !t.is_finished()),
        "Concurrent sends wait for the active native send instead of returning busy",
    );
    {
        let (lock, condvar) = &*release;
        *lock.lock().unwrap() = true;
        condvar.notify_all();
    }
    let code = |r: Result<kakaorelay_core::test_send::TestSendReceipt>| match r {
        Err(Error::Api(f)) => f.code,
        _ => String::new(),
    };
    let first = first.await.unwrap();
    let duplicate = duplicate.await.unwrap();
    let attachment = attachment.await.unwrap();
    let last = last.await.unwrap();
    check.check(
        code(conflict.await.unwrap()) == "request_id_conflict"
            && code(missing.await.unwrap()) == "room_unavailable",
        "Queued conflicts and unavailable rooms preserve their errors",
    );
    check.check(
        first.is_ok()
            && duplicate.is_ok()
            && attachment.is_ok()
            && last.is_ok()
            && *order.lock().unwrap() == ["first", "image", "last"]
            && fake.lock().unwrap().posts == 2
            && image.lock().unwrap().attachments == 1,
        "Text and images share FIFO order, duplicate requests send once, and failures do not block later sends",
    );
}
