//! 원본: `AutoReplyChecks.cs`(24개), `ReplyModeChecks.cs`의 자동답장 검사 4개, `AttachmentChecks.cs`의 후보 검사 2개,
//! `CipherChecks.cs`의 후보 검사 3개, `PersonaChecks.cs`의 호출어 후보 검사 1개.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::cli_runner::AiRunner;
use kakaorelay_ai::persona_images::PersonaAttachment;
use kakaorelay_ai::settings::{AiProviderSettings, AiSettings, AiSettingsStore, room_key};
use kakaorelay_ai::{ApiFailure, ChatContext, ChatReader, Error, LocalMessage, LocalRoom, Result};
use kakaorelay_core::api_send::{ApiSendCommand, Scan};
use kakaorelay_core::auto_reply::{
    AutoReplySession, bot_text, candidate, candidates, candidates_with_self_call,
};
use kakaorelay_core::daily_reply::DailyReplyHistory;
use kakaorelay_core::platform::ConversationTarget;
use kakaorelay_core::readiness::{open_rooms, prepare};

use crate::fakes::{self, FixedClock, confirmed, message, ready, receipt, recorder, typed, until};
use crate::support::Check;

struct ManyRoomReader {
    rooms: Vec<LocalRoom>,
    read_ids: Mutex<Vec<String>>,
}

#[async_trait]
impl ChatReader for ManyRoomReader {
    async fn rooms(&self) -> std::result::Result<Vec<LocalRoom>, ApiFailure> {
        Ok(self.rooms.clone())
    }
    async fn read(
        &self,
        profile: &str,
        room_id: &str,
        _: usize,
    ) -> std::result::Result<ChatContext, ApiFailure> {
        self.read_ids.lock().unwrap().push(room_id.into());
        let room = self
            .rooms
            .iter()
            .find(|r| r.profile == profile && r.id == room_id)
            .unwrap()
            .clone();
        Ok(ChatContext {
            room,
            messages: vec![],
            read_at: chrono::Utc::now().fixed_offset(),
        })
    }
}

/// 기준점 읽기(1개)에는 오래된 메시지만, 이후에는 호출 두 개를 돌려준다.
struct Reader {
    room: LocalRoom,
}

#[async_trait]
impl ChatReader for Reader {
    async fn rooms(&self) -> std::result::Result<Vec<LocalRoom>, ApiFailure> {
        Ok(vec![self.room.clone()])
    }
    async fn read(
        &self,
        _: &str,
        _: &str,
        limit: usize,
    ) -> std::result::Result<ChatContext, ApiFailure> {
        let messages = if limit == 1 {
            vec![message("1", "2", "old")]
        } else {
            vec![
                message("1", "2", "old"),
                message("2", "2", "@bot first"),
                message("3", "2", "@bot second"),
            ]
        };
        Ok(ChatContext {
            room: self.room.clone(),
            messages,
            read_at: chrono::Utc::now().fixed_offset(),
        })
    }
}

struct Runner(Box<dyn Fn() + Send + Sync>);

#[async_trait]
impl AiRunner for Runner {
    async fn run(
        &self,
        _: &AiProviderSettings,
        _: &str,
        _: i32,
        cancel: &CancellationToken,
    ) -> Result<String> {
        (self.0)();
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok("reply".into())
    }
}

fn target(title: &str, handle: &str) -> ConversationTarget {
    ConversationTarget {
        title: title.into(),
        process_id: 1,
        handle: handle.into(),
        ambiguous: false,
        room_id: None,
    }
}

fn scan_of(windows: Vec<ConversationTarget>) -> Scan {
    Arc::new(move || Ok(windows.clone()))
}

