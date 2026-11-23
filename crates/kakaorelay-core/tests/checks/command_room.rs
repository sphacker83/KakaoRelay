//! 원본: `CommandRoomChecks.cs`(36개, ReplyRuntime·runtime API 포함).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::chat_image::{ChatImageGenerator, Progress};
use kakaorelay_ai::cli_runner::AiRunner;
use kakaorelay_ai::relay_command::{self, RelayCommand};
use kakaorelay_ai::settings::{AiProviderSettings, AiSettings, AiSettingsStore, room_key};
use kakaorelay_ai::{ApiFailure, ChatContext, ChatReader, LocalMessage, LocalRoom, Result};
use kakaorelay_core::api_send::ApiSendCommand;
use kakaorelay_core::command_room::{
    CommandRoomService, CommandRoomSettings, CommandRoomStore, Progress as Track, policy,
};
use kakaorelay_core::platform::ConversationTarget;
use kakaorelay_core::relay_api::{RelayApi, RelayApiConfig};
use kakaorelay_core::reply_runtime::ReplyRuntime;
use kakaorelay_core::test_send::TestSendReceipt;

use crate::support::{Check, Http};

struct Reader {
    rooms: Mutex<Vec<LocalRoom>>,
    messages: Mutex<Vec<LocalMessage>>,
}

#[async_trait]
impl ChatReader for Reader {
    async fn rooms(&self) -> std::result::Result<Vec<LocalRoom>, ApiFailure> {
        Ok(self.rooms.lock().unwrap().clone())
    }
    async fn read(
        &self,
        _: &str,
        id: &str,
        limit: usize,
    ) -> std::result::Result<ChatContext, ApiFailure> {
        let rooms = self.rooms.lock().unwrap();
        let matches: Vec<_> = rooms.iter().filter(|r| r.id == id).collect();
        assert_eq!(matches.len(), 1, "원본 Single");
        let messages = self.messages.lock().unwrap();
        let start = messages.len().saturating_sub(limit);
        Ok(ChatContext {
            room: matches[0].clone(),
            messages: messages[start..].to_vec(),
            read_at: chrono::Utc::now().fixed_offset(),
        })
    }
}

struct Runner {
    command: Mutex<RelayCommand>,
    calls: AtomicUsize,
    chats: AtomicUsize,
    last_chat: Mutex<String>,
    reply: Mutex<String>,
}

#[async_trait]
impl AiRunner for Runner {
    async fn run(
        &self,
        _: &AiProviderSettings,
        prompt: &str,
        _: i32,
        _: &CancellationToken,
    ) -> Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !prompt.contains("릴레이 관리 명령을 JSON 하나로 분류한다") {
            self.chats.fetch_add(1, Ordering::SeqCst);
            *self.last_chat.lock().unwrap() = prompt.into();
            return Ok(self.reply.lock().unwrap().clone());
        }
        let c = self.command.lock().unwrap().clone();
        Ok(
            serde_json::json!({"action": c.action, "target": c.target, "value": c.value})
                .to_string(),
        )
    }
}

struct Images {
    path: PathBuf,
    calls: AtomicUsize,
}

#[async_trait]
impl ChatImageGenerator for Images {
    async fn generate(
        &self,
        _: &AiSettings,
        _: &str,
        _: &str,
        _: &CancellationToken,
        _: Progress<'_>,
    ) -> Result<PathBuf> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.path.clone())
    }
}

fn cmd(action: &str, target: &str, value: &str) -> RelayCommand {
    RelayCommand::new(action, target, value)
}

