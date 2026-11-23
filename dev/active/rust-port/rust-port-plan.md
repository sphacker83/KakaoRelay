# KakaoRelay Rust 이식 Plan

Created: 2026-11-23
Last Updated: 2026-11-23
Description: Port KakaoRelay from C#/.NET (WPF) to Rust with macOS and Windows support

## 요약

KakaoRelay는 카카오톡 대화방에 메시지를 보내는 로컬 API와 AI 챗봇 데스크톱 앱이다. 현재는 C#/.NET 10(WPF, Kestrel)으로 작성됐고 핵심 기능이 Win32에 묶여 있다. 맥 지원은 가져온 TXT/JSON 분석만 하는 Portable 판뿐이다. 이를 Rust 단일 코드베이스로 재작성한다. 공통 로직은 OS와 무관하게 두고, 카카오톡 읽기·발송만 OS별 어댑터로 분리한다. 목표는 macOS와 Windows에서 기존 기능을 모두 제공하는 것이다. 사용자 요구는 "러스트로 바꾸고 맥에서도 모든 기능 100% 동작"이다.

## 현재 상태 분석

전수 목록은 [rust-port-inventory.md](rust-port-inventory.md)에 있고, 맥 실증은 [rust-port-macos-research.md](rust-port-macos-research.md)에 있다.

- `src/KakaoRelay.AI/` (약 3,070줄, net10.0): 설정·페르소나·CLI 프로바이더(Codex/Claude/Grok)·상주 세션·지식베이스·자동 정리·이미지 생성. OS 비의존이다.
- `src/KakaoRelay.Core/` (약 2,260줄, net10.0-windows): 발송 원장, HTTP API, 자동답장, 정리감시, 제어방, 위임. 로직 대부분은 OS 비의존이다. 다음 파일만 Win32에 의존한다.
  - `NativeWindows`, `KakaoTestTransport`, `KakaoImageTransport`, `ConversationCatalog`
  - `LocalChatReader`의 메모리 키 스캔
  - `DiagnosticCollector`
- `src/KakaoRelay.App/` (WPF 약 1,650줄 + XAML 635줄): 490×760 고정 창, 탭 4개와 하위 패널, 트레이, 단일 인스턴스, 명령줄 모드.
- `src/KakaoRelay.Portable/`: 맥·리눅스용 브라우저 UI. 가져온 대화만 분석한다.
- `tests/` (약 2,300줄, 검사 약 430개): 자체 러너다. 테스트 실행 파일이 가짜 CLI를 겸한다.
- 맥 카카오톡은 AX에 입력창을 노출하지 않는다. 그래서 발송은 '창' 메뉴 전면화와 합성 키 입력으로 하고, DB 재조회로 결과를 확인한다(사용자 결정).

## 목표 상태

- 루트 Cargo workspace (`crates/`)
  - `kakaorelay-ai`: KakaoRelay.AI 이식
  - `kakaorelay-core`: Core의 OS 비의존 로직, axum HTTP API, 플랫폼 trait
  - `kakaorelay-platform`: `macos`/`windows` 어댑터. DB 읽기, 열린 방 목록, 텍스트·이미지 발송, 진단, 단일 인스턴스
  - `kakaorelay-app`: Tauri 2 데스크톱 앱. 정적 HTML/JS UI, 메뉴바·트레이, 명령줄 모드
  - `kakaorelay-testkit`: 가짜 CLI 바이너리와 테스트 보조
- 기존 사용자 데이터와 호환한다: `ai-settings.json`, 발송 원장, 해시 기반 경로·ID, 옵시디언 관리 영역. 해시용 직렬화는 .NET 출력과 바이트 단위로 같아야 하며, .NET으로 만든 골든 픽스처로 검증한다.
- HTTP API 경로·필드·상태 코드를 유지한다. `capabilities`의 전송 방식 값만 OS별로 실제 방식을 보고한다.
- 기존 테스트 약 430개의 동작을 `cargo test`로 옮긴다. WPF 바인딩 전용 검사 3개는 UI 검증으로 대체한다.
- 맥: 실제 카카오톡에서 '나와의 채팅'으로 읽기·텍스트·이미지·자동답장을 실기 검증한다.
- Windows: `x86_64-pc-windows-msvc` 대상으로 빌드·검사를 통과시킨다. 실제 동작 검증은 Windows PC가 필요하다.
- C# 코드는 Rust 판이 두 OS에서 검증된 뒤 삭제한다.