pub async fn run(root: &Path, check: &Check) {
    readiness(check).await;
    let room = LocalRoom::new("test-profile", "1", "테스트 방", true);
    let reader: Arc<dyn ChatReader> = Arc::new(Reader { room: room.clone() });
    let sent = Arc::new(Mutex::new(Vec::<ApiSendCommand>::new()));
    let days = root.join("auto-reply-days");
    let store = Arc::new(AiSettingsStore::new(root.join("auto-reply-settings.json")));
    let stop = CancellationToken::new();
    let sent_before_generation = Arc::new(AtomicUsize::new(usize::MAX));
    let (observed, stop_on_generate, sent_probe) =
        (sent_before_generation.clone(), stop.clone(), sent.clone());
    let service = fakes::ai(
        reader.clone(),
        store.clone(),
        Arc::new(Runner(Box::new(move || {
            observed.store(sent_probe.lock().unwrap().len(), Ordering::SeqCst);
            stop_on_generate.cancel();
        }))),
    );
    let mut settings = store.load().unwrap();
    settings.persona_mut().name = "bot".into();
    settings.poll_seconds = 1;
    store.save(&mut settings).unwrap();
    let session = Arc::new(AutoReplySession::new(
        service.clone(),
        recorder(sent.clone(), |_, _| ready(confirmed())),
        Some(DailyReplyHistory::new(&days, None)),
        None,
    ));
    let timeout = stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        timeout.cancel();
    });
    let _ = session.run(&room, "1", &stop).await;
    check.check(
        sent_before_generation.load(Ordering::SeqCst) == 0,
        "Detected calls begin model generation without sending a preparation notice",
    );
    check.check(
        sent.lock().unwrap().is_empty(),
        "Cancelling during generation sends no message to the room",
    );

    let stop = CancellationToken::new();
    let generated = Arc::new(AtomicUsize::new(0));
    let counter = generated.clone();
    let complete = fakes::ai(
        reader.clone(),
        store.clone(),
        Arc::new(Runner(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }))),
    );
    let stop_after = stop.clone();
    let session = Arc::new(AutoReplySession::new(
        complete,
        recorder(sent.clone(), move |_, count| {
            if count == 2 {
                stop_after.cancel();
            }
            ready(confirmed())
        }),
        Some(DailyReplyHistory::new(&days, None)),
        None,
    ));
    let timeout = stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        timeout.cancel();
    });
    let _ = session.run(&room, "1", &stop).await;
    {
        let list = sent.lock().unwrap();
        let ids: std::collections::HashSet<_> = list.iter().map(|c| c.request_id.clone()).collect();
        check.check(
            generated.load(Ordering::SeqCst) == 2 && list.len() == 2 && list.iter().all(|c| c.message == "🤖 reply") && ids.len() == 2,
            "Multiple triggers each send only their generated answer with distinct IDs and no preparation notices",
        );
    }

    // 사용자 요청(2026-11-23): 확인되지 않은 발송이 있어도 세션을 끝내지 않고 다음 호출에 답한다.
    let stop = CancellationToken::new();
    let resilient = fakes::ai(
        reader.clone(),
        store.clone(),
        Arc::new(Runner(Box::new(|| {}))),
    );
    let stop_after = stop.clone();
    let resilient_sent = Arc::new(Mutex::new(Vec::<ApiSendCommand>::new()));
    let session = Arc::new(AutoReplySession::new(
        resilient,
        recorder(resilient_sent.clone(), move |_, count| {
            if count == 2 {
                stop_after.cancel();
                return ready(confirmed());
            }
            ready(fakes::receipt(true, Some(false)))
        }),
        Some(DailyReplyHistory::new(root.join("resilient-days"), None)),
        None,
    ));
    let timeout = stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        timeout.cancel();
    });
    let outcome = session.run(&room, "1", &stop).await;
    check.check(
        resilient_sent.lock().unwrap().len() == 2 && matches!(outcome, Err(Error::Cancelled)),
        "An unconfirmed reply does not end the session; the next call is still answered and only cancellation stops it",
    );

    let candidate = message("6", "2", "@bot congratulations");
    let mut result = fakes::result("축하해요!", &room);
    result.emotion = "happy".into();
    result.attachment = Some(PersonaAttachment {
        persona_id: "default".into(),
        provider: "astra".into(),
        image_id: uuid::Uuid::new_v4().simple().to_string(),
    });
    let ordered = Arc::new(Mutex::new(Vec::<ApiSendCommand>::new()));
    let image_done = CancellationToken::new();
    let gate = image_done.clone();
    let ordered_session = Arc::new(AutoReplySession::new(
        service.clone(),
        recorder(ordered.clone(), move |command, _| {
            if command.is_image() {
                let gate = gate.clone();
                Box::pin(async move {
                    gate.cancelled().await;
                    Ok(confirmed())
                })
            } else {
                ready(confirmed())
            }
        }),
        Some(DailyReplyHistory::new(&days, None)),
        None,
    ));
    let task_session = ordered_session.clone();
    let (task_room, task_candidate, task_result) =
        (room.clone(), candidate.clone(), result.clone());
    let sending = tokio::spawn(async move {
        task_session
            .send_reply(
                &task_room,
                &task_candidate,
                &task_result,
                &CancellationToken::new(),
                "",
            )
            .await
    });
    until(|| !ordered.lock().unwrap().is_empty()).await;
    {
        let list = ordered.lock().unwrap();
        check.check(
            list.len() == 1
                && list[0].attachment == result.attachment
                && list[0].message.is_empty()
                && !sending.is_finished(),
            "Emotion image is dispatched first and text waits for its receipt",
        );
    }
    image_done.cancel();
    sending.await.unwrap().unwrap();
    let ids: Vec<String> = {
        let list = ordered.lock().unwrap();
        check.check(
            list.len() == 2
                && !list[1].is_image()
                && list[1].message == bot_text(&result.text)
                && list[0].request_id.starts_with("bot-image-")
                && list[1].request_id.starts_with("bot-"),
            "Confirmed image is followed by the reply with separate stable request IDs",
        );
        list.iter().map(|c| c.request_id.clone()).collect()
    };
    ordered.lock().unwrap().clear();
    ordered_session
        .send_reply(&room, &candidate, &result, &CancellationToken::new(), "")
        .await
        .unwrap();
    check.check(
        ordered
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.request_id.clone())
            .collect::<Vec<_>>()
            == ids,
        "Repeating a reply retains both idempotency IDs after changing send order",
    );
    for failed in [receipt(false, Some(true)), receipt(true, Some(false))] {
        ordered.lock().unwrap().clear();
        let session = AutoReplySession::new(
            service.clone(),
            recorder(ordered.clone(), move |_, _| ready(failed.clone())),
            Some(DailyReplyHistory::new(&days, None)),
            None,
        );
        let outcome = session
            .send_reply(&room, &candidate, &result, &CancellationToken::new(), "")
            .await;
        let list = ordered.lock().unwrap();
        check.check(
            matches!(outcome, Err(Error::InvalidOperation(_)))
                && list.len() == 1
                && list[0].is_image(),
            "Failed or uncertain image stops before text is sent",
        );
    }
    let cancel_after_image = CancellationToken::new();
    ordered.lock().unwrap().clear();
    let cancel = cancel_after_image.clone();
    let session = AutoReplySession::new(
        service.clone(),
        recorder(ordered.clone(), move |_, _| {
            cancel.cancel();
            ready(confirmed())
        }),
        Some(DailyReplyHistory::new(&days, None)),
        None,
    );
    let outcome = session
        .send_reply(&room, &candidate, &result, &cancel_after_image, "")
        .await;
    check.check(
        matches!(outcome, Err(Error::Cancelled))
            && ordered.lock().unwrap().len() == 1
            && ordered.lock().unwrap()[0].is_image(),
        "Cancellation after image prevents the following text",
    );
    ordered.lock().unwrap().clear();
    let text_only = kakaorelay_ai::ai_service::AiResult {
        attachment: None,
        ..result.clone()
    };
    ordered_session
        .send_reply(&room, &candidate, &text_only, &CancellationToken::new(), "")
        .await
        .unwrap();
    {
        let list = ordered.lock().unwrap();
        check.check(
            list.len() == 1 && list[0].message == bot_text(&result.text) && !list[0].is_image(),
            "No configured image keeps text-only replies working",
        );
    }
    daily_policy(root, check, &service, &room, &candidate, &result).await;
    reply_mode(root, check).await;
    attachment_candidates(check);
    cipher_candidates(check);
    persona_candidates(check);
    self_call_candidates(check);
}

