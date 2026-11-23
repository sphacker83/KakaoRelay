//! 원본: `AutoReplySession.cs`. 방별 폴링, 후보 판정, 이미지 → 텍스트 발송 순서, 생성 이미지 분기.
//! 발송은 결과가 `EnterPosted && InputCleared == true`일 때만 다음 단계로 이어지고, 그렇지 않으면 세션을 멈춘다.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kakaorelay_ai::ai_service::{AiCommand, AiResult, AiService};
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::persona_images::PersonaExpression;
use kakaorelay_ai::text::is_blank;
use kakaorelay_ai::{Error, LocalMessage, LocalRoom, Result, json};

use crate::api_send::ApiSendCommand;
use crate::daily_reply::DailyReplyHistory;
use crate::delegations::BackgroundDelegations;
use crate::ids::{MessageId, is_positive_id};
use crate::support::{BoxFuture, StatusEvent, chat_read};
use crate::test_send::TestSendReceipt;

/// 세션이 쓰는 발송. 원본 `Func<ApiSendCommand, Task<TestSendReceipt>>`.
pub type CommandSend =
    Arc<dyn Fn(ApiSendCommand) -> BoxFuture<'static, Result<TestSendReceipt>> + Send + Sync>;

const TRIGGER_PUNCTUATION: &str = ".,!?;:~，。！？";

/// 원본 `char.ToUpperInvariant` 서수 비교. 한 글자 대응만 쓴다.
fn upper(c: char) -> char {
    let mut mapped = c.to_uppercase();
    match (mapped.next(), mapped.next()) {
        (Some(u), None) => u,
        _ => c,
    }
}

fn matches_trigger(text: &str, trigger: &str) -> bool {
    if trigger.is_empty() {
        return true; // 즉시·맥락 방식은 호출어가 필요 없다.
    }
    let mut rest = text.trim_start().chars();
    for expected in trigger.chars() {
        match rest.next() {
            Some(actual) if upper(actual) == upper(expected) => {}
            _ => return false,
        }
    }
    match rest.next() {
        None => true,
        Some(next) => next.is_whitespace() || TRIGGER_PUNCTUATION.contains(next),
    }
}

/// 봇이 보내는 글 앞에 붙이는 표시(사용자 요청 2026-11-23, 원본에 없음). 이미 붙어 있으면 다시 붙이지 않는다.
pub const BOT_MARK: &str = "🤖";

pub fn bot_text(text: &str) -> String {
    if text.trim_start().starts_with(BOT_MARK) {
        text.to_owned()
    } else {
        format!("{BOT_MARK} {text}")
    }
}

/// 새 ID, 본인 아님, 삭제 아님, 종류 1·18·2, 호출어(호출 방식)를 만족하는 메시지를 ID 순으로 모두 고른다.
pub fn candidates(
    messages: &[LocalMessage],
    watermark: &str,
    self_id: &str,
    trigger: &str,
) -> Result<Vec<LocalMessage>> {
    candidates_with_self_call(messages, watermark, self_id, trigger, "")
}

/// `candidates`에 더해, 내가 보낸 메시지도 `self_trigger`(방의 `@봇이름`)로 시작하면 고른다(사용자 요청 2026-11-23, 원본에는 없음).
/// 답변 방식과 무관하게 내 메시지는 호출어가 있을 때만 후보가 된다. `self_trigger`가 비어 있으면 원본과 같다.
pub fn candidates_with_self_call(
    messages: &[LocalMessage],
    watermark: &str,
    self_id: &str,
    trigger: &str,
    self_trigger: &str,
) -> Result<Vec<LocalMessage>> {
    let watermark = MessageId::parse(watermark)?;
    let mut selected = vec![];
    for message in messages {
        let id = MessageId::parse(&message.id)?;
        if id > watermark
            && (message.author_id != self_id
                || (!self_trigger.is_empty() && matches_trigger(&message.text, self_trigger)))
            && !message.deleted
            && matches!(message.kind, 1 | 18 | 2)
            && matches_trigger(&message.text, trigger)
        {
            selected.push((id, message.clone()));
        }
    }
    selected.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(selected.into_iter().map(|(_, m)| m).collect())
}

