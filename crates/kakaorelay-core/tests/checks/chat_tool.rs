//! 원본: `ChatToolChecks.cs`의 자동답장 발송 순서 검사 7개(`AutoReplySession`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use kakaorelay_ai::ai_service::{AiCommand, AiResult};
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::chat_image::{ChatImageGenerator, Progress};
use kakaorelay_ai::cli_runner::AiRunner;
use kakaorelay_ai::imported_reader::ImportedChatReader;
use kakaorelay_ai::settings::{AiProviderSettings, AiSettings, AiSettingsStore};
use kakaorelay_ai::{ChatReader, Error, LocalMessage, Result};
use kakaorelay_core::api_send::ApiSendCommand;
use kakaorelay_core::auto_reply::{AutoReplySession, bot_text};
use kakaorelay_core::daily_reply::DailyReplyHistory;

use crate::fakes::{self, confirmed, ready, receipt, recorder, until};
use crate::support::Check;

const DIRECTIVE: &str = "[[generate_image:\"달 위의 한복 고양이\"]]";

struct Runner {
    reply: Mutex<String>,
}

#[async_trait]
impl AiRunner for Runner {
    async fn run(
        &self,
        _: &AiProviderSettings,
        _: &str,
        _: i32,
        _: &CancellationToken,
    ) -> Result<String> {
        Ok(self.reply.lock().unwrap().clone())
    }
}

#[derive(Default)]
struct Images {
    calls: AtomicUsize,
    fail: AtomicBool,
    /// (시작 신호, 완료 대기). 원본 `BeforeGenerate`.
    gate: Mutex<Option<(CancellationToken, CancellationToken)>>,
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
        let gate = self.gate.lock().unwrap().clone();
        if let Some((started, finish)) = gate {
            started.cancel();
            finish.cancelled().await;
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(Error::invalid_data("fixture failure"));
        }
        Ok(std::path::absolute("generated.png").unwrap())
    }
}