/// 사용자 요청(2026-11-23): 내 메시지도 방의 `@봇이름`으로 시작하면 답장 후보가 된다.
fn self_call_candidates(check: &Check) {
    let rows = [
        message("1", "self", "@봇 내 호출"),
        message("2", "self", "그냥 내 메시지"),
        message("3", "self", "@봇다른 붙은 이름"),
        message("4", "other", "다른 사람 메시지"),
    ];
    let ids = |found: Vec<LocalMessage>| found.into_iter().map(|m| m.id).collect::<Vec<_>>();
    check.check(
        ids(candidates_with_self_call(&rows, "0", "self", "@봇", "@봇").unwrap()) == ["1"],
        "Trigger mode answers own messages only when they start with the room's @bot name",
    );
    check.check(
        ids(candidates_with_self_call(&rows, "0", "self", "", "@봇").unwrap()) == ["1", "4"],
        "Immediate mode answers every other message but own messages only with the @bot call",
    );
    check.check(
        ids(candidates(&rows, "0", "self", "").unwrap()) == ["4"],
        "Without a self call trigger own messages stay excluded as in the original",
    );
    check.check(
        bot_text("안녕") == "🤖 안녕" && bot_text("🤖 이미 붙음") == "🤖 이미 붙음",
        "Bot replies start with the bot mark once",
    );
}