## Phase 실행 지도

### Phase 0 — 기반과 계약 고정

- 목표: 병렬 작업자가 같은 계약으로 시작할 수 있게 한다.
- 작업:
  - workspace·crate 골격과 `rust-toolchain.toml`(1.100.0, windows 타깃)을 만들고, 공통 의존성을 `[workspace.dependencies]`에 둔다.
  - 공통 모델(`LocalRoom`, `LocalMessage`, `ChatContext`, `ApiFailure`)과 플랫폼 trait(`ChatReader`, `RoomCatalog`, `TextTransport`, `ImageTransport`)을 정한다.
  - .NET 골든 픽스처 생성기로 해시 직렬화·설정 직렬화 기준 출력을 만든다.
  - `target/`을 `.gitignore`에 추가한다.
- Acceptance Criteria:
  - `cargo check --workspace`가 macOS와 windows 타깃에서 모두 통과한다.
  - trait 계약 문서(코드 주석)에 발송 결과 상태(`EnterPosted`/`InputCleared` 대응, 맥의 DB 확인)가 정의되어 있다.
  - 골든 픽스처가 `crates/*/tests/fixtures/`에 있다.
- 검증 게이트: `cargo check --workspace`, `cargo check --workspace --target x86_64-pc-windows-msvc`

### Phase 1 — AI 엔진 이식

- 목표: `kakaorelay-ai`가 KakaoRelay.AI의 모든 기능을 제공한다.
- 작업: inventory §1-2, §4, §5의 설정·프로바이더·상주 CLI·모델 목록·지식·자동 정리·페르소나 이미지·첨부·위임 파싱을 옮긴다. 대응 테스트를 옮긴다(AiChecks, Knowledge, StructuredKnowledge, AutoKnowledge, Persona, ImageCli, ChatTool, Instruction, Attachment, ReplyMode, ResidentCli, ModelCatalog). PDF 추출 crate는 한글 PDF로 품질을 비교해 정한다.
- Acceptance Criteria:
  - 옮긴 검사가 모두 통과한다.
  - 골든 픽스처와 해시·직렬화 출력이 일치한다.
  - 기존 `ai-settings.json`(이전 `persona` 단일 형식 포함)을 읽고 다시 써도 의미가 보존된다.
- 검증 게이트: `cargo test -p kakaorelay-ai`, `cargo clippy -p kakaorelay-ai -- -D warnings`

### Phase 2 — Core 공통 이식

- 목표: 발송 원장·API·자동답장·정리감시·제어방·위임이 가짜 어댑터로 동작한다.
- 작업: TestSend 상태 기계, ImageSender, RelayApi(axum), ApiSendService, AutoReplySession, ReplyRuntime, ReplyReadiness, DailyReplyHistory, SummaryForwarder, CommandRoomService, BackgroundDelegations, AsyncWorkQueue, DiagnosticReport·ProbeRunner를 옮긴다. ApiChecks, TestSend, AutoReply, SummaryForward, CommandRoom, Workspace, Delegation 검사와 진단 검사도 옮긴다.
- Acceptance Criteria:
  - HTTP 계약(inventory §3)의 경로·필드·상태 코드·인증 규칙이 테스트로 고정된다.
  - 옮긴 검사가 모두 통과한다.
- 검증 게이트: `cargo test -p kakaorelay-core`, clippy

### Phase 3 — macOS 어댑터

- 목표: 맥에서 카카오톡 읽기·열린 방 목록·텍스트·이미지 발송이 동작한다.
- 작업:
  - IOPlatformUUID와 사용자 ID로 키를 유도하고 SQLCipher compat 3 DB+WAL을 읽는다. NT* 스키마를 공통 모델로 바꾼다.
  - '창' 메뉴로 열린 방 목록·전면화를 구현한다.
  - CGEvent로 유니코드 입력과 Return을 보낸다.
  - 클립보드로 이미지를 붙여넣고 클립보드를 복원한다.
  - DB로 발송을 확인한다.
  - 권한 진단을 구현한다.
- Acceptance Criteria:
  - 실제 DB에서 방 목록·최근 메시지를 읽는다.
  - '나와의 채팅'에 텍스트와 PNG를 보내고, DB 확인 결과로 성공이 기록된다.
  - 동명 방과 닫힌 방은 발송 전에 차단된다.
  - 권한이 없으면 명확한 오류를 낸다.
