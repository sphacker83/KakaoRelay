//! 원본: `SummaryForwarder.cs`. 정리·전달 규칙 처리, 원본 방별 커서, 보류 요약.
//! 처음 켜면 기준점만 저장한다. 전송 확인이 불확실하면 `dispatching`에 고정하고 자동으로 다시 보내지 않는다.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Utc};
use indexmap::IndexMap;
use kakaorelay_ai::ai_service::AiService;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::settings::AiSettings;
use kakaorelay_ai::summary::{SummaryInput, SummaryUpdate};
use kakaorelay_ai::summary_forward_settings::SummaryForwardRule;
use kakaorelay_ai::text::{NEW_LINE, is_blank, nfc, utf16_prefix};
use kakaorelay_ai::{Error, LocalMessage, Result, json};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api_send::{ApiSendCommand, Scan};
use crate::fs::write_atomic;
use crate::ids::MessageId;
use crate::readiness::open_rooms;
use crate::reply_runtime::CancellableSend;
use crate::support::{Clock, StatusEvent, SystemClock, chat_read, chat_rooms, culture_cmp, scan};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ForwardState {
    pub cursors: IndexMap<String, String>,
    #[serde(with = "kakaorelay_ai::json::time")]
    pub last_run: DateTime<FixedOffset>,
    pub history: Vec<SummaryUpdate>,
    pub latest_facts: IndexMap<String, String>,
    pub pending: Option<PendingSummary>,
    pub source_errors: IndexMap<String, String>,
}