async fn readiness(check: &Check) {
    let rooms: Vec<LocalRoom> = (1..=8)
        .map(|i| LocalRoom::new("fixture", i.to_string(), format!("Fixture room {i}"), true))
        .collect();
    let many = Arc::new(ManyRoomReader {
        rooms: rooms.clone(),
        read_ids: Mutex::new(vec![]),
    });
    let windows: Vec<ConversationTarget> = rooms.iter().map(|r| target(&r.title, &r.id)).collect();
    check.check(
        open_rooms(&rooms, &windows[..4]) == rooms[..4],
        "Auto-reply list contains only currently open rooms and retains more than three entries",
    );
    check.check(
        open_rooms(&rooms, &[]).is_empty(),
        "Auto-reply list is empty when no sending windows are open",
    );
    let ambiguous = ConversationTarget {
        ambiguous: true,
        room_id: None,
        ..windows[0].clone()
    };
    let duplicate = LocalRoom {
        id: "duplicate".into(),
        ..rooms[0].clone()
    };
    check.check(
        open_rooms(&rooms, &[ambiguous, windows[1].clone()]) == [rooms[1].clone()]
            && open_rooms(&[rooms[0].clone(), duplicate], &[windows[0].clone()]).is_empty(),
        "Ambiguous windows and duplicate local titles cannot enter the auto-reply list",
    );
    let none = CancellationToken::new();
    let (_, selected) = prepare(many.as_ref(), &rooms, &scan_of(windows.clone()), &none)
        .await
        .unwrap();
    check.check(
        selected == rooms && *many.read_ids.lock().unwrap() == rooms.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        "More than three auto-reply rooms are accepted and every selected room is checked without truncation",
    );
    check.check(
        matches!(
            prepare(many.as_ref(), &[], &scan_of(windows.clone()), &none).await,
            Err(Error::InvalidOperation(_))
        ),
        "Empty auto-reply room selection is still rejected",
    );
    let missing = prepare(
        many.as_ref(),
        &rooms,
        &scan_of(windows[..7].to_vec()),
        &none,
    )
    .await;
    check.check(
        matches!(&missing, Err(Error::InvalidOperation(m)) if m.contains(&rooms[7].title)),
        "Rooms beyond the old limit still require a valid sending window",
    );
}

