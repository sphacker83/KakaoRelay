//! 앱 전역 상태. 원본 `App`·`MainWindow`·각 패널이 필드로 들고 있던 서비스와 작업 상태를 모은다.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use kakaorelay_ai::ai_service::AiService;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::chat_image::CliChatImageGenerator;
use kakaorelay_ai::cli_runner::CliAiRunner;
use kakaorelay_ai::imported_reader::ImportedChatReader;
use kakaorelay_ai::settings::{AiSettings, AiSettingsStore};
use kakaorelay_ai::{Error, Result};
use kakaorelay_core::compose::ComposeSession;
use kakaorelay_core::host::RelayHost;
use kakaorelay_core::platform::Platform;
use kakaorelay_core::test_send::{TestSendReceipt, default_ledger};

/// 화면별 진행 작업. 원본 패널의 `operation`(CancellationTokenSource)과 같다. 같은 이름의 작업은 하나만 돈다.
type ChangeHandler = Box<dyn Fn(&Operations) + Send + Sync>;

#[derive(Default)]
pub struct Operations {
    slots: Mutex<HashMap<&'static str, CancellationToken>>,
    /// 작업이 시작·끝날 때 부른다. 제어방이 화면 설정 작업 중인지 알 수 있게 한다.
    changed: OnceLock<ChangeHandler>,
}

pub struct OperationGuard<'a> {
    owner: &'a Operations,
    name: &'static str,
    pub token: CancellationToken,
}

impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        self.owner.lock().remove(self.name);
        self.owner.notify();
    }
}

impl Operations {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<&'static str, CancellationToken>> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn begin(&self, name: &'static str) -> Result<OperationGuard<'_>> {
        let mut slots = self.lock();
        if slots.contains_key(name) {
            return Err(Error::invalid_operation(
                "다른 작업을 처리 중입니다. 잠시 후 다시 시도하세요.",
            ));
        }
        let token = CancellationToken::new();
        slots.insert(name, token.clone());
        drop(slots);
        self.notify();
        Ok(OperationGuard {
            owner: self,
            name,
            token,
        })
    }

    pub fn on_change(&self, handler: impl Fn(&Operations) + Send + Sync + 'static) {
        let _ = self.changed.set(Box::new(handler));
    }

    fn notify(&self) {
        if let Some(handler) = self.changed.get() {
            handler(self);
        }
    }

    pub fn cancel(&self, name: &str) {
        if let Some(token) = self.lock().get(name) {
            token.cancel();
        }
    }

    pub fn busy(&self, name: &str) -> bool {
        self.lock().contains_key(name)
    }

    pub fn cancel_all(&self) {
        for token in self.lock().values() {
            token.cancel();
        }
    }
}

/// 메시지 탭의 수동 발송 상태(원본 `MainWindow`의 `compose`, `currentReceipt`).
#[derive(Default)]
pub struct Manual {
    pub compose: ComposeSession,
    pub current: Option<TestSendReceipt>,
    /// 마지막으로 화면에 보낸 열린 방. 화면에 직렬화하지 않는 값(맥 DB 방 ID 등)을 발송 요청에 넘기려고 보관한다.
    pub targets: Vec<kakaorelay_core::platform::ConversationTarget>,
}

pub struct AppState {
    pub platform: Arc<dyn Platform>,
    pub ai: Arc<AiService>,
    /// TXT·JSON 가져오기 전용(원본 Portable). 가져온 방은 profile `import`이고 이 서비스로만 생성한다.
    pub import_ai: Arc<AiService>,
    pub imported: Arc<ImportedChatReader>,
    pub runner: Arc<CliAiRunner>,
    pub ledger: PathBuf,
    pub host: OnceLock<Arc<RelayHost>>,
    /// 설정 읽기 실패 문구. 있으면 원본처럼 설정 저장을 막는다(`settingsLoaded == false`).
    pub settings_error: Option<String>,
    fallback_settings: Mutex<AiSettings>,
    pub api_status: Mutex<String>,
    pub manual: Mutex<Manual>,
    /// 수동 발송·창 확인 작업(원본 `working`). API 발송 큐와 배타적이다.
    pub native: tokio::sync::Mutex<()>,
    pub ops: Operations,
    pub shutting_down: AtomicBool,
    pub report: Mutex<Option<PathBuf>>,
    pub resume_replies: bool,
}

impl AppState {
    pub fn new(resume_replies: bool, report: Option<PathBuf>) -> Self {
        let store = Arc::new(AiSettingsStore::new(AiSettingsStore::default_path()));
        let (settings, settings_error) = match store.load() {
            Ok(settings) => (settings, None),
            Err(error) => {
                let mut settings = AiSettings::default();
                settings.ensure_personas();
                (settings, Some(format!("설정 읽기 실패: {error}")))
            }
        };
        let platform = crate::os::platform(&settings.self_id);
        let runner = Arc::new(CliAiRunner::new());
        let images = Arc::new(CliChatImageGenerator::new(
            Arc::new(kakaorelay_ai::magenta::normalize_png),
            None,
            None,
        ));
        let ai = Arc::new(AiService::new(
            platform.reader(),
            store.clone(),
            runner.clone(),
            Some(images),
            None,
        ));
        let imported = Arc::new(ImportedChatReader::new());
        let import_ai = Arc::new(AiService::new(
            imported.clone(),
            store,
            runner.clone(),
            None,
            None,
        ));
        Self {
            platform,
            ai,
            import_ai,
            imported,
            runner,
            ledger: default_ledger(),
            host: OnceLock::new(),
            settings_error,
            fallback_settings: Mutex::new(settings),
            api_status: Mutex::new("AI 연결 준비 중".into()),
            manual: Mutex::new(Manual::default()),
            native: tokio::sync::Mutex::new(()),
            ops: Operations::default(),
            shutting_down: AtomicBool::new(false),
            report: Mutex::new(report),
            resume_replies,
        }
    }

    pub fn host(&self) -> Result<&Arc<RelayHost>> {
        self.host.get().ok_or_else(|| {
            Error::invalid_operation("AI API가 시작되지 않았습니다. 앱을 다시 실행하세요.")
        })
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// 현재 설정(원본 `AiPanel.Settings`). API가 떠 있으면 `RelayHost`가 원본이다.
    pub fn settings(&self) -> AiSettings {
        match self.host.get() {
            Some(host) => host.settings(),
            None => self
                .fallback_settings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }

    /// 원본 `AiPanel.Save`. 검증·저장에 실패하면 현재 설정을 바꾸지 않는다.
    pub fn save_settings(&self, mut next: AiSettings) -> Result<()> {
        if let Some(error) = &self.settings_error {
            return Err(Error::invalid_operation(error.clone()));
        }
        match self.host.get() {
            Some(host) => host.save_settings(next),
            None => {
                self.ai.settings().save(&mut next)?;
                *self
                    .fallback_settings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = next;
                Ok(())
            }
        }
    }

    pub fn set_api_status(&self, text: &str) {
        *self.api_status.lock().unwrap_or_else(|e| e.into_inner()) = text.into();
    }

    /// 수동 발송·창 확인처럼 큐 밖의 네이티브 작업. API 발송과 겹치지 않게 조정자를 지난다.
    pub async fn exclusive<T>(&self, work: impl std::future::Future<Output = T>) -> T {
        let _manual = self.native.lock().await;
        match self.host.get() {
            Some(host) => host.coordinator().exclusive(work).await,
            None => work.await,
        }
    }
}
