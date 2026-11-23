//! KakaoRelay AI 엔진. 원본: `src/KakaoRelay.AI/` (C# 네임스페이스는 `KakaoRelay.Core`).
//! 이식 범위와 계약은 `dev/active/rust-port/rust-port-inventory.md` §1-2, §4, §5를 따른다.
//!
//! 모듈 구조(괄호는 원본 파일):
//! - `ai_service`: 방별 순서 보장·프로바이더 2회전 폴백·세션 재개 응답 생성(`AiService.cs`).
//! - `attachment`: 에이전트 입력용 첨부 원본과 Codex·Claude 입력 블록(`AgentAttachment.cs`).
//! - `auto_knowledge`: 대화 자동 정리 체크포인트·배치·트랜잭션(`AutoKnowledgeService.cs`).
//! - `cancel`: 취소 토큰과 시간 제한 실행(.NET `CancellationToken` 대응).
//! - `chat_image`: 답변의 이미지 생성 표시 해석과 CLI 이미지 폴백(`ChatImageGeneration.cs`).
//! - `chat_models`: 방·메시지·문맥과 `ChatReader` 계약(`ChatModels.cs`, 공통 계약).
//! - `cli_runner`: `AiRunner` 계약과 Codex·Claude·Grok CLI 실행기(`AiService.cs`의 `CliAiRunner`).
//! - `context_judge`: 맥락 답변 판단과 빠른 모델 선택(`ContextReplyJudge.cs`).
//! - `conversation`: 세션 재개 메타데이터와 메시지 지문(`AiConversation.cs`).
//! - `documents`: 보관함 노트 목록과 MD·TXT·PDF·DOCX 가져오기(`KnowledgeDocuments.cs`).
//! - `error`: .NET 예외 종류를 반영한 오류 형식.
//! - `grok_delegation`: Grok 위임 표시 해석과 작업 형식(`GrokDelegation.cs`).
//! - `image_cli`: 페르소나 이미지 CLI 생성기와 결과 검증(`PersonaImageCliGenerator.cs`).
//! - `imported_reader`: 카카오톡 TXT·JSON 내보내기 리더(`ImportedChatReader.cs`).
//! - `json`: System.Text.Json 바이트 호환 직렬화·시각 형식·해시(`PromptJson.cs`).
//! - `magenta`: 마젠타 투명화와 PNG 재인코딩(`MagentaTransparency.cs`, 앱 `MagentaPng.cs`).
//! - `model_catalog`: CLI 모델 목록 조회(`ModelCatalog.cs`).
//! - `obsidian`: 옵시디언 보관함 검색과 노트 저장(`ObsidianKnowledge.cs`).
//! - `paths`: 앱·프로바이더 데이터 경로 결정(경로 결정은 이 모듈에만 둔다).
//! - `persona_images`: 페르소나 이미지 보관함·적용 버전·생성 서비스(`PersonaImages.cs`).
//! - `process`: 자식 프로세스 실행과 프로세스 트리 종료(Unix 프로세스 그룹, Windows Job Object).
//! - `provider_storage`: 프로바이더별 CLI 홈·작업 폴더·환경 구성(`ProviderStorage.cs`).
//! - `received_attachments`: 카카오 첨부 다운로드·검증·캐시(`ReceivedAttachments.cs`).
//! - `relay_command`: 관리방 슬래시 명령과 자연어 명령 분류(`RelayCommand.cs`).
//! - `resident`: Codex app-server·Claude stream-json 상주 세션 풀(`ResidentCli.cs`).
//! - `settings`: AI 설정·페르소나·프로바이더와 저장소(`AiSettings.cs`).
//! - `structured`: 구조화 지식 색인·관리 노트·이전 형식 이관(`StructuredKnowledge.cs`).
//! - `summary`: 여러 방 전달용 요약 생성(`SummaryGenerator.cs`).
//! - `summary_forward_settings`: 정리 전달 규칙 설정(`SummaryForwardSettings.cs`).
//! - `text`: UTF-16 길이·잘라내기·대소문자 등 .NET 문자열 의미 재현.

pub mod ai_service;
pub mod attachment;
pub mod auto_knowledge;
pub mod cancel;
pub mod chat_image;
pub mod chat_models;
pub mod cli_runner;
pub mod context_judge;
pub mod conversation;
pub mod documents;
pub mod error;
pub mod grok_delegation;
pub mod image_cli;
pub mod imported_reader;
pub mod json;
mod json_table;
pub mod magenta;
pub mod model_catalog;
pub mod obsidian;
pub mod paths;
pub mod persona_images;
pub mod process;
pub mod provider_storage;
pub mod received_attachments;
pub mod relay_command;
pub(crate) mod resident;
pub mod settings;
pub mod structured;
pub mod summary;
pub mod summary_forward_settings;
pub mod text;

pub use chat_models::{ApiFailure, ChatContext, ChatReader, LocalMessage, LocalRoom};
pub use error::{Error, Result};