async fn daily_policy(
    root: &Path,
    check: &Check,
    service: &Arc<kakaorelay_ai::ai_service::AiService>,
    room: &LocalRoom,
    candidate: &LocalMessage,
    result: &kakaorelay_ai::ai_service::AiResult,
) {
    let clock = FixedClock::utc(2026, 9, 29, 12, 0);
    let daily_root = root.join("daily-image-policy");
    let ordered = Arc::new(Mutex::new(Vec::<ApiSendCommand>::new()));
    let fresh = || {
        AutoReplySession::new(
            service.clone(),
            recorder(ordered.clone(), |_, _| ready(confirmed())),
            Some(DailyReplyHistory::new(&daily_root, Some(clock.clone()))),
            None,
        )
    };
    let none = CancellationToken::new();
    let with_id = |id: &str| LocalMessage {
        id: id.into(),
        ..candidate.clone()
    };
    let neutral = kakaorelay_ai::ai_service::AiResult {
        text: "확인했어요.".into(),
        emotion: "neutral".into(),
        ..result.clone()
    };
    let take = || std::mem::take(&mut *ordered.lock().unwrap());
    take();
    fresh()
        .send_reply(room, candidate, &neutral, &none, "")
        .await
        .unwrap();
    let list = take();
    check.check(
        list.len() == 2 && list[0].is_image(),
        "First neutral reply of the day sends a default image before text",
    );
    fresh()
        .send_reply(room, &with_id("7"), &neutral, &none, "")
        .await
        .unwrap();
    let list = take();
    check.check(
        list.len() == 1 && !list[0].is_image(),
        "Later neutral reply stays text-only after session restart",
    );
    fresh()
        .send_reply(room, candidate, &neutral, &none, "")
        .await
        .unwrap();
    let list = take();
    check.check(
        list.len() == 2 && list[0].is_image(),
        "Retry of the original greeting preserves its attachment and stable request identity",
    );
    fresh()
        .send_reply(room, &with_id("8"), result, &none, "")
        .await
        .unwrap();
    let list = take();
    check.check(
        list.len() == 2 && list[0].is_image(),
        "Expressive replies can still send their matching emotion image later that day",
    );
    let other = LocalRoom {
        id: "other".into(),
        ..room.clone()
    };
    fresh()
        .send_reply(&other, candidate, result, &none, "")
        .await
        .unwrap();
    fresh()
        .send_reply(&other, &with_id("9"), &neutral, &none, "")
        .await
        .unwrap();
    let list = take();
    check.check(
        list.len() == 3 && list.iter().filter(|c| c.is_image()).count() == 1,
        "An emotional first reply consumes the day's first conversation; later neutral does not send a default",
    );
    let text_room = LocalRoom {
        profile: "other-profile".into(),
        ..room.clone()
    };
    let plain = kakaorelay_ai::ai_service::AiResult {
        attachment: None,
        ..neutral.clone()
    };
    fresh()
        .send_reply(&text_room, candidate, &plain, &none, "")
        .await
        .unwrap();
    fresh()
        .send_reply(&text_room, &with_id("10"), &neutral, &none, "")
        .await
        .unwrap();
    let list = take();
    check.check(
        list.len() == 2 && list.iter().all(|c| !c.is_image()),
        "A text-only first reply also prevents later default images",
    );
    let new_room = LocalRoom {
        id: "new-room".into(),
        ..room.clone()
    };
    fresh()
        .send_reply(&new_room, candidate, &neutral, &none, "")
        .await
        .unwrap();
    check.check(
        take().len() == 2,
        "A different room gets its own first daily greeting",
    );
    clock.advance(chrono::Duration::days(1));
    fresh()
        .send_reply(room, &with_id("11"), &neutral, &none, "")
        .await
        .unwrap();
    let list = take();
    check.check(
        list.len() == 2 && list[0].is_image(),
        "A new local calendar day permits a new default greeting",
    );
}

/// 원본 `ReplyModeChecks.Runner`. Grok이 더 빨리 SKIP을 답한다.
struct JudgeRunner;

#[async_trait]
impl AiRunner for JudgeRunner {
    async fn run(
        &self,
        provider: &AiProviderSettings,
        _: &str,
        _: i32,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let delay = if provider.id == "grok" { 10 } else { 60 };
        tokio::select! {
            () = cancel.cancelled() => Err(Error::Cancelled),
            () = tokio::time::sleep(Duration::from_millis(delay)) => Ok("SKIP".into()),
        }
    }
}

struct ModeReader {
    room: LocalRoom,
}

#[async_trait]
impl ChatReader for ModeReader {
    async fn rooms(&self) -> std::result::Result<Vec<LocalRoom>, ApiFailure> {
        Ok(vec![self.room.clone()])
    }
    async fn read(
        &self,
        _: &str,
        _: &str,
        limit: usize,
    ) -> std::result::Result<ChatContext, ApiFailure> {
        Ok(ChatContext {
            room: self.room.clone(),
            messages: vec![message(
                if limit == 1 { "1" } else { "2" },
                "2",
                "호출어 없는 대화",
            )],
            read_at: chrono::Utc::now().fixed_offset(),
        })
    }
}