pub async fn run(root: &Path, check: &Check) {
    let mut manager = LocalRoom::new("p", "1", "관리", true);
    manager.direct_member_id = Some("2".into());
    let reader = Arc::new(Reader {
        rooms: Mutex::new(vec![
            manager.clone(),
            LocalRoom::new("p", "2", "작업", true),
            LocalRoom::new("p", "3", "다른방", true),
        ]),
        messages: Mutex::new(vec![]),
    });
    let runner = Arc::new(Runner {
        command: Mutex::new(cmd("none", "", "")),
        calls: AtomicUsize::new(0),
        chats: AtomicUsize::new(0),
        last_chat: Mutex::new(String::new()),
        reply: Mutex::new("안녕하세요. 이야기해 주세요.".into()),
    });
    let settings_store = Arc::new(AiSettingsStore::new(root.join("command-settings.json")));
    let mut initial = settings_store.load().unwrap();
    initial.self_id = "1".into();
    settings_store.save(&mut initial).unwrap();
    let settings = Arc::new(Mutex::new(initial));
    let mut config = CommandRoomSettings {
        enabled: true,
        room: Some(manager.clone()),
        operator_id: "2".into(),
        natural_language: true,
        ..Default::default()
    };
    let store = CommandRoomStore::new(Some(root.join("commands")));
    store.save(&config).unwrap();
    let images = Arc::new(Images {
        path: root.join("generated.png"),
        calls: AtomicUsize::new(0),
    });
    let ai = crate::fakes::ai_with_images(
        reader.clone(),
        settings_store.clone(),
        runner.clone(),
        images.clone(),
    );
    let sent = Arc::new(Mutex::new(Vec::<ApiSendCommand>::new()));
    let executed = Arc::new(AtomicUsize::new(0));
    let fail_send = Arc::new(AtomicBool::new(false));
    let service = || {
        let (s1, s2) = (settings.clone(), settings.clone());
        let executed = executed.clone();
        let (sent, fail_send) = (sent.clone(), fail_send.clone());
        CommandRoomService::new(
            ai.clone(),
            store.clone(),
            Arc::new(move || s1.lock().unwrap().clone()),
            Arc::new(move |plan, _| {
                executed.fetch_add(1, Ordering::SeqCst);
                *s2.lock().unwrap() = plan.next.clone();
                let reply = format!("적용: {}", plan.description);
                Box::pin(async move { Ok(reply) })
            }),
            Arc::new(|| "자동답장 0개".to_string()),
            Arc::new(move |command: ApiSendCommand, _| {
                sent.lock().unwrap().push(command);
                let receipt = TestSendReceipt {
                    enter_posted: true,
                    input_cleared: Some(!fail_send.load(Ordering::SeqCst)),
                    ..Default::default()
                };
                Box::pin(async move { Ok(receipt) })
            }),
        )
    };
    let number = AtomicUsize::new(0);
    let add = |text: &str, author: &str, kind: i32, deleted: bool| {
        let id = (number.fetch_add(1, Ordering::SeqCst) + 1).to_string();
        reader.messages.lock().unwrap().push(LocalMessage::new(
            id,
            author,
            author,
            chrono::Utc::now().fixed_offset(),
            kind,
            text,
            deleted,
        ));
    };
    let say = |text: &str| add(text, "2", 1, false);
    let none = CancellationToken::new();
    let count = || executed.load(Ordering::SeqCst);
    let sends = || sent.lock().unwrap().len();
    let last = || sent.lock().unwrap().last().unwrap().clone();
    let mode = |room: &str| settings.lock().unwrap().reply_mode_for_room("p", room);
    let name = format!("{}.json", config.generation);
    let progress = || store.read::<Track>(&name).unwrap().unwrap();
    let set_title =
        |index: usize, title: &str| reader.rooms.lock().unwrap()[index].title = title.into();

    let mut svc = service();
    say("/답변방식 즉시 작업");
    svc.tick(&none).await.unwrap();
    check.check(
        count() == 0 && sends() == 0,
        "Command activation baselines old messages without replay",
    );
    add("/답변방식 즉시 작업", "3", 1, false);
    add("/답변방식 즉시 작업", "2", 77, false);
    add("/답변방식 즉시 작업", "2", 1, true);
    add("/답변방식 즉시 작업", "1", 1, false);
    svc.tick(&none).await.unwrap();
    check.check(
        count() == 0 && sends() == 0,
        "Unauthorized, self, unsupported and deleted messages cannot issue commands",
    );
    say("/답변방식 즉시 작업");
    svc.tick(&none).await.unwrap();
    check.check(
        count() == 1 && mode("2") == "immediate",
        "Explicit authenticated command applies validated setting",
    );
    service().tick(&none).await.unwrap();
    check.check(
        count() == 1 && sends() == 1,
        "Restart and repeated polling do not replay a command",
    );
    *runner.command.lock().unwrap() = cmd("mode", "작업", "호출");
    say("작업은 부를 때만 답해");
    svc.tick(&none).await.unwrap();
    check.check(
        count() == 1 && last().message.contains("/확인"),
        "Natural language mutation previews instead of executing",
    );
    let pending = progress().pending.unwrap();
    check.check(
        pending.command.target == room_key("p", "2"),
        "Confirmation pins a room identity rather than its mutable title",
    );
    set_title(1, "바뀐이름");
    say(&format!("/확인 {}", pending.code));
    svc.tick(&none).await.unwrap();
    check.check(
        count() == 2 && mode("2") == "trigger",
        "Confirmation executes the exact prepared natural-language command",
    );
    set_title(1, "작업");
    say("작업을 바꿔");
    svc.tick(&none).await.unwrap();
    let pending = progress().pending.unwrap();
    settings.lock().unwrap().poll_seconds = 7;
    say(&format!("/확인 {}", pending.code));
    svc.tick(&none).await.unwrap();
    check.check(
        count() == 2 && last().message.contains("설정이 바뀌"),
        "Changed settings invalidate a pending confirmation",
    );
    say("작업을 바꿔");
    svc.tick(&none).await.unwrap();
    let mut track = progress();
    let code = track.pending.as_ref().unwrap().code.clone();
    track.pending.as_mut().unwrap().expires =
        chrono::Utc::now().fixed_offset() - chrono::Duration::minutes(1);
    store.write(&name, &track).unwrap();
    say(&format!("/확인 {code}"));
    svc.tick(&none).await.unwrap();
    check.check(count() == 2, "Expired confirmation cannot execute");
    config.reply = false;
    store.save(&config).unwrap();
    say("/답변방식 맥락 작업");
    svc.tick(&none).await.unwrap();
    check.check(
        count() == 2 && last().message.contains("허용하지"),
        "App-only permission denies an otherwise valid command",
    );
    config.reply = true;
    store.save(&config).unwrap();
    reader
        .rooms
        .lock()
        .unwrap()
        .push(LocalRoom::new("p", "4", "작업", true));
    say("/답변방식 맥락 작업");
    svc.tick(&none).await.unwrap();
    check.check(
        count() == 2 && last().message.contains("특정"),
        "Ambiguous room names never mutate settings",
    );
    reader.rooms.lock().unwrap().remove(3);
    reader.rooms.lock().unwrap()[0].direct_member_id = None;
    say("/답변방식 맥락 작업");
    svc.tick(&none).await.unwrap();
    check.check(
        count() == 2 && svc.status().contains("검증 실패"),
        "Changed room membership stops command execution",
    );
    reader.rooms.lock().unwrap()[0] = manager.clone();
    svc.tick(&none).await.unwrap();
    check.check(
        count() == 3,
        "Authenticated room can resume processing after identity is restored",
    );
    fail_send.store(true, Ordering::SeqCst);
    say("/답변방식 즉시 작업");
    svc.tick(&none).await.unwrap();
    let before = sends();
    svc.tick(&none).await.unwrap();
    check.check(
        count() == 4
            && sends() == before
            && svc
                .history()
                .unwrap()
                .last()
                .unwrap()
                .result
                .contains("회신 불확실"),
        "Uncertain reply is audited without replaying mutation or send",
    );
    fail_send.store(false, Ordering::SeqCst);
    say("/상태");
    svc.tick(&none).await.unwrap();
    check.check(
        last().message.contains("자동답장 0개"),
        "Control status responds even when no auto-reply room runs",
    );
    *runner.command.lock().unwrap() = cmd("none", "", "");
    let before_chat = count();
    say("오늘 뭐 하고 있어?");
    svc.tick(&none).await.unwrap();
    check.check(
        runner.chats.load(Ordering::SeqCst) == 1
            && last().recipient == "관리"
            && last().message.contains("안녕하세요")
            && count() == before_chat,
        "Ordinary control-room messages receive a persona reply without executing settings",
    );
    check.check(
        runner
            .last_chat
            .lock()
            .unwrap()
            .contains(&number.load(Ordering::SeqCst).to_string())
            && svc.history().unwrap().last().unwrap().action == "chat",
        "Conversation targets the incoming message and is audited as chat",
    );
    let chat_sends = sends();
    service().tick(&none).await.unwrap();
    check.check(
        sends() == chat_sends,
        "Restart does not replay a conversational reply",
    );
    config.natural_language = false;
    store.save(&config).unwrap();
    let calls = runner.calls.load(Ordering::SeqCst);
    say("안녕");
    svc.tick(&none).await.unwrap();
    check.check(
        runner.chats.load(Ordering::SeqCst) == 2 && runner.calls.load(Ordering::SeqCst) == calls + 1,
        "Disabling natural-language commands still permits ordinary conversation without classification",
    );
    say("/상태");
    svc.tick(&none).await.unwrap();
    check.check(
        runner.chats.load(Ordering::SeqCst) == 2 && last().message.contains("자동답장 0개"),
        "Slash commands retain priority when natural-language commands are disabled",
    );
    say("/없는명령");
    svc.tick(&none).await.unwrap();
    check.check(
        runner.chats.load(Ordering::SeqCst) == 2 && last().message.contains("명령 형식"),
        "Invalid slash commands do not fall through to ordinary conversation",
    );
    config.natural_language = true;
    store.save(&config).unwrap();
    *runner.command.lock().unwrap() = cmd("mode", "작업", "호출");
    say("작업 답변 방식 변경해");
    svc.tick(&none).await.unwrap();
    let pending_chat = progress().pending.unwrap();
    *runner.command.lock().unwrap() = cmd("none", "", "");
    say("잠깐 다른 이야기 하자");
    svc.tick(&none).await.unwrap();
    check.check(
        progress()
            .pending
            .is_some_and(|p| p.code == pending_chat.code)
            && count() == before_chat,
        "Ordinary conversation preserves a pending confirmation without applying it",
    );
    *runner.reply.lock().unwrap() =
        "고양이를 그릴게요.\n[[generate_image:\"달 위의 고양이\"]]".into();
    let image_start = sends();
    say("고양이 그려줘");
    svc.tick(&none).await.unwrap();
    {
        let list = sent.lock().unwrap();
        check.check(
            images.calls.load(Ordering::SeqCst) == 1
                && list.len() == image_start + 2
                && !list[image_start].is_image()
                && list[image_start + 1].is_image(),
            "Control-room conversation uses normal introduction then generated-image delivery without an extra command reply",
        );
    }
    let image_sent = sends();
    service().tick(&none).await.unwrap();
    check.check(
        sends() == image_sent,
        "Generated image is not replayed by another poller instance",
    );
    *runner.reply.lock().unwrap() = "첨부 확인했어요.".into();
    let chat_count = runner.chats.load(Ordering::SeqCst);
    add("사진", "2", 2, false);
    svc.tick(&none).await.unwrap();
    add("자료.pdf", "2", 18, false);
    svc.tick(&none).await.unwrap();
    check.check(
        runner.chats.load(Ordering::SeqCst) == chat_count + 2 && count() == before_chat,
        "Photos and files enter normal conversation without becoming administrative commands",
    );
    *runner.reply.lock().unwrap() = "고양이를 그릴게요.\n[[generate_image:\"고양이\"]]".into();
    fail_send.store(true, Ordering::SeqCst);
    let failed_start = sends();
    let generated_before = images.calls.load(Ordering::SeqCst);
    say("다시 그려줘");
    svc.tick(&none).await.unwrap();
    svc.tick(&none).await.unwrap();
    fail_send.store(false, Ordering::SeqCst);
    check.check(
        sends() == failed_start + 1 && images.calls.load(Ordering::SeqCst) == generated_before,
        "Uncertain introduction prevents image generation and duplicate fallback sends",
    );
    svc.stop();
    svc.drain().await;
    svc = service();
    *runner.reply.lock().unwrap() = "안녕하세요. 이야기해 주세요.".into();
    config.enabled = false;
    store.save(&config).unwrap();
    say("/답변방식 호출 작업");
    svc.tick(&none).await.unwrap();
    check.check(
        count() == 4,
        "Disabling command room stops all command processing",
    );
    let rooms = reader.rooms.lock().unwrap().clone();
    let current = settings.lock().unwrap().clone();
    let reject = |c: RelayCommand, config: &CommandRoomSettings| {
        policy::prepare(&c, &current, &rooms, config).is_err()
    };
    check.check(
        reject(cmd("shell", "", "whoami"), &config)
            && reject(cmd("knowledge-interval", "", "0"), &config)
            && reject(cmd("provider", "", "unregistered"), &config),
        "Unsupported operations, invalid intervals and providers are rejected",
    );
    check.check(
        reject(cmd("knowledge", "작업", "켜기"), &config),
        "Knowledge activation requires a preconnected vault",
    );
    config.enabled = true;
    check.check(
        reject(cmd("reply", "관리", "켜기"), &config),
        "Control room cannot become an ordinary reply room",
    );
    check.check(
        relay_command::parse("/페르소나 업무 봇 | 작업").unwrap()
            == cmd("persona", "작업", "업무 봇"),
        "Explicit persona syntax preserves names containing spaces",
    );
    let timeout = CancellationToken::new();
    let stopper = timeout.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        stopper.cancel();
    });
    settings_store
        .save(&mut settings.lock().unwrap().clone())
        .unwrap();
    let runtime = ReplyRuntime::new(
        ai.clone(),
        Arc::new(|_, _| Box::pin(async { Ok(TestSendReceipt::default()) })),
        Arc::new(|| {
            Ok(vec![
                ConversationTarget {
                    title: "작업".into(),
                    process_id: 1,
                    handle: "0x1".into(),
                    ambiguous: false,
                    room_id: None,
                },
                ConversationTarget {
                    title: "다른방".into(),
                    process_id: 1,
                    handle: "0x2".into(),
                    ambiguous: false,
                    room_id: None,
                },
            ])
        }),
    );
    runtime
        .start(&[rooms[1].clone(), rooms[2].clone()], &timeout)
        .await
        .unwrap();
    check.check(
        runtime.count() == 2,
        "Reply runtime starts independently managed rooms",
    );
    runtime.stop(Some(&rooms[1])).await;
    check.check(
        runtime.count() == 1 && runtime.running(&rooms[2]),
        "Stopping one room leaves the other room running",
    );
    runtime.stop(None).await;
    check.check(
        runtime.count() == 0,
        "Stopping all reply rooms drains their sessions",
    );
    let probe = runtime.clone();
    let mut api_config = RelayApiConfig::new(
        root.join("runtime-connection.json"),
        Arc::new(|| Ok(vec![])),
        Arc::new(|| Ok(vec![])),
        Arc::new(|_| Box::pin(async { Ok(TestSendReceipt::default()) })),
    );
    api_config.runtime = Some(Arc::new(move || {
        let count = probe.count();
        Box::pin(async move { serde_json::json!({"activeRooms": count}) })
    }));
    let api = RelayApi::start(api_config).await.unwrap();
    let mut client = Http::new(&api.connection().base_url);
    check.check(
        client.get("/v1/ai/runtime").await.0 == 401,
        "Runtime status requires gateway authentication",
    );
    client.token = Some(api.connection().token.clone());
    let (_, body) = client.get("/v1/ai/runtime").await;
    check.check(
        body.contains("\"activeRooms\":0"),
        "Runtime API reports actual stopped rooms independently of API readiness",
    );
    api.stop().await;
}
