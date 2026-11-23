# Phase 2 — Core 공통 이식 작업지시서

- 워커명: `core`
- 빌드 폴더: `CARGO_TARGET_DIR=target/coord`. 디스크 여유가 적어 코디네이터와 같은 폴더를 공유한다. 새 target 폴더나 workspace 사본을 만들지 않는다.
- 담당 crate: `crates/kakaorelay-core/`
- 예외적으로 수정 가능한 범위:
  - `crates/kakaorelay-windows/src/catalog.rs`·`diagnostics.rs`: 진단 스냅숏 타입을 core로 옮길 때 import를 바꾸는 최소 수정만 한다.
  - `crates/kakaorelay-ai/tests/checks/`: Phase 2로 넘어온 18개 검사를 core 테스트로 옮기면서 남은 자리 표시를 정리한다.
- 원본:
  - `src/KakaoRelay.Core/` 중 OS 비의존 파일
    - 발송: `TestSend.cs`, `ImageSender.cs`, `RelayApi.cs`
    - 자동답장: `AutoReplySession.cs`, `AutoReplyReadiness.cs`, `ReplyRuntime.cs`, `DailyReplyHistory.cs`
    - 정리감시·제어방·위임: `SummaryForwarder.cs`, `CommandRoomService.cs`, `BackgroundDelegations.cs`
    - 공용·진단: `AsyncWorkQueue.cs`, `DiagnosticReport.cs`, `ProbeRunner.cs`, `ConversationCatalog.cs`의 `ComposeSession`
  - App 쪽에 있는 서비스 조립 로직(예: `MainWindow.xaml.cs:182-210` API 발송 큐, `AiPanel.xaml.cs:289-336` 자동답장 시작·제어방 명령 실행, `App.xaml.cs:108-123` 종료 순서)은 화면과 분리해 core 서비스로 옮긴다. 화면 자체는 Phase 5다.
- 테스트 원본:
  - `tests/KakaoRelay.Tests/`의 다음 검사: TestSendChecks, WorkspaceChecks(ComposeSession·원장), ApiChecks, AutoReplyChecks, SummaryForwardChecks, CommandRoomChecks, DelegationChecks
  - `Program.cs`의 진단 worker 검사
  - CipherChecks의 자동답장 후보 검사 3개
  - `ai` 워커가 넘긴 18개: PersonaChecks 5, AttachmentChecks 2, ReplyModeChecks 4, ChatToolChecks 7
- 참조: inventory §1-1, §2, §3(HTTP 계약 전체), §4(원장·상태 파일), §6, §7. 공통 규칙 [common.md](common.md).

## 목표

Core의 OS 비의존 동작을 `kakaorelay-core`로 이식하고, 가짜 어댑터로 모든 대응 검사를 통과시킨다. 앱(Phase 5)은 core 서비스만 조립하면 되도록 공개 API를 만든다.

## 계약과 제약

- **플랫폼 계약** `crates/kakaorelay-core/src/platform.rs`
  - 이 파일은 이제 core 소유다. 필요하면 수정하되, 기존 어댑터(macos·windows) 컴파일을 깨지 않는 방향(기본 구현 추가)으로만 바꾸고 보고한다.
  - 현재 계약: `SendTarget`의 초안 규칙 메서드와 `room_id()`, `TransportError::{InputTimeout, PermissionDenied{code,detail}, Failed}`, `read_draft -> Option`, `confirm_after_enter`.
- **발송 상태 기계** `TestSender.SendOnce`, `ImageSender.SendOnce`
  - 원본 순서를 유지한다: 원장 저장 → 대상 확인 → 초안 검사 → `inputting` 저장 → 입력 → 지연 또는 되읽기 → 대상 재확인 → `dispatching` 저장 → Enter → `needs-review` 저장 → 확인.
  - 상태 문자열과 문구를 유지한다.
  - `read_draft`가 `None`인 OS의 처리:
    - 초안 검사와 되읽기를 건너뛴다.
    - 되읽기 대신 지연 경로를 쓴다. 지연이 없으면 기본 1000ms를 쓴다.
    - 원장 `detail`에 그 사실을 남긴다.
  - 맥은 사용자 결정으로 Cmd+A·Cmd+C 초안 검사를 구현 중이라 `Some`을 돌려줄 수 있다. 두 경우 모두 동작해야 한다.