async fn reply_mode(root: &Path, check: &Check) {
    let messages: Vec<LocalMessage> = (1..=25)
        .map(|i| message(&i.to_string(), "2", &format!("message-{i}")))
        .collect();
    let mut with_extra = messages.clone();
    with_extra.push(LocalMessage {
        id: "26".into(),
        author_id: "1".into(),
        ..messages[24].clone()
    });
    with_extra.push(LocalMessage {
        id: "27".into(),
        deleted: true,
        ..messages[24].clone()
    });
    check.check(
        candidates(&with_extra, "24", "1", "")
            .unwrap()
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>()
            == ["25"],
        "Immediate mode accepts new text while excluding self and deleted messages",
    );
    check.check(
        candidates(&messages, "24", "1", "@bot").unwrap().is_empty(),
        "Trigger mode ignores non-trigger messages",
    );
    let store = Arc::new(AiSettingsStore::new(root.join("reply-mode.json")));
    let mut settings = AiSettings::default();
    settings.poll_seconds = 1;
    settings.reply_mode = "context".into();
    store.save(&mut settings).unwrap();
    let room = LocalRoom::new("test", "1", "테스트", true);
    let reader: Arc<dyn ChatReader> = Arc::new(ModeReader { room: room.clone() });
    let service = fakes::ai(reader, store.clone(), Arc::new(JudgeRunner));
    let sends = Arc::new(Mutex::new(Vec::<ApiSendCommand>::new()));
    let stop = CancellationToken::new();
    let session = Arc::new(AutoReplySession::new(
        service.clone(),
        recorder(sends.clone(), |_, _| ready(confirmed())),
        Some(DailyReplyHistory::new(root.join("mode-days"), None)),
        None,
    ));
    let stopper = stop.clone();
    session.status.subscribe(move |s| {
        if s.contains("맥락상 답변 생략") {
            stopper.cancel();
        }
    });
    let timeout = stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        timeout.cancel();
    });
    let _ = session.run(&room, "1", &stop).await;
    check.check(
        sends.lock().unwrap().is_empty() && service.reply_judge.preferred_provider().is_some(),
        "Context SKIP sends no messages",
    );
    settings
        .room_reply_modes
        .insert(room_key(&room.profile, &room.id), "immediate".into());
    store.save(&mut settings).unwrap();
    let stop = CancellationToken::new();
    let stopper = stop.clone();
    let session = Arc::new(AutoReplySession::new(
        service.clone(),
        recorder(sends.clone(), move |_, _| {
            stopper.cancel();
            ready(confirmed())
        }),
        Some(DailyReplyHistory::new(root.join("mode-days"), None)),
        None,
    ));
    let timeout = stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        timeout.cancel();
    });
    let _ = session.run(&room, "1", &stop).await;
    let list = sends.lock().unwrap();
    check.check(
        list.len() == 1 && list[0].message == "🤖 SKIP" && list[0].request_id.starts_with("bot-"),
        "Room-specific immediate mode overrides the global context mode and sends only the generated answer",
    );
}

fn attachment_candidates(check: &Check) {
    let file = typed("1", "human", "회의.txt", 18, false);
    check.check(
        candidates(std::slice::from_ref(&file), "0", "self", "")
            .unwrap()
            .len()
            == 1
            && candidates(&[file], "0", "self", "@루나")
                .unwrap()
                .is_empty(),
        "File messages trigger immediate replies without bypassing mention mode",
    );
    let photo = typed("1", "human", "사진", 2, false);
    check.check(
        candidates(std::slice::from_ref(&photo), "0", "self", "")
            .unwrap()
            .len()
            == 1
            && candidates(&[photo], "0", "self", "@루나")
                .unwrap()
                .is_empty(),
        "Photos respect immediate and mention reply modes",
    );
}

fn cipher_candidates(check: &Check) {
    let rows = vec![
        message("8", "2", "@bot old"),
        message("10", "1", "@bot own"),
        typed("11", "2", "@bot deleted", 1, true),
        message("12", "2", "ordinary"),
        typed("13", "2", "@bot attachment", 3, false),
        message("14", "2", "@bot question"),
    ];
    check.check(
        candidate(&rows, "9", "1", "@bot")
            .unwrap()
            .is_some_and(|m| m.id == "14"),
        "Auto reply excludes baseline, self, deleted, nontext and untriggered messages",
    );
    check.check(
        candidate(&rows, "14", "1", "@bot").unwrap().is_none(),
        "Auto reply watermark prevents repeated generation for the same trigger",
    );
    let mut repeated = rows.clone();
    repeated.push(message("16", "2", "@bot third"));
    repeated.push(message("15", "2", "@bot second"));
    check.check(
        candidates(&repeated, "9", "1", "@bot")
            .unwrap()
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>()
            == ["14", "15", "16"],
        "All repeated triggers are queued in message order without collapsing to the last call",
    );
}

fn persona_candidates(check: &Check) {
    let mentions = [
        message("1", "other", "@새봇 hello"),
        message("2", "other", "@새봇다른봇 hello"),
    ];
    check.check(
        candidates(&mentions, "0", "self", "@새봇")
            .unwrap()
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>()
            == ["1"],
        "A longer bot name is not mistaken for this bot's mention",
    );
}