impl Default for ForwardState {
    fn default() -> Self {
        Self {
            cursors: IndexMap::new(),
            last_run: json::time::min_value(),
            history: vec![],
            latest_facts: IndexMap::new(),
            pending: None,
            source_errors: IndexMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSummary {
    pub request_id: String,
    pub text: String,
    #[serde(default)]
    pub items: Vec<SummaryUpdate>,
    #[serde(default)]
    pub cursors: IndexMap<String, String>,
    #[serde(default)]
    pub dispatching: bool,
}

impl ForwardState {
    /// `JsonSerializerDefaults.Web` 읽기(이름 대소문자 무시). 사전 키는 그대로 둔다.
    pub fn from_json(text: &str) -> Result<Option<Self>> {
        let mut value: serde_json::Value = serde_json::from_str(text)?;
        json::normalize_keys(
            &mut value,
            &[
                "cursors",
                "lastRun",
                "history",
                "latestFacts",
                "pending",
                "sourceErrors",
                "requestId",
                "text",
                "items",
                "dispatching",
                "topic",
                "evidence",
                "source",
                "messageId",
                "quote",
            ],
            &["cursors", "latestFacts", "sourceErrors"],
        );
        Ok(serde_json::from_value(value)?)
    }
}

/// 골든 `summaryIdentity`. 상태 파일 이름이 된다.
pub fn identity(rule: &SummaryForwardRule) -> Result<String> {
    let mut sources: Vec<String> = rule.sources.iter().map(|s| s.key()).collect();
    sources.sort_by(|a, b| culture_cmp(a, b));
    let destination = rule
        .destination
        .as_ref()
        .ok_or_else(|| Error::invalid_operation("수신방이 없습니다."))?;
    Ok(json::hash(&json::prompt(&json!({
        "Id": rule.id,
        "Sources": sources,
        "Destination": destination.key(),
    }))))
}

fn content_hash(text: &str) -> String {
    json::hash(nfc(text).trim())
}

/// 골든 `summaryFactKey`. 같은 주제·출처의 최신 사실 해시를 기억해 중복 알림을 막는다.
pub fn fact_key(item: &SummaryUpdate) -> String {
    let mut sources: Vec<String> = vec![];
    for evidence in item.evidence.iter().flatten() {
        if !sources.contains(&evidence.source) {
            sources.push(evidence.source.clone());
        }
    }
    sources.sort_by(|a, b| culture_cmp(a, b));
    json::hash(&json::prompt(
        &json!({"Topic": nfc(&item.topic).trim(), "Sources": sources}),
    ))
}

pub fn default_root() -> PathBuf {
    kakaorelay_ai::paths::app_data().join("summary-forwards")
}

pub struct SummaryForwarder {
    ai: Arc<AiService>,
    send: CancellableSend,
    windows: Scan,
    root: PathBuf,
    clock: Arc<dyn Clock>,
    gate: tokio::sync::Mutex<()>,
    pub status: StatusEvent,
}

fn save(path: &std::path::Path, state: &ForwardState) -> Result<()> {
    write_atomic(path, json::indented(state).as_bytes(), false)
}

impl SummaryForwarder {
    pub fn new(
        ai: Arc<AiService>,
        send: CancellableSend,
        windows: Scan,
        state_root: Option<PathBuf>,
        clock: Option<Arc<dyn Clock>>,
    ) -> Self {
        Self {
            ai,
            send,
            windows,
            root: state_root.unwrap_or_else(default_root),
            clock: clock.unwrap_or_else(|| Arc::new(SystemClock)),
            gate: tokio::sync::Mutex::new(()),
            status: StatusEvent::default(),
        }
    }

    fn utc_now(&self) -> DateTime<FixedOffset> {
        self.clock.now().with_timezone(&Utc).fixed_offset()
    }

    /// 한 번에 한 틱만 돈다. 이미 도는 중이면 바로 돌아간다.
    pub async fn tick(&self, cancel: &CancellationToken) -> Result<()> {
        let Ok(_guard) = self.gate.try_lock() else {
            return Ok(());
        };
        let settings = self.ai.settings().load()?;
        for rule in settings.summary_forwards.iter().filter(|r| r.enabled) {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            match self.process(rule, &settings, cancel).await {
                Ok(()) => {}
                Err(Error::Cancelled) if cancel.is_cancelled() => return Err(Error::Cancelled),
                Err(error) => self.report(rule, &format!("실패: {error}")),
            }
        }
        Ok(())
    }

    fn report(&self, rule: &SummaryForwardRule, message: &str) {
        let title = rule.destination.as_ref().map_or("", |d| d.title.as_str());
        self.status.emit(&format!("{title} · {message}"));
    }

    fn report_status(&self, rule: &SummaryForwardRule, state: &ForwardState, message: &str) {
        let failures: Vec<String> = rule
            .sources
            .iter()
            .filter_map(|s| {
                state
                    .source_errors
                    .get(&s.key())
                    .map(|e| format!("{}: {e}", s.title))
            })
            .collect();
        let suffix = if state.source_errors.is_empty() {
            String::new()
        } else {
            format!(" · 읽기 실패: {}", failures.join(" / "))
        };
        self.report(rule, &format!("{message}{suffix}"));
    }

    async fn process(
        &self,
        rule: &SummaryForwardRule,
        settings: &AiSettings,
        cancel: &CancellationToken,
    ) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let rule_identity = identity(rule)?;
        let path = self.root.join(format!("{rule_identity}.json"));
        let mut state = if path.exists() {
            ForwardState::from_json(&kakaorelay_ai::text::read_text(&path)?)?
                .ok_or_else(|| Error::io("전달 상태 파일 오류"))?
        } else {
            ForwardState::default()
        };
        let now = self.utc_now();
        if state.pending.as_ref().is_some_and(|p| p.dispatching) {
            self.report(
                rule,
                "전송 결과 확인 필요 · 재발송 중단 · 발송 이력 확인 후 규칙을 껐다 켜세요.",
            );
            return Ok(());
        }
        if state.last_run != json::time::min_value()
            && now - state.last_run < chrono::Duration::minutes(i64::from(rule.interval_minutes))
        {
            return Ok(());
        }
        let reader = self.ai.chats().clone();
        if state.pending.is_none() {
            chat_rooms(reader.as_ref(), cancel).await?;
            let mut inputs: Vec<SummaryInput> = vec![];
            let baseline = state.cursors.is_empty();
            for source in &rule.sources {
                let key = source.key();
                let outcome: Result<()> = async {
                    let cursor = state.cursors.get(&key).cloned();
                    let context = chat_read(
                        reader.as_ref(),
                        &source.profile,
                        &source.id,
                        if cursor.is_some() { 500 } else { 1 },
                        cancel,
                    )
                    .await?;
                    let mut messages = context.messages;
                    sort_by_id(&mut messages)?;
                    let latest = messages.last().map_or_else(|| "0".to_string(), |m| m.id.clone());
                    match cursor {
                        None => {
                            state.cursors.insert(key.clone(), latest);
                        }
                        Some(cursor) => {
                            let cursor = MessageId::parse(&cursor)?;
                            if MessageId::parse(&latest)? < cursor
                                || (messages.len() == 500 && MessageId::parse(&messages[0].id)? > cursor)
                            {
                                return Err(Error::io(
                                    "처리 위치와 조회 범위가 맞지 않습니다. 주기를 줄이고 규칙을 껐다 켜세요.",
                                ));
                            }
                            for message in messages {
                                if MessageId::parse(&message.id)? > cursor {
                                    inputs.push(SummaryInput {
                                        source: key.clone(),
                                        message,
                                    });
                                }
                            }
                        }
                    }
                    Ok(())
                }
                .await;
                match outcome {
                    Ok(()) => {
                        state.source_errors.shift_remove(&key);
                    }
                    Err(Error::Cancelled) if cancel.is_cancelled() => return Err(Error::Cancelled),
                    // 이 방의 커서는 그대로 두고 다른 원본 방은 계속 진행한다.
                    Err(error) => {
                        state.source_errors.insert(key, error.to_string());
                    }
                }
            }
            if baseline {
                state.last_run = now;
                save(&path, &state)?;
                self.report_status(
                    rule,
                    &state,
                    "읽을 수 있는 방의 시작 기준 저장 · 이후 새 대화부터 정리",
                );
                return Ok(());
            }
            // 앞쪽 100개(첨부 5개까지)만 처리하고 나머지는 다음 주기로 넘긴다.
            let mut keyed = Vec::with_capacity(inputs.len());
            for input in inputs {
                keyed.push((MessageId::parse(&input.message.id)?, input));
            }
            keyed.sort_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| culture_cmp(&a.1.source, &b.1.source))
            });
            let mut attachments = 0;
            let batch: Vec<(MessageId, SummaryInput)> = keyed
                .into_iter()
                .take(100)
                .take_while(|(_, i)| {
                    if matches!(i.message.kind, 18 | 2) {
                        attachments += 1;
                        attachments <= 5
                    } else {
                        true
                    }
                })
                .collect();
            let mut next = state.cursors.clone();
            let mut highest: IndexMap<String, MessageId> = IndexMap::new();
            for (id, input) in &batch {
                if highest.get(&input.source).is_none_or(|h| id > h) {
                    highest.insert(input.source.clone(), id.clone());
                    next.insert(input.source.clone(), input.message.id.clone());
                }
            }
            let mut text_inputs: Vec<SummaryInput> = batch
                .into_iter()
                .map(|(_, i)| i)
                .filter(|i| {
                    !i.message.deleted
                        && matches!(i.message.kind, 1 | 18 | 2)
                        && !is_blank(&i.message.text)
                        && !i.message.text.starts_with("[정리·전달]")
                })
                .map(|i| SummaryInput {
                    message: LocalMessage {
                        text: utf16_prefix(&i.message.text, 3000).to_string(),
                        ..i.message
                    },
                    source: i.source,
                })
                .collect();
            for input in &mut text_inputs {
                input.message = reader.read_attachment(input.message.clone()).await?;
            }
            state.last_run = now;
            save(&path, &state)?;
            let items = if text_inputs.is_empty() {
                vec![]
            } else {
                let previous: Vec<SummaryUpdate> =
                    state.history[state.history.len().saturating_sub(80)..].to_vec();
                self.ai
                    .summaries
                    .generate(settings, &text_inputs, &previous, cancel)
                    .await?
                    .items
                    .unwrap_or_default()
            };
            let mut seen = std::collections::HashSet::new();
            let updates: Vec<SummaryUpdate> = items
                .into_iter()
                .filter(|i| seen.insert(content_hash(&i.text)))
                .filter(|i| state.latest_facts.get(&fact_key(i)) != Some(&content_hash(&i.text)))
                .collect();
            if updates.is_empty() {
                state.cursors = next;
                state.last_run = now;
                save(&path, &state)?;
                self.report_status(rule, &state, "추가사항 없음 · 발송 생략");
                return Ok(());
            }
            let titles: Vec<&str> = rule.sources.iter().map(|s| s.title.as_str()).collect();
            let mut body = format!("[정리·전달]\n{}{NEW_LINE}", titles.join(", "));
            body.push_str(&format!(
                "{}{NEW_LINE}",
                now.with_timezone(&chrono::Local).format("%m/%d %H:%M")
            ));
            for item in &updates {
                let sources: Vec<&str> = rule
                    .sources
                    .iter()
                    .filter(|s| item.evidence.iter().flatten().any(|e| e.source == s.key()))
                    .map(|s| s.title.as_str())
                    .collect();
                body.push_str(&format!(
                    "\n• {}\n{}\n출처: {}{NEW_LINE}",
                    item.topic,
                    item.text,
                    sources.join(", ")
                ));
            }
            state.pending = Some(PendingSummary {
                request_id: format!("summary-{}", uuid::Uuid::new_v4().simple()),
                text: body,
                items: updates,
                cursors: next,
                dispatching: false,
            });
            save(&path, &state)?;
        }
        // 이름이 바뀌었거나 모호한 창으로 보내지 않도록 계정·방 식별자로 다시 확인한다.
        let current = self.ai.settings().load()?;
        let still_enabled = current
            .summary_forwards
            .iter()
            .any(|r| r.enabled && identity(r).is_ok_and(|id| id == rule_identity));
        if !still_enabled {
            return Ok(());
        }
        let target = rule
            .destination
            .as_ref()
            .ok_or_else(|| Error::invalid_operation("수신방이 없습니다."))?;
        let rooms = chat_rooms(reader.as_ref(), cancel).await?;
        let matches: Vec<_> = rooms
            .iter()
            .filter(|r| r.profile == target.profile && r.id == target.id)
            .collect();
        let [destination] = matches.as_slice() else {
            return Err(Error::io("수신방을 로컬 목록에서 찾지 못했습니다."));
        };
        let destination = (*destination).clone();
        let windows = scan(&self.windows, cancel).await?;
        if !open_rooms(&rooms, &windows)
            .iter()
            .any(|r| r.profile == destination.profile && r.id == destination.id)
        {
            state.last_run = now;
            save(&path, &state)?;
            self.report(rule, "수신방을 열어주세요 · 요약 보관, 다음 주기에 재확인");
            return Ok(());
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let pending = {
            let pending = state.pending.as_mut().expect("위에서 만들거나 읽었음");
            pending.dispatching = true;
            pending.clone()
        };
        save(&path, &state)?; // 중단·불확실 발송은 절대 자동으로 다시 보내지 않는다.
        let receipt = (self.send)(
            ApiSendCommand::new(&pending.request_id, &destination.title, &pending.text),
            cancel.clone(),
        )
        .await?;
        if !receipt.confirmed() {
            return Err(Error::io("전송 결과 확인 필요 · 자동 재발송 중단"));
        }
        state.cursors = pending.cursors.clone();
        state.history.extend(pending.items.iter().cloned());
        for item in &pending.items {
            state
                .latest_facts
                .insert(fact_key(item), content_hash(&item.text));
        }
        state.pending = None;
        state.last_run = self.utc_now();
        save(&path, &state)?;
        self.report_status(
            rule,
            &state,
            &format!("추가사항 {}건 전달 완료", pending.items.len()),
        );
        Ok(())
    }
}

fn sort_by_id(messages: &mut Vec<LocalMessage>) -> Result<()> {
    let mut keyed = Vec::with_capacity(messages.len());
    for message in messages.drain(..) {
        keyed.push((MessageId::parse(&message.id)?, message));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    messages.extend(keyed.into_iter().map(|(_, m)| m));
    Ok(())
}