pub async fn run(root: &Path, check: &Check) {
    let reader = Arc::new(ImportedChatReader::new());
    let room = reader
        .import(
            "image-request.txt",
            "--------------- 2026년 9월 29일 ---------------\n[테스트] [오후 1:00] 달 위의 한복 고양이 그려줘",
        )
        .unwrap();
    let store = Arc::new(AiSettingsStore::new(root.join("chat-image-settings.json")));
    let runner = Arc::new(Runner {
        reply: Mutex::new(format!(
            "달 위의 한복 고양이를 수채화로 그려볼게요.\n{DIRECTIVE}\n[[emotion:happy]]"
        )),
    });
    let images = Arc::new(Images::default());
    let service = fakes::ai_with_images(
        reader.clone() as Arc<dyn ChatReader>,
        store,
        runner.clone(),
        images.clone(),
    );
    let none = CancellationToken::new();
    let command = AiCommand::new(&room.profile, &room.id, "reply");
    let result = service
        .generate(&command, &none, None, false, false, None)
        .await
        .unwrap();
    *runner.reply.lock().unwrap() = "평범한 답변\n[[emotion:neutral]]".into();
    let ordinary = service
        .generate(&command, &none, None, false, false, None)
        .await
        .unwrap();
    let image_path = root
        .join("generated-reply.png")
        .to_string_lossy()
        .into_owned();
    let sent = Arc::new(Mutex::new(Vec::<ApiSendCommand>::new()));
    let session = AutoReplySession::new(
        service.clone(),
        recorder(sent.clone(), |_, _| ready(confirmed())),
        Some(DailyReplyHistory::new(root.join("image-send-days"), None)),
        None,
    );
    let candidate = LocalMessage::new(
        "901",
        "other",
        "tester",
        chrono::Utc::now().fixed_offset(),
        1,
        "그려줘",
        false,
    );
    let with_id = |id: &str| LocalMessage {
        id: id.into(),
        ..candidate.clone()
    };
    session
        .send_reply(&room, &candidate, &ordinary, &none, "")
        .await
        .unwrap();
    sent.lock().unwrap().clear();
    let requested = AiResult {
        image_path: Some(image_path.clone()),
        ..result.clone()
    };
    session
        .send_reply(&room, &with_id("902"), &requested, &none, "")
        .await
        .unwrap();
    {
        let list = sent.lock().unwrap();
        check.check(
            list.len() == 2
                && list[0].message == bot_text(&result.text)
                && list[1].image_path.as_deref() == Some(image_path.as_str())
                && list[1].attachment.is_none(),
            "Requested image follows the explanation even after today's first reply and without persona images enabled",
        );
    }
    *runner.reply.lock().unwrap() =
        format!("달 위의 한복 고양이를 수채화로 그려볼게요.\n{DIRECTIVE}\n[[emotion:neutral]]");
    let before_plan = images.calls.load(Ordering::SeqCst);
    let plan = service
        .generate(&command, &none, None, true, false, None)
        .await
        .unwrap();
    let intro_done = CancellationToken::new();
    let (image_started, image_done) = (CancellationToken::new(), CancellationToken::new());
    *images.gate.lock().unwrap() = Some((image_started.clone(), image_done.clone()));
    sent.lock().unwrap().clear();
    let intro = intro_done.clone();
    let staged = Arc::new(AutoReplySession::new(
        service.clone(),
        recorder(sent.clone(), move |command, _| {
            if command.is_image() {
                ready(confirmed())
            } else {
                let intro = intro.clone();
                Box::pin(async move {
                    intro.cancelled().await;
                    Ok(confirmed())
                })
            }
        }),
        Some(DailyReplyHistory::new(root.join("staged-image-days"), None)),
        None,
    ));
    let (task_session, task_room, task_plan) = (staged.clone(), room.clone(), plan.clone());
    let task_candidate = with_id("903");
    let sending = tokio::spawn(async move {
        task_session
            .send_reply(
                &task_room,
                &task_candidate,
                &task_plan,
                &CancellationToken::new(),
                "",
            )
            .await
    });
    until(|| !sent.lock().unwrap().is_empty()).await;
    check.check(
        sent.lock().unwrap().len() == 1
            && sent.lock().unwrap()[0].message == bot_text(&plan.text)
            && images.calls.load(Ordering::SeqCst) == before_plan
            && !sending.is_finished(),
        "Only the drawing explanation is sent while its delivery receipt is pending",
    );
    intro_done.cancel();
    let _ =
        tokio::time::timeout(std::time::Duration::from_secs(5), image_started.cancelled()).await;
    check.check(
        sent.lock().unwrap().len() == 1 && images.calls.load(Ordering::SeqCst) == before_plan + 1 && !sending.is_finished(),
        "Image generation starts only after explanation delivery and sends nothing while generating",
    );
    image_done.cancel();
    sending.await.unwrap().unwrap();
    {
        let list = sent.lock().unwrap();
        check.check(
            list.len() == 2 && list[1].is_image() && list[1].image_path.is_some(),
            "Completed drawing sends only the image, without repeating the explanation",
        );
    }
    *images.gate.lock().unwrap() = None;
    let before_failure = images.calls.load(Ordering::SeqCst);
    let uncertain = AutoReplySession::new(
        service.clone(),
        Arc::new(|_| ready(receipt(true, Some(false)))),
        Some(DailyReplyHistory::new(
            root.join("uncertain-image-days"),
            None,
        )),
        None,
    );
    let outcome = uncertain
        .send_reply(&room, &candidate, &plan, &none, "")
        .await;
    check.check(
        matches!(outcome, Err(Error::InvalidOperation(_)))
            && images.calls.load(Ordering::SeqCst) == before_failure,
        "Uncertain explanation delivery prevents image generation",
    );
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let cancelled = AutoReplySession::new(
        service.clone(),
        Arc::new(move |_| {
            trigger.cancel();
            ready(confirmed())
        }),
        Some(DailyReplyHistory::new(root.join("cancel-image-days"), None)),
        None,
    );
    let outcome = cancelled
        .send_reply(&room, &candidate, &plan, &cancel, "")
        .await;
    check.check(
        matches!(outcome, Err(Error::Cancelled))
            && images.calls.load(Ordering::SeqCst) == before_failure,
        "Stop after the explanation prevents image generation",
    );
    images.fail.store(true, Ordering::SeqCst);
    sent.lock().unwrap().clear();
    session
        .send_reply(&room, &with_id("904"), &plan, &none, "")
        .await
        .unwrap();
    let list = sent.lock().unwrap();
    check.check(
        list.len() == 2
            && list.iter().all(|c| !c.is_image())
            && list[0].message == bot_text(&plan.text)
            && list[1].message.contains("생성하지 못")
            && list[1].request_id.starts_with("bot-image-error-"),
        "Generation failure follows the explanation with a distinct failure notice and no image",
    );
}