- 검증 게이트: `cargo test -p kakaorelay-platform`. 맥 실기 검증 기록(개인 내용 제외)을 context에 남긴다.

### Phase 4 — Windows 어댑터

- 목표: 기존 Win32 동작(SQLCipher 4 + 메모리 키, EM_REPLACESEL, WM_DROPFILES, UIA 진단, named mutex)을 Rust로 옮긴다.
- 작업: `windows` crate로 NativeWindows·KakaoTestTransport·KakaoImageTransport·ConversationCatalog·LocalChatReader·DiagnosticCollector를 이식한다. CipherChecks를 이식한다.
- Acceptance Criteria:
  - windows 타깃 빌드·clippy가 통과한다.
  - OS 비의존 암호 검사가 맥에서 통과한다.
  - 실제 Windows 검증 절차서가 있다.
- 검증 게이트: `cargo clippy --target x86_64-pc-windows-msvc -p kakaorelay-platform -- -D warnings`, `cargo test -p kakaorelay-platform`(cipher). 실제 동작 검증은 Windows PC 확보 시 진행한다.

### Phase 5 — Tauri 앱

- 목표: WPF 화면의 모든 탭·패널 기능과 트레이·단일 인스턴스·종료 순서·명령줄 모드를 제공한다.
- 작업:
  - 화면: AI 챗봇(자동답장·페르소나+이미지·프로바이더·대화분석·정리감시·제어방), 메시지, API, 지식베이스(자료·연결 설정·대화 자동 정리), Portable의 TXT/JSON 가져오기.
  - 동작: 490×760 고정 창, 닫으면 숨김, 메뉴바·트레이의 열기/완전 종료.
- Acceptance Criteria:
  - inventory §2의 기능이 모두 화면에서 동작한다.
  - 스크린샷으로 탭별 표시를 확인한다.
  - 맥 `.app` 번들이 생성된다.
- 검증 게이트: `cargo build -p kakaorelay-app --release`, 앱 실행 후 화면 점검, API health 호출

### Phase 6 — 통합 검증과 정리

- 목표: 맥 전체 흐름을 실기 검증하고, 독립 리뷰와 문서 갱신을 마친다.
- 작업:
  - 맥 '나와의 채팅'으로 자동답장(호출 모드)·API 발송·이미지·정리감시 수신방 발송을 확인한다.
  - Codex Sol 독립 리뷰를 받고 지적 사항을 반영한다.
  - README·docs(ARCHITECTURE, AI, API, PORTABLE, VALIDATION)·openapi를 갱신한다.
  - Windows 실기 검증 후 C# 코드를 삭제한다.
- Acceptance Criteria:
  - 맥 실기 시나리오가 모두 통과한다.
  - 리뷰 필수 지적이 해소된다.
  - 문서가 실제 동작과 일치한다.
  - C# 삭제는 Windows 실기 통과가 확인된 경우에만 한다.
- 검증 게이트: `cargo test --workspace`, clippy(두 타깃), 맥 실기 시나리오 기록

## 리스크와 완화 전략

| 리스크 | 영향 | 완화 |
|---|---|---|
| 해시용 직렬화가 .NET과 다름 | 기존 세션·원장·상태 파일 미인식, 중복 발송 방지 무력화 | .NET 골든 픽스처 바이트 비교 테스트 |
| 맥 발송이 전면 전환·키 입력에 의존 | 사용자 키 입력 혼입, 오발송 | 발송 직전 key window 확인, 짧은 발송 창, DB 확인 실패 시 `needs-review`로 중단, 자동 재전송 금지 |
| 맥 초안 보호 불가 | 입력창에 쓰던 글과 섞여 발송 | DB·설정의 초안 저장 여부 확인 후 정책 확정, 한계 명시 |
| Windows 실기 검증 불가(맥 개발 환경) | Windows 회귀 미발견 | 타깃 빌드·로직 테스트, 검증 절차서, C# 삭제를 Windows 실기 통과 뒤로 보류 |
| PDF 텍스트 추출 품질 차이 | 지식 자료 품질 저하 | 한글 PDF 표본 비교 후 crate 선택 |
| 맥 카카오톡 업데이트로 키·스키마·메뉴 변경 | 읽기·발송 실패 | 키 HMAC 검증, 스키마 검사, 명확한 오류 |
| 권한(전체 디스크 접근·손쉬운 사용) 미부여 | 기능 불가 | 진단 화면·API에서 권한 상태와 안내 표시 |