- **발송 잠금**: 원본 named mutex `Local\KakaoRelay.TestSend`는 OS 공통 파일 잠금(`std::fs::File::try_lock`, 원장 폴더의 잠금 파일)으로 바꾼다. 프로세스 사이 상호 배제와 비차단 획득 실패 문구를 유지한다.
- **원장 형식**: inventory §4-1-b와 골든을 따른다.
  - 영수증 `*.json`은 camelCase이고 WriteThrough+flush 후 이동한다.
  - API 예약 `test-sends/api/<id>.json`은 CreateNew로 만든다.
  - 지문 해시는 골든 `testSendFingerprint`, `apiFingerprint.*`와 바이트로 같아야 한다.
  - `daily-replies`(PascalCase, 골든 `dailyReplyEntry`), `summary-forwards`(골든 `summaryIdentity`, `summaryFactKey`), `command-room`, `delegations` 형식을 유지한다.
  - 경로 함수는 `kakaorelay-ai`의 경로 모듈을 재사용한다.
- **HTTP API** (axum)
  - inventory §3의 경로·필드·상태 코드·인증을 모두 지킨다.
    - 인증: Bearer 상수 시간 비교, `Origin`/`Sec-Fetch-Site` 거부, 존재하지 않는 경로도 401
    - 응답: `Cache-Control: no-store`
    - 오류: 400/500 고정 본문
    - 연결: `connection.json` 원자적 쓰기, 토큰이 일치할 때만 삭제
    - 직렬화: 한글 비이스케이프 JSON
  - `capabilities.imageSend.transport`는 `Platform::image_transport_name()`을 쓴다.
  - `TransportError::PermissionDenied`는 409 `{code, detail}`로 응답한다.
  - inventory §3-6의 문서 불일치는 **코드 동작 기준**으로 유지한다. 예: `AiSettings` 미문서 필드 8개 포함.
- **`read_attachment` 공통 처리**: `ChatReader`를 감싸 `ReceivedAttachments`(kakaorelay-ai)로 첨부 원본을 채우는 래퍼를 core에 둔다. 두 OS 어댑터가 공유한다.
- **열린 방과 DB 방 연결**: 맥 어댑터는 창과 DB 방을 연결할 수 있다. 내부용으로 `ConversationTarget`에 `room_id: Option<String>`을 추가할 수 있다. 단, API `GET /v1/rooms` 응답 형태는 원본과 같게 유지한다(직렬화 제외). 발송 요청 타입은 이 값을 `SendTarget::room_id()`로 넘긴다.
- **진단**
  - 진단 스냅숏 타입(`WindowSnapshot`, `NativeControlSnapshot`, `AutomationControlSnapshot`, `ProcessSnapshot`, 현재 `crates/kakaorelay-windows/src/catalog.rs`)과 `DiagnosticReport`를 core로 옮긴다. JSON 필드명·순서를 유지한다.
  - `ProbeRunner`의 `--worker` 자식 프로세스 프로토콜은 앱 바이너리 이름을 인자로 받게 일반화한다.
- **비동기**
  - tokio를 쓴다.
  - 발송 transport는 블로킹이므로 `spawn_blocking`에서 실행하고, 발송은 전역 직렬 큐(원본 `AsyncWorkQueue` + `sendQueue`)를 거친다.
  - 루프 주기(자동답장 폴링, 정리감시 15초, 제어방 2초, 자동 정리 1분, 위임 동시 2개)와 취소 규칙은 inventory §6 그대로다.
  - 주기 타이머는 core 서비스가 소유하고 앱은 시작·중지만 호출하게 한다.
- **종료 순서**: 새 작업 차단 → 봇·전달·제어방 드레인 → 진행 중 발송 종료 대기 → API 정지(2초). 원본 `App.xaml.cs:108-123`을 core의 종료 함수로 제공한다.

## 완료 조건

- 위 원본 검사가 모두 Rust 테스트로 존재하고 통과한다. 묶음별 원본 수와 Rust 수 대조표를 보고한다.
- 골든 비교 테스트를 core에도 둔다: `testSendFingerprint`, `apiFingerprint.*`, `summaryIdentity`, `summaryFactKey`, `dailyReplyEntry`, `reportOptions.time`.
- 실제 HTTP 서버를 띄운 통합 테스트로 인증·오류 본문·멱등을 확인한다.
- 검증 명령:
  - `cargo test -p kakaorelay-core -p kakaorelay-ai -p kakaorelay-windows -p kakaorelay-macos`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings`
- 실제 카카오톡 발송은 하지 않는다. 가짜 transport만 쓴다.