pub fn candidate(
    messages: &[LocalMessage],
    watermark: &str,
    self_id: &str,
    trigger: &str,
) -> Result<Option<LocalMessage>> {
    Ok(candidates(messages, watermark, self_id, trigger)?
        .into_iter()
        .next())
}

fn identity(room: &LocalRoom, candidate: &LocalMessage) -> String {
    json::hash(&format!("{}:{}:{}", room.profile, room.id, candidate.id))[..40].to_string()
}

fn ensure_running(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(())
}

pub struct AutoReplySession {
    ai: Arc<AiService>,
    send: CommandSend,
    daily: DailyReplyHistory,
    delegation_root: Option<PathBuf>,
    /// 이 세션이 보낸 텍스트(최근 50개). 봇 답장도 내 계정으로 올라오므로, 호출어로 시작하는 답장에 다시 답하는 반복을 막는다.
    sent_texts: std::sync::Mutex<std::collections::VecDeque<String>>,
    greetings: crate::greetings::GreetingStore,
    pub status: StatusEvent,
}

impl AutoReplySession {
    pub fn new(
        ai: Arc<AiService>,
        send: CommandSend,
        history: Option<DailyReplyHistory>,
        delegation_root: Option<PathBuf>,
    ) -> Self {
        Self {
            ai,
            send,
            daily: history
                .unwrap_or_else(|| DailyReplyHistory::new(DailyReplyHistory::default_root(), None)),
            delegation_root,
            sent_texts: std::sync::Mutex::default(),
            greetings: crate::greetings::GreetingStore::new(
                crate::greetings::GreetingStore::default_path(),
            ),
            status: StatusEvent::default(),
        }
    }

    /// 자동인사 설정 파일 위치를 바꾼다(테스트용).
    pub fn with_greetings(mut self, store: crate::greetings::GreetingStore) -> Self {
        self.greetings = store;
        self
    }

    /// 켜져 있으면 오늘 처음 말한 사람에게 인사한다. 확인되지 않은 발송은 답장과 같이 세션을 멈춘다.
    async fn greet(
        &self,
        room: &LocalRoom,
        messages: &[LocalMessage],
        watermark: &str,
        self_id: &str,
    ) -> Result<()> {
        if !self
            .greetings
            .enabled(&kakaorelay_ai::settings::room_key(&room.profile, &room.id))?
        {
            return Ok(());
        }
        let period = crate::greetings::greeting_period(chrono::Utc::now().fixed_offset());
        for speaker in crate::greetings::first_speakers(messages, watermark, self_id, period)? {
            let text = crate::greetings::greeting_text(&speaker.author);
            let receipt = self
                .dispatch(ApiSendCommand::new(
                    crate::greetings::request_id(room, &speaker.author_id, period),
                    &room.title,
                    &text,
                ))
                .await?;
            if !receipt.confirmed() {
                return Err(Error::invalid_operation(
                    "자동인사 발송 결과 확인이 필요해 중지했습니다.",
                ));
            }
            self.report(&format!(
                "{} · {} 자동인사",
                room.title,
                crate::greetings::short_name(&speaker.author)
            ));
        }
        Ok(())
    }

    fn report(&self, text: &str) {
        self.status.emit(text);
    }

    async fn dispatch(&self, command: ApiSendCommand) -> Result<TestSendReceipt> {
        // 발송 결과와 무관하게 먼저 기록한다. 확인 실패여도 실제로 올라갔을 수 있다.
        self.remember_sent(&command.message);
        (self.send)(command).await
    }

