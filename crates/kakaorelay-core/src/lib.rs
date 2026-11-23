//! KakaoRelay 공통 엔진. 원본: `src/KakaoRelay.Core/`의 OS 비의존 부분과 App의 서비스 조립 로직.
//! OS 의존 부분(창 열거, 입력, 첨부, 로컬 DB 키)은 `platform` trait 뒤로 분리하고
//! `kakaorelay-macos`, `kakaorelay-windows`가 구현한다. 오류는 원본 예외 분기를 보존한 `kakaorelay_ai::Error`를 쓴다.
//!
//! 모듈 구조(괄호는 원본):
//! - `platform`: OS 어댑터 계약.
//! - `test_send`: 텍스트 발송 상태 기계와 원장(`TestSend.cs`).
//! - `image_send`: 이미지 검증·사본·상태 기계(`ImageSender.cs`).
//! - `compose`: 수동 발송 작성 세션(`ComposeSession`).
//! - `work_queue`: 도착순 직렬 큐(`AsyncWorkQueue.cs`).
//! - `api_send`: API 발송 명령·예약·멱등(`ApiSendCommand`, `ApiSendService`).
//! - `relay_api`: 로컬 HTTP 게이트웨이(`RelayApi`).
//! - `delegations`: Grok 위임 작업 저장소와 작업자(`BackgroundDelegations.cs`).
//! - `daily_reply`, `readiness`, `auto_reply`, `reply_runtime`: 일일 첫 답장 기록, 시작 전 검증, 방별 자동답장 세션과 실행 관리
//!   (`DailyReplyHistory.cs`, `AutoReplyReadiness.cs`, `AutoReplySession.cs`, `ReplyRuntime.cs`).
//! - `fs`, `ids`, `support`: 원자적 쓰기·발송 잠금, ID 형식·비교, 시계·상태 알림·오류 변환.

pub mod api_send;
pub mod attachments;
pub mod auto_reply;
pub mod command_room;
pub mod compose;
pub mod daily_reply;
pub mod delegations;
pub mod diagnostics;
pub mod fs;
pub mod greetings;
pub mod host;
pub mod ids;
pub mod image_send;
pub mod periodic;
pub mod platform;
pub mod probe;
pub mod readiness;
pub mod relay_api;
pub mod reply_runtime;
pub mod summary_forwarder;
pub mod support;
pub mod test_send;
pub mod work_queue;