    fn remember_sent(&self, text: &str) {
        let mut sent = self.sent_texts.lock().unwrap_or_else(|e| e.into_inner());
        sent.push_back(text.to_owned());
        while sent.len() > 50 {
            sent.pop_front();
        }
    }

    /// 내 메시지 중 이 세션이 보낸 답장과 같은 본문은 후보에서 뺀다.
    fn is_own_reply(&self, message: &LocalMessage, self_id: &str) -> bool {
        message.author_id == self_id
            && self
                .sent_texts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|sent| crate::platform::same_text(sent, &message.text))
    }

    /// 원본 `SendReplyAsync`. 같은 후보·단계는 항상 같은 요청 ID를 쓰므로 재시도해도 중복 발송되지 않는다.
    pub async fn send_reply(
        &self,
        room: &LocalRoom,
        candidate: &LocalMessage,
        result: &AiResult,
        cancel: &CancellationToken,
        phase: &str,
    ) -> Result<()> {
        ensure_running(cancel)?;
        if is_blank(&result.text) {
            return Err(Error::invalid_operation(
                "자동 답변이 비어 있어 중지했습니다.",
            ));
        }
        let identity = if phase.is_empty() {
            identity(room, candidate)
        } else {
            format!("{}-{phase}", identity(room, candidate))
        };
        let title = &room.title;
        let day = self.daily.today();
        if result.image_error.is_none()
            && (result.image_prompt.is_some() || result.image_path.is_some())
        {
            self.report(&format!("{title} · 그림 설명 먼저 전송 중"));
            let introduction = self
                .dispatch(ApiSendCommand::new(
                    format!("bot-{identity}"),
                    title,
                    bot_text(&result.text),
                ))
                .await?;
            if !introduction.confirmed() {
                return Err(Error::invalid_operation(
                    "그림 설명 발송 결과가 불확실해 이미지 생성을 시작하지 않고 중지했습니다.",
                ));
            }
            self.daily.record_sent(room, day, &identity)?;
            ensure_running(cancel)?;
            let status = self.status.clone();
            let progress_title = title.clone();
            let progress = move |s: &str| status.emit(&format!("{progress_title} · {s}"));
            let generated = self
                .ai
                .generate_image(result.clone(), cancel, Some(&progress))
                .await?;
            ensure_running(cancel)?;
            if generated.image_error.is_some() {
                self.report(&format!("{title} · 이미지 생성 실패 안내 전송 중"));
                let failure = self
                    .dispatch(ApiSendCommand::new(
                        format!("bot-image-error-{identity}"),
                        title,
                        bot_text(&generated.text),
                    ))
                    .await?;
                if !failure.confirmed() {
                    return Err(Error::invalid_operation(
                        "이미지 생성 실패 안내의 발송 결과 확인이 필요해 중지했습니다.",
                    ));
                }
                return Ok(());
            }
            let image_path = generated
                .image_path
                .ok_or_else(|| Error::invalid_operation("생성 이미지 경로가 없습니다."))?;
            self.report(&format!("{title} · 생성 이미지 전송 중"));
            let mut command = ApiSendCommand::new(format!("bot-image-{identity}"), title, "");
            command.image_path = Some(image_path);
            if !self.dispatch(command).await?.confirmed() {
                return Err(Error::invalid_operation(
                    "생성 이미지 발송 결과 확인이 필요해 자동 답장을 중지했습니다.",
                ));
            }
            return Ok(());
        }
        let send_image = result.attachment.is_some()
            && (result.emotion != "neutral" || self.daily.is_first_reply(room, day, &identity)?);
        if send_image {
            let label = match &result.image_path {
                Some(_) => "생성한".to_string(),
                None => PersonaExpression::find(&result.emotion)?.name.to_string(),
            };
            self.report(&format!("{title} · {label} 이미지 먼저 전송 중"));
            let mut command = ApiSendCommand::new(format!("bot-image-{identity}"), title, "");
            command.image_path = result.image_path.clone();
            command.attachment = if result.image_path.is_none() {
                result.attachment.clone()
            } else {
                None
            };
            if !self.dispatch(command).await?.confirmed() {
                return Err(Error::invalid_operation(
                    "이미지 발송 결과가 불확실해 텍스트 답변을 보내지 않고 중지했습니다. 발송 이력과 대화를 확인하세요.",
                ));
            }
            self.daily.record_sent(room, day, &identity)?;
            ensure_running(cancel)?;
        }
        self.report(&format!("{title} · 답변 텍스트 전송 중"));
        let receipt = self
            .dispatch(ApiSendCommand::new(
                format!("bot-{identity}"),
                title,
                bot_text(&result.text),
            ))
            .await?;
        if !receipt.confirmed() {
            return Err(Error::invalid_operation(
                "답변 발송 결과 확인이 필요해 자동 답장을 중지했습니다. 발송 이력과 대화를 확인하세요.",
            ));
        }
        self.daily.record_sent(room, day, &identity)
    }

    /// 일반 폴링과 제어방 대화가 함께 쓴다. 위임 표시가 있으면 Grok 작업으로 넘기고, 없으면 답변을 보낸다.
    pub async fn reply_to(
        &self,
        room: &LocalRoom,
        candidate: &LocalMessage,
        jobs: &BackgroundDelegations,
        cancel: &CancellationToken,
    ) -> Result<AiResult> {
        let mut command = AiCommand::new(&room.profile, &room.id, "reply");
        command.instruction = format!(
            "메시지 ID {}에만 답하라. 다른 호출은 별도로 처리된다. 호출 내용: {}",
            candidate.id,
            json::prompt(&candidate.text)
        );
        command.target_message_id = Some(candidate.id.clone());
        let status = self.status.clone();
        let title = room.title.clone();
        let progress = move |s: &str| status.emit(&format!("{title} · {s}"));
        let result = self
            .ai
            .generate(&command, cancel, Some(&progress), true, true, None)
            .await?;
        if result.delegation.is_some() {
            jobs.start(room, candidate, &result).await?;
        } else {
            self.send_reply(room, candidate, &result, cancel, "")
                .await?;
        }
        self.report(&format!(
            "{} · {} 답변 전송 요청 처리 · 다음 호출 대기",
            room.title, result.provider
        ));
        Ok(result)
    }

    /// 위임 작업이 이 세션의 발송 규칙(`send_reply`)으로 결과를 보내게 하는 연결.
    pub fn delegations(
        self: &Arc<Self>,
        room_title: &str,
        cancel: &CancellationToken,
    ) -> BackgroundDelegations {
        let me = self.clone();
        let send: crate::delegations::DelegationSend =
            Arc::new(move |room, message, result, phase, ct| {
                let me = me.clone();
                Box::pin(async move { me.send_reply(&room, &message, &result, &ct, &phase).await })
            });
        let status = self.status.clone();
        let title = room_title.to_string();
        BackgroundDelegations::new(
            self.ai.clone(),
            send,
            Arc::new(move |s: &str| status.emit(&format!("{title} · {s}"))),
            cancel,
            self.delegation_root.clone(),
        )
    }

    /// 원본 `RunAsync`. 시작 시점의 최신 메시지를 기준점으로 삼고 이후 새 메시지만 처리한다. 취소될 때까지 돈다.
    pub async fn run(
        self: &Arc<Self>,
        room: &LocalRoom,
        self_id: &str,
        cancel: &CancellationToken,
    ) -> Result<()> {
        if !is_positive_id(self_id) {
            return Err(Error::argument("내 사용자 ID가 필요합니다."));
        }
        let jobs = self.delegations(&room.title, cancel);
        let outcome = self.poll(room, self_id, &jobs, cancel).await;
        jobs.dispose().await;
        outcome
    }

    async fn poll(
        &self,
        room: &LocalRoom,
        self_id: &str,
        jobs: &BackgroundDelegations,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let reader = self.ai.chats().clone();
        let title = &room.title;
        let mut settings = self.ai.settings().load()?;
        let mut mode = settings.reply_mode_for_room(&room.profile, &room.id);
        let baseline = chat_read(reader.as_ref(), &room.profile, &room.id, 1, cancel).await?;
        let mut watermark = baseline
            .messages
            .last()
            .map_or_else(|| "0".to_string(), |m| m.id.clone());
        self.report(&format!(
            "{title} 대기 중 · {mode} · {}초 폴링 · 시작 이후 새 메시지만 답장",
            settings.poll_seconds
        ));
        loop {
            let delay = Duration::from_secs(u64::try_from(settings.poll_seconds).unwrap_or(1));
            tokio::select! {
                () = cancel.cancelled() => return Err(Error::Cancelled),
                () = tokio::time::sleep(delay) => {}
            }
            let context = chat_read(reader.as_ref(), &room.profile, &room.id, 500, cancel).await?;
            if context.messages.len() == 500
                && MessageId::parse(&context.messages[0].id)? > MessageId::parse(&watermark)?
            {
                return Err(Error::invalid_operation(
                    "새 메시지가 읽기 범위를 초과해 중지했습니다. 누락된 호출을 확인하세요.",
                ));
            }
            settings = self.ai.settings().load()?;
            mode = settings.reply_mode_for_room(&room.profile, &room.id);
            let trigger = settings.trigger_for_room(&room.profile, &room.id)?;
            self.greet(room, &context.messages, &watermark, self_id)
                .await?;
            let mut pending = candidates_with_self_call(
                &context.messages,
                &watermark,
                self_id,
                if mode == "trigger" { &trigger } else { "" },
                &trigger,
            )?;
            pending.retain(|m| !self.is_own_reply(m, self_id));
            if mode == "context" {
                let persona = settings.resolve_persona(&room.profile, &room.id)?.persona;
                let mut approved = vec![];
                for candidate in pending {
                    let candidate_id = MessageId::parse(&candidate.id)?;
                    let mut recent = vec![];
                    for message in &context.messages {
                        if MessageId::parse(&message.id)? <= candidate_id {
                            recent.push(message.clone());
                        }
                    }
                    let mut recent = recent.split_off(recent.len().saturating_sub(20));
                    for message in recent.iter_mut().filter(|m| m.id == candidate.id) {
                        *message = reader.read_attachment(message.clone()).await?;
                    }
                    let status = self.status.clone();
                    let progress_title = title.clone();
                    let progress = move |s: &str| status.emit(&format!("{progress_title} · {s}"));
                    let needed = self
                        .ai
                        .reply_judge
                        .should_reply(&settings, &persona, &recent, cancel, Some(&progress))
                        .await?;
                    self.report(&format!(
                        "{title} · 메시지 {} · {}",
                        candidate.id,
                        if needed {
                            "답변 필요"
                        } else {
                            "맥락상 답변 생략"
                        }
                    ));
                    if needed {
                        approved.push(candidate);
                    }
                }
                pending = approved;
            }
            for candidate in &pending {
                // 생성 전에 호출을 소비한다. 실패를 새 발송으로 재시도하지 않는다.
                self.report(&format!(
                    "{title} 답변 생성 중 · 이번 묶음 {}개 호출",
                    pending.len()
                ));
                self.reply_to(room, candidate, jobs, cancel).await?;
            }
            if let Some(last) = context.messages.last() {
                watermark = last.id.clone();
            }
            let grok = match jobs.pending() {
                0 => String::new(),
                n => format!(" · Grok {n}건 진행 중"),
            };
            self.report(&format!(
                "{title} 대기 중 · 마지막 확인 {} · {mode} · {}초 폴링{grok}",
                chrono::Local::now().format("%H:%M:%S"),
                settings.poll_seconds
            ));
        }
    }
}
