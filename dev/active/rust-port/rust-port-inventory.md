# C# 기능·계약 전수 목록

Last Updated: 2026-11-23

> 이식 기준 자료. 2026-11-23 C# 코드(커밋 772283b)를 정적으로 읽어 작성했다. 코드와 다르면 코드가 우선이다.


표기: **[확인]**은 코드를 직접 읽고 확인한 사실이고, **[추정]**은 추론입니다. 위치는 `파일:줄`이며, 경로 접두어 `src/KakaoRelay.`는 생략하고 `AI/`, `Core/`, `App/`처럼 씁니다.

저장소 전체 구성입니다.
- 솔루션은 두 개입니다. `KakaoRelay.slnx`는 AI, Core, App, Tests를 묶고, `KakaoRelay.Portable.slnx`는 AI, Portable, Portable.Tests를 묶습니다.
- `Directory.Build.props:4`가 기본 TFM을 `net10.0-windows`로 지정합니다. AI와 Portable만 `net10.0`으로 덮어씁니다 (`AI/KakaoRelay.AI.csproj`, `Portable/*.csproj`).
- 앱 버전은 0.10.0입니다 (`App/KakaoRelay.App.csproj`, `Core/RelayApi.cs:135,183`).
- **네임스페이스 주의:** AI 프로젝트의 클래스도 전부 `namespace KakaoRelay.Core`입니다. 프로젝트 경계와 네임스페이스가 일치하지 않습니다.
- 코드 크기는 `src` 합계 약 7,000줄·460KB, `tests` 2,308줄입니다. **줄 수 대비 바이트가 큰 이유는 한 줄이 매우 긴 파일이 많기 때문입니다.** 줄 수만으로는 규모를 과소평가하게 되므로 아래 표에 바이트도 함께 적었습니다.

---

## 1. 파일 → 책임 요약, Windows 전용 의존

범례는 다음과 같습니다.
- **W**: Windows 전용 API에 의존합니다. Rust에서 `windows` 계열 crate나 별도 구현이 필요합니다.
- **P**: 이식 가능한 순수 로직입니다.
- **W/P**: 대부분 이식 가능하지만 일부만 Windows 분기입니다.

### 1-1. KakaoRelay.Core (2,264줄·143KB, 19개 파일)

| 파일 | 책임 | Win |
|---|---|---|
| `Core/NativeWindows.cs` | user32로 창 열거, 클래스명·제목·가시성 수집. 최상위 창 64개, 자식 창 512개 제한 | **W** (EnumWindows, GetWindowText 등, :9-22) |
| `Core/KakaoTestTransport.cs` | 대상 창 검증 후 RichEdit 컨트롤(ID 1006)에 `EM_REPLACESEL(0x00C2)`로 입력, `WM_KEYDOWN/UP`으로 Enter 게시, 잠금 데스크톱 확인 | **W** (SendMessageTimeoutW, PostMessageW, OpenInputDesktop, GetAsyncKeyState, :18-32) |
| `Core/KakaoImageTransport.cs` | `WM_DROPFILES(0x233)`로 파일 첨부 후 "사진 전송" 확인 창을 찾아 Enter | **W** (kernel32 GlobalAlloc/Lock, GetWindow GW_OWNER) |
| `Core/ConversationCatalog.cs` | 열린 대화방 목록(`EVA_Window_Dblclk` + `RICHEDIT50W` 1개 + ID 1006) 선택과 `ComposeSession` | **W** (GetDlgCtrlID, Process "KakaoTalk"). 단 `SelectTargets`는 P |
| `Core/TestSend.cs` | 영수증(`TestSendReceipt`), 지문, `TestSender.SendOnce` 상태 기계, 원장(ledger) 파일 | **W/P** (`Mutex(@"Local\KakaoRelay.TestSend")`, :78). 로직은 P |
| `Core/ImageSender.cs` | 이미지 검증(헤더 매직), 영수증 상태 기계, 전송용 사본을 Documents에 보관 | **W/P** (같은 named mutex, :45) |
| `Core/RelayApi.cs` | Kestrel HTTP 게이트웨이(Bearer, Origin 차단), `ApiSendService` 예약·멱등 | **P** (ASP.NET Core) |
| `Core/LocalChatReader.cs` | SQLCipher4 복호화, WAL 병합, 카카오 프로세스 메모리에서 키 탐색, 방·메시지 조회 | **W** — 아래 세 부분으로 나뉩니다. `ChatCipher`(:11-96)는 P입니다. `MemoryChatDb`(:99-142)는 `winsqlite3.dll`을 P/Invoke하므로 W이고, Rust는 rusqlite로 대체합니다. `KakaoMemoryKeys`(:265-325)는 OpenProcess, VirtualQueryEx, ReadProcessMemory를 쓰므로 W입니다. |
| `Core/AutoReplySession.cs` | 방별 폴링 루프, 후보 판정, 이미지→텍스트 발송 순서, 생성 이미지 분기 | P (발송은 콜백 주입) |
| `Core/AutoReplyReadiness.cs` | 열린 방 판정(로컬 방 ∩ 창), 시작 전 검증 | P |
| `Core/ReplyRuntime.cs` | 방별 `AutoReplySession` Task와 취소 관리 | P |
| `Core/DailyReplyHistory.cs` | 방별 "오늘 첫 답장" 기록 | P |
| `Core/BackgroundDelegations.cs` | Grok 위임 작업 저장소·워커 | P |
| `Core/SummaryForwarder.cs` | 정리·전달 규칙 처리, 커서, 보류 발송 | P |
| `Core/CommandRoomService.cs` | 제어방 폴러, 명령 정책, 확인번호, 감사 이력 | P |
| `Core/AsyncWorkQueue.cs` | 도착순 직렬 실행 큐(27줄) | P |
| `Core/DiagnosticCollector.cs` | UIA 트리 순회(최대 250개 컨트롤, 깊이 16, 4초, 창 12개) | **W** (`System.Windows.Automation`) |
| `Core/DiagnosticReport.cs` | 진단 보고서 스키마, `ReportStore.JsonOptions`, 원자적 저장 | P |
| `Core/ProbeRunner.cs` | 진단 worker 자식 프로세스 실행·시간 초과 시 kill | P (`--worker` 프로토콜은 App 의존) |
| `Core/GlobalUsings.cs` | `global using System.IO` | P |

### 1-2. KakaoRelay.AI (3,072줄·212KB, 21개 파일)

| 파일 | 책임 | Win |
|---|---|---|
| `AI/AiService.cs` (442줄·37KB) | `GenerateAsync`(방별 게이트, 2회전 폴백, 프롬프트 구성), `CliAiRunner`(CLI 실행·경로 탐색·인자 구성) | W/P (`.exe` 해석, `%LOCALAPPDATA%/OpenAI/Codex/bin`, :255,267) |
| `AI/ResidentCli.cs` | 상주 CLI 풀과 세션(Codex JSON-RPC, Claude stream-json), `CliJsonPipe` | W/P (`windows.sandbox`, :107) |
| `AI/ModelCatalog.cs` | 모델 목록 질의(codex/claude/grok) | P |
| `AI/ProviderStorage.cs` | 앱 로컬 CLI 홈, 환경변수 주입, 로그인 복사·동기화, 세션 이전 | W/P (`File.SetUnixFileMode`, `OperatingSystem.IsWindows`) |
| `AI/AiSettings.cs` | 설정 스키마·검증, `AiSettingsStore`(.bak). `INotifyPropertyChanged`는 WPF 바인딩용 | P (INPC는 이식 불필요) |
| `AI/AiConversation.cs` | 방별 세션 상태 `state.json`, 해시 함수 | P |
| `AI/ChatModels.cs` | `ApiFailure`, `LocalRoom/Message/ChatContext`, `IChatReader` | P |
| `AI/PromptJson.cs` | 한글 비이스케이프 인코더 | P (동일 출력 재현이 과제) |
| `AI/RelayCommand.cs` | 슬래시 명령 파서와 AI 자연어 해석 | P |
| `AI/ContextReplyJudge.cs` | "맥락 판단" 모드의 REPLY/SKIP 경주 | P |
| `AI/SummaryGenerator.cs` | 정리 요약 JSON 생성·근거 검증 | P |
| `AI/AutoKnowledgeService.cs` | 대화 자동 정리 틱, 체크포인트, 트랜잭션 | P |
| `AI/StructuredKnowledge.cs` | 보관함 구조화 노트 계획·적용·마이그레이션 | P (**lookahead 정규식** :263, 78) |
| `AI/ObsidianKnowledge.cs` | 보관함 검색·노트 저장 | P (ReparsePoint 속성 검사는 Windows 의미가 강함) |
| `AI/KnowledgeDocuments.cs` | MD/TXT/PDF/DOCX 가져오기, 목록 | P (PdfPig 의존) |
| `AI/ReceivedAttachments.cs` | 카카오 CDN 첨부 다운로드·캐시 | P |
| `AI/AgentAttachment.cs` | Codex/Claude 입력 블록 구성 | W/P (`python.exe` 경로, :16) |
| `AI/PersonaImages.cs` | 이미지 라이브러리, 카탈로그, 서비스 | P |
| `AI/PersonaImageCliGenerator.cs` | Codex/Grok 이미지 생성 CLI | W/P (Grok cwd 우회, :103) |
| `AI/ChatImageGeneration.cs` | `[[generate_image:…]]` 파싱, 채팅 이미지 생성 | P |
| `AI/GrokDelegation.cs` | `[[delegate_grok:…]]` 파싱 | P |
| `AI/MagentaTransparency.cs` | BGRA 마젠타 투명화 알고리즘 | P |
| `AI/ImportedChatReader.cs` | Portable용 TXT/JSON 가져오기 | P |
| `AI/SummaryForwardSettings.cs` | 규칙 스키마·검증 | P |

### 1-3. KakaoRelay.App (WPF, 1,655줄·101KB + XAML 635줄)

| 파일 | 책임 | Win |
|---|---|---|
| `App/App.xaml.cs` | 시작 모드 분기, 단일 인스턴스 mutex·event, 종료 순서 | **W** (WPF, `Local\KakaoRelay.MainWindow`, `Local\KakaoRelay.ShowWindow`) |
| `App/TrayHost.cs` | 트레이(WinForms `NotifyIcon`) | **W** |
| `App/MainWindow.xaml(.cs)` | 4개 탭 호스트, 메시지 발송, 이력, 진단 | **W** (WPF, Clipboard, SaveFileDialog 아님) |
| `App/AiPanel.xaml(.cs)` | AI 챗봇 탭(자동답장, 페르소나, 프로바이더, 대화분석, 정리감시, 제어방) | **W** (WPF, SaveFileDialog) |
| `App/KnowledgePanel.xaml(.cs)` | 지식베이스 탭 | **W** (OpenFolderDialog, DragDrop, obsidian:// 실행) |
| `App/AutoKnowledgePanel.xaml(.cs)` | 대화 자동 정리 설정 UI, 1분 타이머 | **W** (UI) |
| `App/PersonaImagesPanel.xaml(.cs)` | 이미지 생성·채택·적용 UI, PNG 정규화 | **W** (BitmapDecoder, PngBitmapEncoder) |
| `App/SummaryForwardPanel.xaml(.cs)` | 정리·전달 규칙 UI, 15초 타이머 | **W** (UI) |
| `App/CommandRoomPanel.xaml(.cs)` | 제어방 UI, 2초 타이머 | **W** (UI) |
| `App/MagentaPng.cs` | WPF 비트맵 ↔ BGRA 변환 | **W** (WPF imaging) |
| `App/RoomPersonaNameConverter.cs` | 목록 칩 표시용 변환기 | **W** (IMultiValueConverter) |
| `App/app.manifest` | asInvoker, PerMonitorV2 DPI | **W** |

### 1-4. Portable·Tests

| 파일 | 책임 | Win |
|---|---|---|
| `Portable/Program.cs` (78줄) | loopback Kestrel, `/api/*`, 정적 UI, 토큰은 URL fragment | P |
| `Portable/web/{index.html, app.js, style.css}` | 브라우저 UI(app.js 51줄) | P |
| `tests/KakaoRelay.Tests/*` | Core·AI 통합 검증 | **W** (Core의 Win32를 가짜 전송기로 대체, WPF 의존 :`ModelCatalog.Tests`) |
| `tests/KakaoRelay.Portable.Tests`, `tests/KakaoRelay.ModelCatalog.Tests` | AiChecks·ResidentCliChecks 재사용, 모델 목록 | Portable.Tests는 P. ModelCatalog.Tests는 **W** (WPF ComboBox 바인딩, `UseWPF`) |

---

## 2. 사용자 기능 목록

`App/MainWindow.xaml:1`: 창은 490×760 고정이고 최소화만 허용합니다 (`ResizeMode=CanMinimize`). 최상위 탭은 **AI 챗봇 → 메시지 → API → 지식베이스** 순서입니다 (:13,16,96,139).

### 2-1. MainWindow
| 기능 | 호출 서비스 | 위치 |
|---|---|---|
| 열린 대화방 새로고침(↻) | `ConversationCatalog.Scan` | `MainWindow.xaml.cs:115` |
| 선택 시 사전 점검(대상 검증, 초안이 비었거나 `메시지 입력`이어야 함) | `KakaoTestTransport.ValidateTarget/ReadDraft` | :130-149 |
| 보내기 | `ComposeSession.Begin` → `TestSender.SendOnce` | :150-170 |
| "대화 확인 완료"(관측 기록) | `TestSender.RecordObserved` | :212-225 |
| 이력 목록·새로고침·기록 폴더 열기 | `TestSender.ReadHistory` | :171-181, 277 |
| API 탭: 사용법·OpenAPI 복사 | 임베디드 `API.md`, `openapi.json` | :256-276 |
| 연결 진단 | `ProbeRunner` → 자식 프로세스 `--worker` → `DiagnosticCollector` | :226-254 |
| API 요청 발송(큐 통과) | `SendFromApiAsync`: UI `working` 상태 대기 → 직렬 | :182-210 |
| AI 답변을 발송란으로 가져오기 | `ChatbotPanel.ComposeRequested` | :81-89 |

### 2-2. AiPanel (내부 탭: 자동답장, 페르소나, 프로바이더, 대화분석, 정리감시, 제어방)

| 탭·기능 | 서비스 | 위치 |
|---|---|---|
| 자동답장: 방 선택, 봇·답변방식 "연결", 시작/중지, 폴링초, 카카오 ID | `ReplyRuntime`, `AiSettings` | `AiPanel.xaml.cs:289-297, 141-149` |
| 방 목록 2초 갱신(보일 때, 봇 미가동일 때) | `ConversationCatalog.Scan`, `ILocalChatReader.RoomsAsync` | :194-220 |
| 저장된 자동답장 방 복원(`--resume-replies`) | `ResumeSelectedRepliesAsync` | :301-310 |
| 페르소나 3개 편집(이름·역할·말투·성향·지침, 이미지 첨부) + 이미지 서브탭 | `PersonaImagesPanel` | xaml:100-139 |
| 프로바이더: CLI 설치·모델 새로고침, enabled/실행경로/모델/effort, 우선 프로바이더, 문맥수, 제한시간, `GrokDelegationEnabled` | `CliAiRunner.Status`, `ModelCatalog.QueryAsync` | :151-175 |
| 대화분석: 대화 읽기, 분석/답변/질문, JSON 저장, 발송란으로 가져오기, 요약을 노트로 저장, 취소 | `AiService.GenerateAsync`, `ObsidianKnowledge.SaveNoteAsync` | :245-288, 128-140 |
| 설정 700ms 지연 자동 저장 | `AiSettingsStore.Save` | :24, 100 |
| 제어방 명령 실행 콜백(`ExecuteCommandAsync`) | `CommandPolicy`, `ReplyRuntime` | :311-336 |

### 2-3. KnowledgePanel, AutoKnowledgePanel
- **자료 탭:** 노트 목록·미리보기(20,000자), 검색(AI 호출 없음), 자료 추가(파일당 20MB, 한 번에 50개, 끌어놓기 포함), 직접 쓰기, 옵시디언에서 편집, 폴더 열기. 서비스는 `KnowledgeDocuments`, `ObsidianKnowledge` (`KnowledgePanel.xaml.cs:100-225`).
- **연결 설정 탭:** 보관함 선택·새로 만들기(`KakaoRelay-yyyyMMdd-HHmmss`), 사용 토글, 연결 저장, 모든 봇에 적용 (:110-149).
- **대화 자동 정리 탭:** 방 체크, 사용, 주기, "확인된 내용은 바로 저장", 설정 적용, 지금 정리, 검토 폴더 열기. 서비스는 `AutoKnowledgeService.TickAsync` (`AutoKnowledgePanel.xaml.cs:92-136`).

### 2-4. PersonaImagesPanel
- 이미지 버전은 `astra`(Codex)와 `grok` 두 가지입니다 (`PersonaImages.cs:201`).
- 기능은 기준 이미지 생성, 감정별 생성·재생성, 없는 감정 모두 생성, 마젠타 투명화(허용오차 0~100, 기본 30), 채택, 버전 적용, 삭제, 프롬프트 복사, 폴더 열기입니다.
- 서비스는 `PersonaImageService`, `PersonaImageStore`, `PersonaImageCatalog`, `PersonaImageCliGenerator`입니다 (`PersonaImagesPanel.xaml.cs:95-169`).

### 2-5. SummaryForwardPanel
- 규칙 목록, 원본 방 다중 선택, 수신방 선택, 주기, 사용, 적용·삭제·새 규칙 (`SummaryForwardPanel.xaml.cs:105-137`).
- 원본·수신방이 같으면 `Validate`가 거부합니다.
- 서비스는 `SummaryForwarder.TickAsync`, `SummaryGenerator`입니다.

### 2-6. CommandRoomPanel
- 제어방 사용, 1:1 방 하나, 자연어 명령, 지식·답변·전달·프로바이더 권한 체크, 적용, 이력, 명령어 도움말 (`CommandRoomPanel.xaml.cs:42-68`).
- 서비스는 `CommandRoomService`, `CommandRoomStore`, `RelayCommandParser`입니다.

### 2-7. 트레이와 단일 인스턴스
- **트레이:** 열기·완전 종료 메뉴, 좌클릭 복원, 종료 중에는 "종료 중…"으로 비활성화됩니다 (`TrayHost.cs`).
- **창 닫기:** X는 숨기기입니다 (`App.xaml.cs:75`). 세션 종료(`SessionEnding`)만 허용합니다.
- **단일 인스턴스:** mutex `Local\KakaoRelay.MainWindow`와 event `Local\KakaoRelay.ShowWindow`를 씁니다. 두 번째 실행은 event를 `Set`하고 종료합니다 (`App.xaml.cs:58-64`).
- **종료 순서:** `PrepareShutdown` → `WaitForBotsAsync` → 작업 종료 대기 → API 종료 → `Shutdown` (`App.xaml.cs:108-123`).
- **명령줄 모드:** `--send-image <json> <out>`, `--worker <path>`, `--diagnose <path>`, `--resume-replies`, `--report <path>`이며 그 외 인자는 종료 코드 64입니다 (`App.xaml.cs:21-58`).

---

## 3. HTTP API (Core/RelayApi.cs, 포트는 127.0.0.1 임의 할당)

### 3-1. 공통 규칙 [확인]

- **리스너:** `IPAddress.Loopback:0`, 요청 본문 크기 무제한 (:114-118).
- **인증 미들웨어 (:120-134):**
  - 모든 응답에 `Cache-Control: no-store`를 붙입니다.
  - 요청에 `Origin` 또는 `Sec-Fetch-Site` 헤더가 있거나, `Authorization`이 `Bearer <토큰>`과 정확히 일치하지 않으면 **401 `{"code":"unauthorized"}`**입니다.
  - 비교는 상수 시간(`FixedTimeEquals`)입니다. 존재하지 않는 경로도 먼저 401이 됩니다.
  - 토큰은 랜덤 32바이트의 대문자 hex 64자입니다 (:109).
- **오류 매핑:**
  - `ApiFailure` → `{status, code, detail}`.
  - `JsonException`, `BadHttpRequestException`, `ArgumentException` → **400 `{"code":"invalid_request"}`** (detail 없음).
  - 그 외 모든 예외 → **500 `{"code":"unknown_result","detail":"같은 requestId로 결과를 조회하세요. 새 ID로 자동 재전송하지 마세요."}`**.
- **JSON:** HTTP 기본 옵션이라 camelCase입니다. 인코더는 `PromptJson.Encoder`로 한글을 `\uXXXX`로 이스케이프하지 않습니다.
- **연결 파일:** `%LOCALAPPDATA%/KakaoRelay/connection.json`에 `{baseUrl, token, processId, version}`을 camelCase 들여쓰기로 저장합니다. `.tmp`에 쓴 뒤 이동하는 원자적 쓰기입니다 (:183-187).
  - 종료 시 파일의 토큰이 일치할 때만 삭제하고, 서버 정지 대기는 2초입니다 (:192-205).

### 3-2. 엔드포인트

| 메서드 경로 | 조건 | 요청 → 응답 | 상태 코드 |
|---|---|---|---|
| GET `/v1/health` | 항상 | → `{status:"ready", version:"0.10.0", application:"KakaoRelay"}` | 200 |
| GET `/v1/ai/runtime` | `runtime` 콜백이 있을 때 | → `{autoReplyRunning, activeRooms, rooms[string], lastError, commandRoom{enabled,busy,status}}` (`AiPanel.xaml.cs:299`) | 200 |
| GET `/v1/capabilities` | 항상 | → 아래 3-3 | 200 |
| GET `/v1/rooms` | 항상 | → `ConversationTarget[]` `{title, processId, handle, ambiguous, hint, selectable}` | 200 |
| POST `/v1/send` | 항상 | `{requestId, recipient, message, sendDelayMs?(0~30000, 기본 1000), imagePath?, attachment?{personaId, provider(astra\|grok), imageId(32hex)}}` → `TestSendReceipt` | 200, 400, 409, 500, 503(`shutting_down`) |
| POST `/v1/send-image` | 항상 | `{requestId, recipient, imagePath? \| attachment?}` (둘 중 하나만) → `TestSendReceipt` | 위와 동일 |
| GET `/v1/requests` | 항상 | → `TestSendReceipt[]` (원장 루트 `*.json`, `startedAt` 내림차순, 본문 제외) | 200 |
| GET `/v1/requests/{id}` | 항상 | 정규식 `\A[a-zA-Z0-9_-]{1,80}\z` 검증 | 200, 400 `invalid_request_id`, 404 `receipt_unavailable` |
| GET `/v1/personas/{personaId}/images` | AI가 있을 때 | → `{personaId, appliedProvider, versions:{astra:Library, grok:Library}}` | 200, 404 `persona_unavailable` |
| GET `/v1/local/rooms` | AI가 있을 때 | → `LocalRoom[]` | 200, 409, 404 |
| GET `/v1/local/messages?profile&roomId&limit` | AI가 있을 때 | limit 기본 100, 허용 1~500 → `ChatContext{room,messages,readAt}` | 200, 400 `invalid_limit`, 404 `room_not_loaded`, 409 |
| GET/PUT `/v1/ai/settings` | AI가 있을 때 | `AiSettings` 전체 교체. PUT은 `Validate` 후 저장(200 본문 없음) | 200, 400 |
| GET `/v1/ai/providers` | AI가 있을 때 | → `CliStatus[]` `{provider, installed, path, detail}` | 200 |
| GET `/v1/ai/delegations` | AI가 있을 때 | → `DelegationJob[]` (이전 실행의 진행 중 상태는 `interrupted`로 표시) | 200 |
| POST `/v1/ai/generate` | AI가 있을 때 | `{profile, roomId, mode(analyze\|reply\|chat), instruction(≤10000), targetMessageId?}` → `AiResult` | 200, 400 `invalid_ai_request`, 409 `empty_context`, 503 `ai_unavailable` |

### 3-3. capabilities 필드
다음은 `Core/RelayApi.cs:137-145`에서 확인한 값입니다.
- `send`는 true이고, `idempotency`는 "requestId"입니다. `requiresOpenRoom`은 true, `maxMessageCharacters`는 null입니다.
- `textSend`는 `{defaultSendDelayMs:1000, maxSendDelayMs:30000, verifyInsertedText:false, continueAfterInputTimeout:true}`입니다.
- `imageSend`는 `{supported:true, endpoint:"/v1/send-image", transport:"WM_DROPFILES + HWND", computerUse:false}`입니다.
- `recentMessages`는 `{supported: ai!=null, reason}`이고 `aiChat`은 `ai!=null`입니다.
- `deliveryVerification`은 "manual"입니다.
- `endpoints` 배열은 AI가 있을 때만 확장됩니다.

### 3-4. 발송 처리 계약 [확인]
- **검증 (`ApiSendCommand.Validate`, :20-32):**
  - `sendDelayMs`가 범위를 벗어나면 400 `invalid_send_delay`입니다.
  - `requestId`, `recipient`, `message` 오류나 NUL 포함이면 400 `invalid_request`입니다. 이미지가 아니면 공백뿐인 본문도 거부합니다.
  - `imagePath`는 절대 경로여야 하고, `attachment`와 함께 쓸 수 없으며 `message`는 비어 있어야 합니다.
- **멱등 처리 (`ApiSendService.SendAsync`, :54-93):**
  1. `ledger/api/<id>.json` 예약이 있으면 지문을 대조합니다. 불일치는 409 `request_id_conflict`입니다. 영수증이 없으면 409 `uncertain`입니다. 영수증 지문이 예약의 `engineFingerprint`와 다르면 409 `uncertain`입니다. 모두 맞으면 영수증을 그대로 반환합니다.
  2. 예약은 없고 영수증만 있으면 409 `request_id_conflict`입니다.
  3. 제목이 같고 `selectable`인 방이 정확히 하나가 아니면 409 `room_unavailable`입니다.
  4. 이미지인데 어댑터가 없으면 409 `image_unavailable`입니다.
  5. `FileMode.CreateNew`로 예약을 `WriteThrough` 기록한 뒤 `SendOnce`를 호출합니다.
- **직렬화:** `ApiSendService`는 `AsyncWorkQueue` 1개로 직렬이고, App 쪽 `sendQueue`와 UI의 `working` 상태 대기가 추가로 걸립니다 (`MainWindow.xaml.cs:182-210`).
- **시간 초과:** 영수증은 `inputResponseTimedOut`이 true여도 지연 대기 후 Enter까지 진행합니다 (`TestSend.cs:114-121`).

### 3-5. Portable의 별도 API (`Portable/Program.cs`)
- **인증과 Origin:**
  - `Host`가 `127.0.0.1`이 아니면 403입니다 (:26).
  - `/api/*`는 Bearer 토큰이 필요합니다. `Origin`이 있어도 `http://<Host>`와 같으면 통과합니다. Core API와 달리 일부 Origin을 허용합니다.
  - 토큰은 URL `#fragment`로 전달합니다. 응답 헤더로 CSP(`default-src 'self'` 등)와 `X-Content-Type-Options`를 붙입니다. 본문 크기는 20MB로 제한합니다.
- **오류 형식:** `{error: 메시지}`입니다. Core의 `{code, detail}`과 다릅니다.
- **라우트:**
  - `/` → `index.html`(리소스 임베딩). `/app.js`, `/style.css`는 :41-45의 정적 라우트입니다.
  - GET·PUT `/api/settings`, GET `/api/providers`, GET `/api/models`(프로바이더별 `{choices, catalog, error}`), POST `/api/import {name, content}`, GET `/api/rooms`, GET `/api/messages?roomId&limit`, POST `/api/generate`.
  - 이 라우트들은 GET·PUT `/api/settings` 계열도 포함해 Core의 `/v1/*`와 이름·형식이 모두 다릅니다. Portable 라우트는 Core API 문서(`openapi.json`)에 없습니다.

### 3-6. RelayApi.cs와 openapi.json 대조 — 불일치

| # | 항목 | 코드 [확인] | openapi.json/문서 | 비고 |
|---|---|---|---|---|
| 1 | `AiSettings` 스키마 누락 필드 | 코드가 직렬화·수용하는 `selfId`, `trigger`, `pollSeconds`, `imageGptModel`, `imageGrokModel`, `replyMode`, `selectedBotRooms`, `selectedChatRoom` (`AI/AiSettings.cs:45-60`) | `AiSettings.properties`에 위 8개가 없음 (키 12개뿐) | **PUT은 전체 교체**이므로 문서만 따르는 클라이언트는 이 값을 기본값으로 초기화할 수 있음. 이식 시 8개 모두 계약에 포함할 것 |
| 2 | `capabilities` 스키마 누락 | `textSend`, `imageSend` 객체 출력 (:140-141) | `Capabilities.properties`에 없음 | |
| 3 | `capabilities.endpoints` | `GET /v1/ai/delegations`, `GET /v1/ai/runtime`은 배열에 없음 | 스키마는 문자열 배열만 | 실제 라우트는 존재 |
| 4 | `LocalRoom` 응답 필드 | `LocalRoom`에 `display`(계산 속성), `directMemberId`(null 가능)가 직렬화됨 (`AI/ChatModels.cs:8-12`) | `required`가 profile/id/title/readable뿐 | `AiResult.room`에도 동일 |
| 5 | `Library` 필드 | `PersonaImage`의 `isReference`, `display`가 직렬화됨 (`AI/PersonaImages.cs:16-24`) | `/v1/personas/{id}/images` 응답이 설명 문자열뿐 (스키마 없음) | |
| 6 | 409 "AI 처리 중" | 방별 `SemaphoreSlim`은 **대기**하며 409를 던지지 않음 (`AiService.cs:45-47`) | `openapi.json`과 `API.md:195`가 "AI 작업 중 409"라고 서술 | 문서 서술이 코드와 불일치로 보임 |
| 7 | 401 응답 문서화 | 모든 경로에서 401 가능 | `/v1/ai/runtime`, `/v1/personas/...`에 401·400 응답 없음 | 사소 |
| 8 | 오류 본문 | 400은 `{code:"invalid_request"}`만(detail 없음) | `Error{code, detail?}` | 일치 |
| 9 | `/v1/send` 이미지 | `message`가 빈 문자열이면 `imagePath`·`attachment` 허용 | `SendCommand.required`에 `message` 포함 | 일치 (`message:""`) |
| 10 | 탭 이름 | 실제 탭은 `API` (`MainWindow.xaml:96`) | `API.md:44`는 "연결 · API 탭" | 사소 |
| 11 | README 폴더명 | 확정 노트는 `01-프로젝트`, `02-업무지식`, `03-할일`, 근거 `04-대화기록`, 이력 `05-변경이력`, 검토 `검토필요` (`StructuredKnowledge.cs:248`). `자동정리/`는 이전 버전 마이그레이션 입력 전용 | `README.md:42`(§ 자동 정리 단락)가 `자동정리/`에 저장한다고 서술 | `docs/AI.md:59-68`은 현재 구조와 일치 |
| 12 | Grok ACP | 코드에 `acp`/`agent stdio`가 **전혀 없음** (grep 결과 0건) | `docs/AI.md:153`은 "ACP에서도 확인했으나 상주 기능은 미적용"이라고 명시 | 의뢰 문구의 "Grok(headless/ACP)" 중 ACP는 구현 대상이 아님 |
| 13 | 검증 수치 | 정적 `check(` 호출 약 430개 | `docs/VALIDATION.md`는 "Windows 90개"(v0.6.1 기준), `docs/COMMAND-ROOM-PLAN.md`는 "회귀 417 + 제어방 25" | 문서가 오래됨 |

---

## 4. 영속 파일 형식

### 4-0. 직렬화 공통 규칙 (호환성 핵심) [확인]
- `ReportStore.JsonOptions`는 들여쓰기, camelCase, `PromptJson.Encoder`입니다 (`Core/DiagnosticReport.cs:96`). 영수증, 예약, 위임 작업, 진단 보고서, 연결 파일, 제어방 일부가 이 옵션을 씁니다.
- `JsonSerializerDefaults.Web`에 들여쓰기와 `PromptJson.Encoder`를 더한 옵션을 쓰는 곳은 ai-settings, 라이브러리, 정리 상태, 제어방 설정·진행, 구조화 카탈로그입니다.
- **옵션 없이 기본값으로 쓰는 곳은 PascalCase입니다.** 다음 파일이 이에 해당합니다.
  - `DailyReplyHistory`: 필드가 `Day`, `FirstReply`입니다 (`Core/DailyReplyHistory.cs:35`).
  - `AiConversation state.json`: `SessionId`, `KnowledgeRevision`, `InstructionsDate`, `InstructionsRevision`, `Seen` (`AI/AiConversation.cs:24,33`).
  - `PersonaImageCatalog applied.json`: 문자열 하나 (`AI/PersonaImages.cs:211,221`).
- `PromptJson.Encoder`는 `JavaScriptEncoder.Create(UnicodeRanges.All)`이라 한글은 이스케이프하지 않습니다. 그래도 `< > & ' + "` 등은 이스케이프합니다 **[추정: STJ 기본 동작]**.
- **해시를 만드는 직렬화 (경로·지문·ID에 영향):** 아래는 모두 .NET 직렬화 바이트를 SHA-256으로 해시한 결과를 키로 씁니다. Rust 이식은 바이트 단위로 동일하게 재현해야 기존 데이터를 계속 인식합니다. 기본 옵션 계열은 PascalCase에 비-ASCII를 `\uXXXX`로 이스케이프하고, `PromptJson` 계열은 한글을 이스케이프하지 않습니다.
  - `ApiSendCommand.Fingerprint`: 기본 옵션, `{Recipient, Message}` 또는 `{Recipient, ImagePath}` 등 (`Core/RelayApi.cs:34-36`).
  - `TestSendRequest.Fingerprint`: 기본 옵션, `{Recipient, Message, ExpectedProcessId, ExpectedWindowHandle, VerifiedEmptyPlaceholder}` (`Core/TestSend.cs:22`). 프로세스 ID와 창 핸들이 포함되므로 재시작 간 동일성은 보장되지 않습니다.
  - `AiSettings.RoomKey`: 기본 옵션, `["profile","roomId"]` (`AI/AiSettings.cs`). 설정 딕셔너리 키와 일일 답장 파일명에 쓰입니다.
  - `AiConversation.Folder`: 기본 옵션, `{Profile, RoomId, provider(Id,Enabled,Executable,Model,Effort), persona(Name,Role,Tone,Style,Instructions)}`의 SHA-256 → `data/providers/<p>/conversations/<해시>/` (`AI/AiConversation.cs:17`).
  - `AiConversation.Fingerprint(LocalMessage)`: 기본 옵션이며 `Time`은 `DateTimeOffset`의 ISO 형식입니다 (`AI/AiConversation.cs:15`). `Seen`에 저장됩니다.
  - `SummaryForwarder.Identity`: `PromptJson`, `{Id, Sources(정렬 키 배열), Destination}` (`Core/SummaryForwarder.cs:25`).
  - `AutoKnowledgeService` 체크포인트 ID: `PromptJson`, `{Profile, RoomId, Generation, vault, SelfId}` (`AI/AutoKnowledgeService.cs:56,99`). `vault` 경로의 `\`가 JSON에서 `\\`로 이스케이프됩니다.
  - `CommandPolicy.Revision`: 기본 옵션의 `AiSettings` 직렬화 해시입니다. 확인번호의 5분 유효성에만 쓰이므로 영속 호환은 불필요합니다.
- **ID 형식:**
  - `Convert.ToHexString`은 **대문자 hex**입니다.
  - `Guid.ToString("N")`은 소문자 32자입니다.
  - 요청 ID는 `bot-`, `bot-image-`, `bot-image-error-`, `command-` 접두어에 40자 hex를 붙인 형식입니다 (`Core/AutoReplySession.cs:23`).

### 4-1. `%LOCALAPPDATA%\KakaoRelay\` 하위 (기존 사용자 데이터 호환 필요 ★)

| 경로 | 스키마 | 쓰기 규칙 | 호환 |
|---|---|---|---|
| `connection.json` | `{baseUrl, token, processId, version}` | `.tmp` → 이동. 종료 시 토큰 일치할 때만 삭제 | 실행마다 재생성 (호환 불요) |
| `ai-settings.json` | 아래 4-1-a | `.tmp`에 쓰고, **기존 파일을 `.bak`으로 복사**한 뒤 이동 (`AI/AiSettings.cs:140-146`). 저장 전 `Validate` | ★ 사용자 설정 |
| `test-sends/<requestId>.json` | `TestSendReceipt` (4-1-b) | 모든 상태 전이에서 `.tmp`(WriteThrough, flush) → 이동 (`Core/TestSend.cs:192-201`) | ★ 멱등·이력 |
| `test-sends/api/<requestId>.json` | `{fingerprint, engineFingerprint}` | `CreateNew` + WriteThrough + flush. 덮어쓰기 없음 | ★ |
| `daily-replies/<SHA256(RoomKey) 대문자 hex>.json` | `{"Day":"yyyy-MM-dd","FirstReply":"<id>"}` (PascalCase) | `.tmp` → 이동 | ★ |
| `summary-forwards/<Identity 해시>.json` | `State{cursors{key:msgId}, lastRun, history[SummaryUpdate], latestFacts{factKey:hash}, pending{requestId,text,items,cursors,dispatching}, sourceErrors}` (camelCase) | `.tmp`(UTF-8 BOM 없음) → 이동 | ★ |
| `knowledge-automation/<해시>.json` | `Checkpoint{cursor, lastRun, lastSummary, topics{}, contentHashes[]}` | `.tmp` → 이동. 읽기 크기 상한 8MB | ★ |
| `knowledge-automation/<해시>.pending.json` | `Pending{vault, batch, accepted, review, next(Checkpoint), writes[{source, before, body}]}` | 동일. 트랜잭션: pending 쓰기 → 노트 적용 → 상태 쓰기 → pending 삭제 | ★ 복구 로직 포함 |
| `command-room/settings.json` | `CommandRoomSettings{enabled, naturalLanguage, room{profile,id,title,readable,directMemberId}, operatorId, generation(guid), reply, knowledge, forward, provider}` | `Validate` 후 `.tmp` → 이동 | ★ |
| `command-room/<generation>.json` | `Progress{cursor, pending{code,command{action,target,value},revision,expires}, history[{messageId,action,result,at,before,after}]}` (이력 최대 100개) | `.tmp` → 이동 | ★ |
| `persona-images/<personaId>/<astra\|grok>/library.json` | 4-1-c | GUID 임시 파일 → 이동 | ★ |
| `persona-images/<personaId>/<astra\|grok>/<32hex>.png` | PNG 원본 | `File.WriteAllBytes` | ★ |
| `persona-images/<personaId>/applied.json` | JSON 문자열 `"astra"` 또는 `"grok"` | `.tmp` → 이동 | ★ |
| `persona-images/.../library.json.before-family-repair.bak` | 마이그레이션 백업 | 최초 1회 복사 | ★ |
| `attachments/<체크섬 또는 SHA256(url)><확장자>` | 받은 첨부 캐시 (카카오 CDN) | `WriteAllBytes` | 선택 |
| `diagnostics/diagnostic-<guid>.json` | `DiagnosticReport` | `.tmp` → 이동 | 불필요 |
| `ai-sessions/<identity>/state.json` | **이전 버전** 세션 상태 (이전 입력) | 읽기만 (`ProviderStorage.cs:108`) | ★ 한 번만 마이그레이션 |

다른 경로:
- `%LOCALAPPDATA%\Kakao\KakaoTalk\users\<40hex 프로필>\chat_data\` 아래가 **카카오톡 자체 데이터(읽기 전용)**입니다.
  - 파일은 `chatLogs_<id>.edb`, `chatListInfo.edb`와 같은 이름의 `-wal`, 그리고 프로필 폴더의 `TalkUserDB.edb`입니다 (`Core/LocalChatReader.cs:151-161`).
- **같은 이름의 폴더가 두 군데입니다.** `Documents\KakaoRelay\attachments\<guid><확장자>`는 이미지 발송용 사본입니다 (`Core/ImageSender.cs:14`). `%LOCALAPPDATA%\KakaoRelay\attachments`는 받은 첨부 캐시입니다. 혼동하지 마세요.

**4-1-a. `ai-settings.json` 스키마** [확인: `AI/AiSettings.cs`]

- `selfId`는 문자열입니다 (자동답장 시작에는 양의 BigInteger가 필요합니다).
- `trigger`는 직렬화되지만 setter가 no-op이며 값은 `"@" + persona.name`으로 계산됩니다. 읽을 때는 무시합니다.
- `pollSeconds`는 기본 5이고 범위는 1~300입니다.
- `imageGptModel`은 기본 `gpt-6-astra`, `imageGrokModel`은 기본 `grok-4.7`이며, 둘 다 정규식 `\A[a-zA-Z0-9._:/-]{1,150}\z`를 따릅니다.
- `replyMode`는 `immediate|trigger|context`이고 기본은 `trigger`입니다.
- `roomReplyModes`는 `{RoomKey: mode}` 딕셔너리입니다.
- `selectedBotRooms`는 RoomKey 문자열 배열입니다.
- `autoKnowledge`는 `{enabled, intervalMinutes(5~1440, 기본 30), autoSave(기본 true), rooms[{profile, roomId, title, generation(guid "D")}] (최대 20, 중복 금지, enabled이면 1개 이상)}`입니다.
- `summaryForwards`는 `[{id(32hex), enabled, intervalMinutes(1~1440), sources[{profile,id,title}], destination}]`입니다. 원본과 수신방은 달라야 하고 ID 중복은 금지입니다.
- `selectedChatRoom`은 문자열 또는 null입니다.
- `provider`는 `auto|codex|grok|claude`입니다.
- `contextMessages`는 1~500(기본 100), `timeoutSeconds`는 10~900(기본 180), `grokDelegationEnabled`는 기본 false, `grokTimeoutSeconds`는 10~1800(기본 600)입니다.
- `persona`와 `personas`:
  - `persona`는 1번 슬롯의 별칭입니다. `personas`가 비면 `persona`가 첫 슬롯이 됩니다.
  - `EnsurePersonas`는 기본 3개 슬롯 `default/기본`, `work/업무`, `friend/친구`를 만듭니다.
  - 페르소나는 `{id, label, persona{name, role, tone, style, instructions}, attachImages, knowledge{enabled, vaultPath}}`이며, 길이 상한 10,000자, ID 형식은 `[A-Za-z0-9_-]{1,80}`입니다.
  - **역직렬화 순서 의존:** `persona` 속성이 `personas`보다 먼저 처리되므로, 이전 형식(personas 없음)은 `legacyPersona`가 첫 슬롯으로 승격됩니다. 이 순서를 Rust에서 재현해야 합니다.
- `roomPersonas`는 `{RoomKey: personaId}`이고 존재하지 않는 ID는 오류입니다.
- `providers`는 정확히 3개(codex, grok, claude)여야 하며, 각 항목은 `{id, enabled(기본 true), executable, model(≤150자, 정규식 `\A[a-zA-Z0-9._:/\[\]-]{1,150}\z`), effort}`입니다. `effort`는 `default|none|minimal|low|medium|high|xhigh|max|ultra|ultracode` 중 하나입니다.
- 런타임 전용 `ModelChoices`, `ModelStatus`는 저장하지 않습니다 (JsonIgnore, `tests/ModelCatalog.Tests/Program.cs:66`).

**4-1-b. `TestSendReceipt` 필드** (camelCase) [확인: `Core/TestSend.cs:31-55`]
- 필드는 `requestId`, `recipient`, `fingerprint`, `status`, `detail`, `startedAt`, `finishedAt`, `enterPosted`, `inputResponseTimedOut`, `sendDelayMs`, `kind`(text|image), `attachmentQueued`, `inputCleared`(bool?), `foregroundBefore`, `foregroundAfter`입니다.
- `status`는 `prepared`, `inputting`, `dispatching`, `blocked-before-input`, `needs-review`, `observed-in-chat`, `continued-after-visual-review` 중 하나입니다.
- `DisplayTime`, `DisplayStatus`는 `JsonIgnore`입니다.
- 이력 읽기는 `파일명 == requestId`인 항목만 포함합니다. 읽기 오류는 무시합니다 (:176-190).

**4-1-c. `library.json`** [확인: `AI/PersonaImages.cs:26-31,16-24`]
- 구조는 `{referenceId, images[...], active{expression: imageId}}`입니다.
- 이미지 항목은 `{id, expression, referenceId, personaName, prompt, createdAt, provider, model, isTransparentCopy, referenceFamilyId}`입니다. 읽기 호환을 위해 `isReference`, `display`는 쓰기에 포함해도 읽을 때 무시해야 합니다.
- 표정은 `neutral, happy, sad, surprised, angry, thinking, cheering` 7가지입니다.
- 검증(`Validate`):
  - 기준 이미지는 반드시 `neutral`이어야 합니다.
  - 감정 이미지의 `referenceId`는 실제 기준 이미지를 가리켜야 합니다.
  - `active`는 같은 캐릭터 계열에서만 섞을 수 있습니다.

### 4-2. 앱 폴더 `data/` 하위 (`AppContext.BaseDirectory` 기준, .gitignore의 `**/data/`)

| 경로 | 내용 |
|---|---|
| `data/providers/<codex\|grok\|claude>/` | 프로바이더별 CLI 홈 (`CODEX_HOME`, `GROK_HOME`, `CLAUDE_CONFIG_DIR`로 주입). `.initialized-v1` 마커, `auth.json`(claude는 `.credentials.json`), `config.toml` 등 (`AI/ProviderStorage.cs:23-45`) |
| `data/providers/<p>/conversations/<해시>/state.json` | `AiConversation` (PascalCase, 위 4-0) |
| `data/providers/<p>/jobs/<kind>-<guid>/` | 일회성 작업 폴더 (끝나면 삭제) |
| `data/providers/<p>/generated-replies/<guid>.png` | 채팅 생성 이미지 (`ChatImageGeneration.cs:50`) |
| `data/providers/codex/generated_images`, `data/providers/grok/sessions` | CLI가 만드는 이미지 경로. `ValidateOutput`이 허용 루트로 검사 |
| `data/delegations/<40hex>.json` | `DelegationJob` (camelCase). `CreateNew`로 중복 방지, 상태 저장은 `.tmp` → 이동 |

복사 규칙(`AI/ProviderStorage.cs`):
- 로그인 파일은 기존 설치(`~/.codex`, `~/.grok`, `~/.claude`, 또는 환경변수가 지정한 폴더)에서 **최초 한 번만 복사**합니다 (`CopyFile`은 대상이 있으면 덮어쓰지 않음).
- Codex만 매 실행 시 `RefreshCodexLogin`으로 동기화합니다. `account_id`가 같고 원본의 `last_refresh`가 더 최신일 때만 덮어씁니다.
- 새 `config.toml`은 codex가 `cli_auth_credentials_store = "file"`, grok이 `[cli]\nauto_update = false`입니다.
- 개인 훅·플러그인·엔드포인트 설정은 상속하지 않습니다.

### 4-3. 옵시디언 보관함 `.kakaorelay` 및 노트 (사용자 보관함 호환 ★)

- `.kakaorelay/structured.json`:
  - 구조는 `Catalog{version:1, entities{id: KnowledgeEntity}, blocks{source: 관리영역 해시}, migrated[source]}`입니다.
  - 엔티티는 `{id, projectId, project, topic, kind, text, owner, due, taskState, source, profile, roomId, roomTitle, evidence, createdAt, updatedAt, changes[], tags[], aliases[]}`입니다.
- `.kakaorelay/migration.pending.json`: `KnowledgeWrite[]` 복구 기록입니다.
- **노트 폴더:**
  - `01-프로젝트/`, `02-업무지식/`, `03-할일/`, `04-대화기록/`, `05-변경이력/`, `검토필요/`, `자료/<제목>-<시각>-<guid>/NNN.md`, `KakaoRelay/<시각>-<guid>.md`.
  - 마이그레이션 입력용 이전 폴더가 `자동정리/`입니다.
  - 검색과 목록은 다음 항목을 건너뜁니다. 점(`.`)으로 시작하는 항목, 루트의 `검토필요`·`04-대화기록`·`05-변경이력`, 심볼릭 링크 항목, 마이그레이션 완료 원본.
- **관리 영역 마커:** `<!-- kakaorelay:{id}:start -->` ~ `<!-- kakaorelay:{id}:end -->`. 사용자가 이 영역을 수정했다면 해시 불일치로 쓰기를 거부하고 `IOException`을 냅니다 (`StructuredKnowledge.cs:152-163`). 이 해시 규칙(UTF-8 텍스트의 SHA-256)을 재현하지 못하면 기존 보관함의 관리 노트가 갱신되지 않습니다.
- **원자적 적용:** `Apply`는 사전에 전체를 검증한 뒤, 각 파일을 `.tmp`(GUID)에 쓰고 이동합니다. 전제 해시(`before`)가 달라졌으면 덮어쓰지 않습니다.
- **크기 제한:** 노트 256KB, 보관함 파일 2,000개, 항목 10,000개, 문자 16,000,000자, 검색 상위 6개 발췌(청크 2,000자).
- **노트 직접 저장:** 파일 크기가 256KB를 넘으면 거부합니다. `CreateNew` 후 이동하고 기존 파일은 덮어쓰지 않습니다.

### 4-4. 원자적 쓰기 규칙 요약

| 방식 | 해당 파일 |
|---|---|
| `.tmp` → 이동(덮어쓰기), flush 없음 | ai-settings(+`.bak` 복사 선행), 상태·진행·위임·진단·라이브러리 |
| `.tmp`(WriteThrough + flush) → 이동 | 영수증 |
| `CreateNew` + WriteThrough | API 예약, 위임 작업 최초 생성 |
| 쓰기 후 검증 | 이미지 전송 사본(`FileIdentity` SHA-256 재확인) |
| 읽기 시 `.tmp` 무시 | 이력 읽기는 `*.json`만 열거 |

---

## 5. 외부 프로세스 계약

공통 사항 [확인: `AI/AiService.cs:370-383`, `AI/ProviderStorage.cs:54-74`]:
- 모든 자식 프로세스는 `UseShellExecute=false`, `CreateNoWindow=true`이고 stdin·stdout·stderr를 UTF-8(BOM 없음)로 리다이렉트합니다.
- 출력은 최대 2,000,000자까지만 읽습니다.
- 환경변수 주입은 자식에만 적용하고 사용자 환경은 바꾸지 않습니다.
  - 세 개 프로바이더 홈 변수 `CODEX_HOME`, `GROK_HOME`, `CLAUDE_CONFIG_DIR`를 모두 `data/providers/<id>`로 설정합니다.
  - `CODEX_SQLITE_HOME`도 설정합니다.
  - Codex는 인자 맨 앞에 `-c sqlite_home="<홈 경로 JSON>"`을 추가합니다.
  - Claude는 `CLAUDE_CODE_PROJECT_DIR_NAME = "kakaorelay-" + SHA256(cwd)`와 `CLAUDE_CODE_TMPDIR = <홈>/tmp`를 추가합니다.
- **환경변수 제거:** `CLAUDECODE`, `CODEX_THREAD_ID`, `CODEX_INTERNAL_ORIGINATOR_OVERRIDE`를 제거합니다. 이미지 생성은 `OPENAI_API_KEY`, `XAI_API_KEY`, `GROK_API_KEY`도 제거합니다 (`AI/PersonaImageCliGenerator.cs:106`).
- 실행 파일 해석 (`AiService.cs:250-273`):
  - 설정된 경로는 환경변수를 확장하고 절대 경로여야 하며, Windows에서는 `.exe`여야 합니다.
  - 비어 있으면 `PATH`, `~/.local/bin`, `~/.grok/bin`, `%APPDATA%/npm`, `/opt/homebrew/bin`, `/usr/local/bin` 순으로 탐색합니다.
  - Codex는 Windows에서 `%LOCALAPPDATA%/OpenAI/Codex/bin` 아래 `codex.exe` 중 최신을 추가로 찾습니다.
- 종료와 취소: 취소·시간 초과 시 `Kill(entireProcessTree)`로 종료합니다.

### 5-1. Codex

**일회성 호출** (`AiService.cs:284`, `RunCoreAsync`):
- 인자:
  - `exec --skip-git-repo-check --ephemeral --ignore-user-config --sandbox read-only --color never -c approval_policy="never" -c features.shell_tool=false -c web_search="live" --output-last-message <작업폴더>/answer.txt`.
  - `--model`과 `-c model_reasoning_effort="<effort>"`는 기본값이 아닐 때만 붙고, 마지막 인자는 `-`입니다.
- 프롬프트는 **stdin으로 전달**한 뒤 stdin을 닫습니다.
- 응답은 `answer.txt` 파일에서 읽습니다. 없으면 오류입니다.

**상주 연결 (대화, `ResidentCliPool`)** [확인: `ResidentCli.cs`, `AiService.cs:317-327`]:
- 실행: `codex -c sqlite_home=… app-server --stdio -c features.shell_tool=<bool> -c web_search="live" -c approval_policy="never" -c sandbox_mode="read-only|workspace-write"`. `workspace-write`는 PDF 첨부가 있을 때입니다.
- 프로토콜은 줄 단위 JSON이며 `jsonrpc` 필드가 없습니다. 요청은 `{id(증가 정수), method, params}`입니다.
- 초기화 순서:
  1. `initialize {clientInfo{name:"kakaorelay", version:"1.0.0"}}`
  2. 알림 `initialized {}`
  3. `config/read {includeLayers:false}`
  4. `thread/start` 또는 `thread/resume`
- `thread/*`의 `params`:
  - `model`(비어 있으면 null), `cwd`, `approvalPolicy:"never"`, `sandbox`, `threadId`(resume일 때).
  - `config` 오버라이드: `features.shell_tool`, `web_search:"live"`, `sandbox_mode`, Windows+PDF일 때 `windows.sandbox:"unelevated"`, 기본이 아닌 effort일 때 `model_reasoning_effort`, 그리고 `config/read`가 돌려준 `mcp_servers.*`, `plugins.*`를 각각 `<섹션>.<이름>.enabled=false`로 끕니다. 이름에 `.`이 있으면 오류입니다.
  - 응답의 `thread.id`가 resume ID와 다르면 오류입니다.
- 턴: `turn/start {threadId, model, effort, input[]}`. `input`은 `{type:"text", text}` 블록과 첨부입니다. 첨부는 이미지가 `{type:"localImage", path}`이고 PDF는 경로를 설명하는 텍스트 블록입니다 (`AI/AgentAttachment.cs:8-24`).
- 수신 처리:
  - `item/completed`에서 `item.type=="agentMessage"`이고 `phase`가 null 또는 `final_answer`인 텍스트만 누적합니다.
  - `turn/completed`에서 `turn.status != "completed"`이면 실패입니다.
  - 응답 텍스트가 비면 "Codex 최종 응답 없음"입니다.
- **서버 → 클라이언트 요청 처리:** `id`와 `method`를 함께 가진 메시지에는 `{id, error:{code:-32601, message:"Tools and approvals are disabled for chatbot replies."}}`로 응답합니다 (`ResidentCli.cs:223-227`). 한 줄 크기 상한은 2MB입니다.
- 풀: 키는 `폴더|JSON(provider)|fileTools`입니다. 키별 게이트 1개를 두고 유휴 10분이 지나면 폐기합니다. 오류·취소·시간 초과 시 해당 세션만 폐기하며 **자동 재시도는 하지 않습니다**.

**모델 목록:** `app-server --stdio`로 `initialize` → `initialized` → `model/list {limit:100, includeHidden:false, cursor}`를 최대 20페이지 반복합니다. `result.data[].{model|id, displayName, isDefault, hidden}`과 `nextCursor`를 읽습니다 (`AI/ModelCatalog.cs:30-47`). 전체 제한은 30초입니다.

**이미지 생성 호출** (`AI/PersonaImageCliGenerator.cs:84-90`):
- 인자: `exec --skip-git-repo-check --ignore-user-config --ephemeral --sandbox read-only --enable image_generation -c approval_policy="never" -c features.shell_tool=false --model <M> -c model_reasoning_effort="low" --json --output-last-message <폴더>/answer.json [--image <기준이미지>] -`.
- 프롬프트는 stdin으로 전달하고 결과는 `answer.json`에서 읽습니다.
- 결과는 JSON `{image_path}` 또는 `{error}`입니다. 마지막 `{`부터 거꾸로 최대 128회 파싱을 시도해 추출합니다.
- 경로 검증(`ValidateOutput`):
  - 절대 경로여야 하고, 작업 폴더, `codex/generated_images`, `grok/sessions` 중 하나 아래여야 합니다.
  - 확장자는 png/jpg/jpeg/webp, 파일 크기는 24바이트~64MB입니다.
  - 마지막 수정 시각이 시작 시각 −2초 이후여야 합니다.
  - 링크(ReparsePoint) 경로는 거부하고, 기준 이미지 자체를 돌려주면 거부합니다.
- 이후 PNG로 재인코딩(WPF)하여 저장합니다. 시간 제한은 10분, 기본 모델은 `gpt-6-astra`입니다.

### 5-2. Claude

**일회성:** 인자는 `--print --output-format json --tools WebSearch,WebFetch --allowedTools WebSearch,WebFetch --permission-mode dontAsk --strict-mcp-config --mcp-config {"mcpServers":{}} --no-session-persistence --disable-slash-commands --setting-sources ""`이고, 기본값이 아니면 `--model <m>`, `--effort <e>`가 붙습니다. 프롬프트는 stdin으로 전달합니다. 응답은 `{result|response|text}` 문자열이고 `is_error==true`이면 실패입니다 (`ParseJsonOutput`, :429-441).

**상주 (대화):**
- 인자: `--print --input-format stream-json --output-format stream-json --verbose --tools WebSearch,WebFetch --allowedTools WebSearch,WebFetch --strict-mcp-config --mcp-config {"mcpServers":{}} --disable-slash-commands --setting-sources "" --permission-mode dontAsk [--model][--effort] (--session-id <새 GUID> | --resume <ID>)`.
- 입력 한 줄은 `{type:"user", session_id, message:{role:"user", content: <문자열 또는 블록 배열>}, parent_tool_use_id:null}`입니다.
- 블록 배열은 `{type:"text"}`와 `{type:"image"|"document", source:{type:"base64", media_type, data}}`로 구성합니다 (PDF는 `document`).
- `type=="result"` 이벤트까지 읽습니다. `session_id`가 일치해야 하고, `is_error==true`이거나 `subtype != "success"`이면 실패입니다.
- `control_request` 이벤트에는 `{type:"control_response", response:{subtype:"error", request_id, error:"Only pre-approved WebSearch and WebFetch tools are available."}}`로 응답합니다.

**모델 목록:** `--print --input-format stream-json --output-format stream-json --verbose --tools "" --strict-mcp-config --mcp-config {...} --no-session-persistence --disable-slash-commands --setting-sources ""`로 실행하고 `{type:"control_request", request_id:"models", request:{subtype:"initialize"}}`를 보냅니다. `request_id=="models"`인 `control_response`의 `response.response.models[].{value, displayName}`를 읽습니다 (`ModelCatalog.cs:50-58`).

### 5-3. Grok (코드상 헤드리스 전용 — ACP 아님 [확인])

**일회성** (`AiService.cs:286`):
- 프롬프트는 **파일**로 전달합니다. `<작업폴더>/prompt.txt`에 UTF-8(BOM 없음)로 쓰고 stdin은 바로 닫습니다 (:369, :389의 `provider.Id != "grok"` 분기).
- 인자: `--prompt-file <경로> --output-format json --no-auto-update --tools web_search,web_fetch --allow web_search --allow web_fetch --permission-mode dontAsk --no-subagents --max-turns 8 [--model][--effort]`.
- 첨부 파일이 있으면 작업 폴더에 사본을 만들고 프롬프트에 `첨부 원본: {...}` JSON과 안내 문장을 덧붙입니다. 그리고 `--tools` 값에 `,read_file`을 추가하고 `--allow Read(<경로>)`를 붙입니다 (:357-380).

**대화:** 상주가 아니라 요청별 실행입니다. `--session-id <새 GUID>` 또는 `--resume <ID>`를 인자 끝에 붙입니다 (`ConversationArguments`, :343). 세션 ID는 `sessionId ?? newId`로 저장되며, Grok은 응답에서 ID를 추출하지 않습니다.

**모델 목록:** `grok models` 출력에서 ANSI 색상 코드를 제거합니다. `Available models:` 이후의 `* id (default)`, `- id` 줄을 정규식으로 파싱하고, `not authenticated`가 포함되면 `loginRequired=true`입니다 (`ModelCatalog.cs:73-81`).

**이미지 생성:** 인자는 `--tools image_gen,image_edit --allow image_gen --allow image_edit --no-subagents --no-auto-update --disable-web-search --permission-mode dontAsk --model <M> --effort low --max-turns 4 --output-format json --prompt-file <폴더>/prompt.txt`입니다. Windows에서는 작업 디렉터리를 `AppContext.BaseDirectory`로 바꿉니다 (MAX_PATH 회피, :103). stderr에 `unmappable entries`가 있으면 "CLI 업데이트" 오류로 처리합니다 (:122).

### 5-4. AI 호출 규칙 (`AiService.GenerateAsync`, `AiService.cs:42-155`)
- **프롬프트:** `BuildPrompt`가 최근 메시지를 최대 60,000자까지 거꾸로 채웁니다. 일반 8,000자, 파일·사진 25,000자입니다. 삭제된 메시지는 `[삭제된 메시지]`로 바꾸고 시간은 KST(+09:00)로 표기합니다.
  - 고정 지시문은 세션당 하루 1회(날짜가 바뀌거나 지시문 해시가 바뀔 때)만 포함합니다.
- **프로바이더 순회:** 코디네이터(위임 모드의 reply)는 `codex`만 시도합니다. 그 외에는 `auto`이면 codex부터, 직접 지정이면 해당 프로바이더부터 최대 2회전(`order.Length*2`)입니다. 비활성은 건너뛰고, 사용자 취소는 즉시 전파합니다. 모두 실패하면 503 `ai_unavailable`입니다.
- **응답 후처리:** `[[emotion:*]]`, `[[generate_image:"…"]]`, `[[delegate_grok:{…}]]`를 말미에서 정규식으로 파싱합니다 (`:193`, `ChatImageGeneration.cs:12`, `GrokDelegation.cs:14`).
- **감정 이미지 첨부:** reply 모드에서 `attachImages`가 켜져 있고, 이미지 생성·위임이 없을 때 `ResolveAttachment`로 `PersonaAttachment`를 붙입니다.
- **외부 작업의 프롬프트·타임아웃:**
  - 맥락 판단 `ContextReplyJudge`: 모델 `gpt-6-luna`(codex), `grok-4.7`(grok), effort `low`. 두 프로바이더에 동시에 요청해 먼저 유효하게 답한 쪽(`REPLY`/`SKIP`)을 선택해 기억합니다. 제한은 `min(설정 타임아웃, 60초)`입니다.
  - 자연어 명령 해석 `RelayCommandParser.InterpretAsync`: 45초.
  - 요약 `SummaryGenerator`, 지식 추출 `AutoKnowledgeService.Extract`: 설정 타임아웃, 프로바이더를 순환하며 JSON 객체만 기대합니다.

---

## 6. 백그라운드 루프와 타이머

| 대상 | 주기 | 동시성 | 취소·실패 규칙 | 위치 |
|---|---|---|---|---|
| 자동답장 방별 폴링 | `pollSeconds`(기본 5, 1~300). 시작 직후 대기 | **방마다 독립 Task**, 전체 동시 제한 없음. 같은 방 AI 생성은 게이트 1개 | 방별 `CancellationTokenSource`. 취소는 해당 방만 멈춤. 읽기 범위(500) 초과 시 방 중지 후 알림 | `Core/ReplyRuntime.cs:30-38`, `Core/AutoReplySession.cs:88-133` |
| 자동답장 시작점 | 시작 시 최신 메시지 ID를 워터마크로 저장, 이후 새 메시지만 처리 | — | `selfId`는 양의 BigInteger여야 함 | `AutoReplySession.cs:94-97`, `ReplyRuntime.cs:17` |
| 후보 판정 | 새 ID, 본인 아님, 삭제 아님, 타입 1/18/2, 호출어(trigger 모드) | 메시지 ID 순 모두 처리 | 호출어는 "@봇이름" 뒤에 공백·구두점이 와야 함 | `AutoReplySession.cs:13-21` |
| 맥락 판단 | 후보마다 최근 20개 | 두 프로바이더 경주 | 판정 실패 시 해당 방 중지 | `ContextReplyJudge.cs` |
| 발송 단계 | UI·API·자동답장 공용 큐 | **직렬**. 이미지 → 텍스트 순서 고정 | 발송이 `EnterPosted && InputCleared==true`가 아니면 후속 발송 중단 | `AutoReplySession.cs:25-75` |
| 열린 방 목록 갱신 | **2초**(DispatcherTimer) | 보일 때·자동답장 미가동일 때만. 로컬 방 재탐색은 30초에 1회 | — | `AiPanel.xaml.cs:23, 194-220` |
| 정리감시 | **15초** 타이머(규칙별 주기 검사) | `SemaphoreSlim(1)`로 한 틱씩. 겹치면 건너뜀 | 설정 적용·삭제 시 진행 중 틱 취소. 수신방이 닫혀 있으면 요약을 보류하고 다음 틱에 재확인 | `SummaryForwardPanel.xaml.cs:24`, `SummaryForwarder.cs:30-45` |
| 정리감시 규칙 주기 | 규칙의 `intervalMinutes`(1~1440). `lastRun` 기준 | 규칙별 순차 | 처음 켜면 기준점만 저장. 전송 확인이 불확실하면 `dispatching` 고정, 자동 재발송 없음 | `SummaryForwarder.cs:54-56, 140-151` |
| 자동 정리(지식) | **1분** 타이머, 방별 주기(5~1440분, 기본 30)는 `lastRun`으로 검사 | `SemaphoreSlim(1)`, `Busy` 플래그. 방 순차 | 실패 시 `retryAfter` = 지금+주기까지 건너뜀(강제 실행은 무시). 배치 100개씩, 성공마다 커서 저장 | `AutoKnowledgePanel.xaml.cs:27`, `AutoKnowledgeService.cs:68-91, 133-138` |
| 제어방 폴러 | **2초** 타이머 | `SemaphoreSlim(1)`(이미 처리 중이면 즉시 건너뜀). 자동답장과 독립 | 메시지를 처리 **전에** 커서를 소비하고 이력에 "처리 중"을 기록. 중단된 실행은 재실행하지 않음. 설정 세대(generation)가 바뀌면 중단 | `CommandRoomPanel.xaml.cs:13`, `CommandRoomService.cs:146-248` |
| 제어방 확인번호 | 5분 유효, 6자리 hex(3바이트) | — | 설정이 바뀌면(Revision 불일치) 무효 | `CommandRoomService.cs:202-215` |
| 위임 대기열(Grok) | 작업 시작 즉시 `Task.Run` | **앱 전체 동시 2개**(`SemaphoreSlim(2)`). 방별 대화 게이트와는 분리 | 앱 종료·방 중지 시 취소. 발송 중 취소·실패는 `uncertain`, 그 외는 `cancelled`/`failed`. 자동 재시도 없음 | `AI/AiService.cs:36, 157-173`, `Core/BackgroundDelegations.cs:100-139` |
| 위임 제한 시간 | `grokTimeoutSeconds`(기본 600, 10~1800) | — | — | `AiService.cs:167` |
| 위임 상태 | `accepting → queued → running → finalizing → sending → completed`, 실패 시 `failed/cancelled/uncertain`, 재실행 시 이전 실행의 진행 상태는 `interrupted` | — | 같은 (프로필, 방, 메시지) 해시 ID로 `CreateNew`하므로 재시작 후 재접수 불가 | `BackgroundDelegations.cs:17-59` |
| 방별 AI 게이트 | — | `(profile, roomId)`마다 `SemaphoreSlim(1)`. 다른 방은 병렬 | 대기 중 취소는 CLI를 시작하지 않음 | `AiService.cs:35, 45-47` |
| 상주 CLI 풀 | 유휴 **10분** 후 폐기(다음 호출 시 검사) | 키별 게이트 1개 | 타임아웃은 요청별 `timeoutSeconds` | `ResidentCli.cs:28-33` |
| 진단 worker | 최대 20초 | 자식 프로세스 1개 | 시간 초과 시 `Kill(entireProcessTree)`하고 부분 결과 보존, 상태 `timeout`; 사용자 취소는 `cancelled` | `ProbeRunner.cs`, `MainWindow.xaml.cs:234` |
| 모델 목록 조회 | 30초 | 프로바이더별 병렬 | 취소가 새 조회에 의해 발생하면 이전 조회 결과는 버림 | `ModelCatalog.cs:19-20`, `AiPanel.xaml.cs:151-174` |
| 설정 자동 저장 | **700ms** 디바운스 | UI 스레드 | — | `AiPanel.xaml.cs:24, 100` |
| 이미지 전송 확인 | 창 확인 최대 30회×100ms, 닫힘 확인 50회×100ms | 직렬 | 창이 2개 이상이면 중단 | `KakaoImageTransport.cs:55-80` |
| 텍스트 발송 | Enter 후 입력창 비움 확인 8회×150ms | 직렬 | 입력 시간 초과도 대기 후 진행 | `TestSend.cs:131-136` |
| 첨부 다운로드 | HttpClient 60초 | `SemaphoreSlim(1)` | 리다이렉트 비활성. `https`, 기본 포트, `kakaocdn.net` 계열만 허용 | `ReceivedAttachments.cs:12-23` |
| 종료 순서 | — | — | `PrepareShutdown`(새 작업 차단) → 봇·전달·제어방 드레인 → 진행 중 발송 종료 대기(100ms 폴링) → API 정지(2초) → 종료 | `App.xaml.cs:108-123` |

---

## 7. 테스트

### 7-1. 러너 형태 [확인]
- **자체 `Program.cs` 러너입니다.** xUnit/NUnit 같은 프레임워크는 없습니다.
- 각 `*Checks.cs`는 `static class` + `RunAsync(string root, Action<bool,string> check)` 형태입니다. `Check(condition, description)`는 실패 시 예외를 던지고 `PASS …`를 출력합니다 (`tests/KakaoRelay.Tests/Program.cs:67`).
- 인자 없이 실행하면 전체를 순서대로 돌립니다 (:112-162). 인자로 그룹을 고릅니다.
  - `--summary-checks`: Attachment, SummaryForward, AutoReply
  - `--delegation-checks`
  - `--knowledge-checks`: Knowledge, AutoKnowledge, StructuredKnowledge
  - `--chat-tools`: ChatTool, Ai, AutoReply
  - `--api-checks`
  - `--command-checks`
- **테스트 실행 파일이 가짜 CLI를 겸합니다.** 같은 exe가 `app-server`/`--input-format`/`--output-last-message` 인자를 받으면 가짜 Codex/Claude 프로토콜을 흉내 냅니다 (`Program.cs:5-18`, `ResidentCliChecks.Fixture`). 모델 조회 테스트는 `models` 인자에 가짜 Grok 출력을 돌려줍니다 (`ModelCatalog.Tests/Program.cs:6`).
- 수동 전용 모드: `--local-read`, `--ai-smoke`, `--ai-room <방 제목>`, `--worker <경로>`(진단 worker 시험).
- 임시 폴더 `Path.GetTempPath()/KakaoRelay.Tests/<guid>`를 만들고 종료 시 삭제합니다.
- 별도 실행 파일 두 개가 더 있습니다.
  - `KakaoRelay.Portable.Tests`: `AiChecks`와 `ResidentCliChecks`를 소스 링크로 재사용하고 "portable checks"를 출력합니다.
  - `KakaoRelay.ModelCatalog.Tests`: Program.cs 하나이며 `Check` 7개입니다. `--live` 모드는 실제 CLI를 조회합니다.
- 빌드 스크립트는 `scripts/Build.ps1`이 `dotnet run --project tests\KakaoRelay.Tests`를 실행합니다.

### 7-2. 케이스 수 (정적 호출 수 추정 [추정] — `check(` 호출 횟수. 헬퍼 호출이 일부 섞여 있어 ±수 개)

| 파일 | 대략 수 | 검증하는 동작 묶음 |
|---|---|---|
| `AiChecks.cs` | 40 | TXT·JSON 가져오기(한국어 오전/오후, 모바일 형식), 설정 원자 저장·`.bak`·한글 비이스케이프, 프로바이더 폴백·2회전·취소, 프롬프트의 삭제 본문 제외, effort 인자, CLI stdin 유니코드·취소·시간 초과, 방별 병렬/직렬, 세션 폴더 격리·재개 메타데이터 |
| `ApiChecks.cs` | 41 | 인증, Origin 차단, 이미지 엔드포인트, 멱등(방 닫힘 후에도 저장 결과), ID 충돌, 불확실 예약 재발송 금지, 본문 비저장, 기본 지연 1초·0 지연, capabilities, 로컬 대화·AI 설정·delegations·generate, 연결 파일 정리, 한글 JSON |
| `AttachmentChecks.cs` | 19 | 잘못된 이미지, 원본 보존(OCR 없음), 경로 비노출, Codex/Claude 입력 형식, PDF 경로 전달, 파일 도구는 PDF 세션에서만 |
| `AutoKnowledgeChecks.cs` | 21 | 설정·세대 영속, 기준점만 저장, 주기 대기, 사람 텍스트만 요약, 근거 검증, 재시작 이어하기, 배치 처리·실패 시 커서 보존, 500개 한도 초과 보고 |
| `AutoReplyChecks.cs` | 27 | 방 선택 검증, 감정 이미지 → 텍스트 순서와 영수증 대기, 요청 ID 안정성, 불확실 시 텍스트 중단, 취소, 일일 첫 답장 이미지 규칙 |
| `ChatToolChecks.cs` | 20 | 이미지 생성 지시어 파싱, 형식 오류 거부, 폴백 2회전, 설명 발송 불확실 시 이미지 생성 중단 |
| `CipherChecks.cs` | 9 | SQLCipher 헤더·예약 바이트 복원, 변조 페이지·잘못된 키 거부, WAL 커밋 적용·미커밋 제외·손상 꼬리, 후보 판정·워터마크 |
| `CommandRoomChecks.cs` | 36 | 명령 실행·재시작 중복 방지, 자연어 미리보기·확인번호·만료·설정 변경 무효화, 권한 거부, 모호한 방, 방 변경 중단, 불확실 회신, 일반 대화 경로 |
| `DelegationChecks.cs` | 13 | 위임 작업 재제출 금지, 긴 결과 전달, 불확실 접수, 잘못된 지시어 차단 |
| `ImageCliChecks.cs` | 15 | 이미지 CLI 모델 설정 영속, 최종 JSON 추출, 인자·환경 구성 |
| `InstructionChecks.cs` | 8 | 지시문 일일 재사용, 이전 메타데이터 1회 갱신 |
| `KnowledgeChecks.cs` | 33 | 한글 조사 검색, 숨김 폴더·비MD 제외, 큰 노트 건너뜀, 발췌 한도, 페르소나별 보관함, 삭제 반영, 보관함 미연결·누락 오류 |
| `PersonaChecks.cs` | 49 | 3슬롯 마이그레이션, 방 매핑, 호출어 강제, 감정 태그 제거, 이미지 라이브러리·프롬프트·기준 일관성·후보·채택·삭제·계열 혼합 금지, 실패 시 보존 |
| `ReplyModeChecks.cs` | 18 | 폴링·방별 답변 방식 영속, 잘못된 값 거부, 가장 빠른 판정 모델 선택과 SKIP |
| `ResidentCliChecks.cs` | 14 | 상주 취소·시간 초과·장애·종료, Claude 인자와 resume, 파일 도구 샌드박스 |
| `StructuredKnowledgeChecks.cs` | 19 | 알 수 없는 ID 방지, 수동 수정 보존, 동시 수정 시 전체 트랜잭션 중단, 경로 이탈 거부 |
| `SummaryForwardChecks.cs` | 24 | 기준선, 주기, 재시작 커서, 중복 사실 억제, 보류 요약, 불확실 발송 재발송 금지, 근거 검증, 500개 초과 실패, 파일 본문 추출 |
| `TestSendChecks.cs` | 14 | Enter 1회, 중복 요청, 초안 보호, 대상 변경 시 중단, 불확실 시 재시도 금지, 만료, placeholder 규칙, 계속 전송 |
| `WorkspaceChecks.cs` | 12 | 대화방 선택 필터(메인·숨김·최소화 제외), 동명 방, 작성 세션, 이력 병합·손상 파일 허용 |
| `Program.cs`(메인) | 12 | 진단 보고서 후보 판정, worker 성공·시간 초과·취소·충돌, 오래된 성공 재사용 금지 |
| `ModelCatalog.Tests/Program.cs` | 7 | 모델 메타데이터 프로토콜·페이지네이션, Grok 미로그인 표시, 별칭 `[1m]`, WPF 선택 유지 |
| `Portable.Tests/Program.cs` | — | `AiChecks`+`ResidentCliChecks` 재실행 |
| **합계** | **약 430+** | 문서는 "417 + 제어방 25"로 서술 |

Windows 전용 테스트 의존: `ProbeRunner` 테스트는 `--worker`로 자기 자신을 실행합니다. `ModelCatalog.Tests`의 WPF 바인딩 테스트(3개)는 Rust에서 대응물이 없습니다. 발송 계열은 가짜 `ITestSendTransport`를 쓰므로 Win32 없이 이식할 수 있습니다.

---

## 8. 외부 의존성과 Rust 대체 crate 후보

NuGet 패키지는 하나뿐입니다: **PdfPig 0.1.16** (`AI/KakaoRelay.AI.csproj`). 나머지는 .NET 프레임워크 기능입니다. 라이선스 `docs/third-party/PdfPig-LICENSE.txt`를 출력 폴더에 복사합니다.

| .NET 기능·패키지 | 용도 (위치) | Rust 후보 (추정) |
|---|---|---|
| PdfPig (`ContentOrderTextExtractor`) | PDF 텍스트 추출, 200쪽 제한 (`KnowledgeDocuments.cs:71-82`) | `pdf-extract`, `lopdf`, 또는 `pdfium-render`. **읽기 순서·한글 추출 품질이 다를 수 있음(위험)** |
| `System.IO.Compression` + `System.Xml` | DOCX `word/document.xml` 파싱 (DTD 금지) | `zip` + `quick-xml` |
| ASP.NET Core / Kestrel | HTTP 게이트웨이, Portable 서버 | `axum`(tokio, hyper), `tower` |
| `CryptographicOperations.FixedTimeEquals` | Bearer 상수 시간 비교 | `subtle` |
| `RandomNumberGenerator`, `SHA256`, `SHA1`, `HMACSHA512`, `Rfc2898DeriveBytes.Pbkdf2`(SHA-512), `Aes`(CBC) | 토큰, 해시, SQLCipher 복호화 | `rand`/`getrandom`, `sha2`, `sha1`, `hmac`, `pbkdf2`, `aes`+`cbc` |
| `winsqlite3.dll` | 복호화 DB를 `sqlite3_deserialize`로 메모리에서 조회 | `rusqlite`(`serialize` + `bundled` 기능) |
| `System.Text.Json` | 모든 JSON | `serde`, `serde_json`(`preserve_order`). **STJ와 바이트 호환 직렬화기가 필요**(4-0 참조) |
| `System.Text.RegularExpressions` | 다수 정규식. **lookahead 사용:** `StructuredKnowledge.cs:78,263` | `regex` + 룩어헤드가 필요한 두 곳은 `fancy-regex` 또는 수동 파서 |
| `BigInteger` | 메시지 ID 비교 | `num-bigint` (ID가 u64 범위인지는 **[추정] 미확인**) |
| `Guid`, `DateTimeOffset`, `DateOnly`, `TimeProvider` | ID, 시각 | `uuid`, `chrono`(또는 `time`) |
| `string.Normalize()`(NFC) | 지식 키·해시 | `unicode-normalization` |
| `HttpClient` | 첨부 다운로드(리다이렉트 비활성) | `reqwest`(rustls, `redirect(Policy::none())`), URL 검증은 `url` |
| `Process` | CLI 실행·종료(프로세스 트리) | `tokio::process`, 트리 종료는 Windows Job Object(`windows` crate) 또는 `sysinfo` |
| `SemaphoreSlim`, `CancellationTokenSource`, `Task.WhenAny` | 동시성·취소 | `tokio::sync::Semaphore`, `tokio_util::sync::CancellationToken`, `tokio::select!` |
| Win32 P/Invoke (user32, kernel32) | 창 열거·메시지·메모리 읽기·DROPFILES | `windows` 또는 `windows-sys` |
| UI Automation | 진단 | `windows` crate의 `Win32::UI::Accessibility` |
| Named Mutex/Event | 단일 인스턴스, 발송 락 | `windows` crate(`CreateMutexW`, `CreateEventW`) 또는 `named_lock`, `single-instance` |
| WPF + WinForms | 화면, 트레이, 대화상자, 클립보드, 드래그앤드롭 | GUI: `tauri`, `egui`/`eframe`, `slint`, `iced` 중 선택. 트레이 `tray-icon`, 대화상자 `rfd`, 클립보드 `arboard`, URL·폴더 열기 `open` |
| `BitmapDecoder`/`PngBitmapEncoder` | PNG 정규화, 마젠타 투명화 | `image`(png 기능) |
| 경로 | `LocalApplicationData`, `MyDocuments`, `UserProfile`, `ApplicationData` | `dirs`/`directories` (경로 의미를 Windows와 동일하게 맞출 것) |
| 원자적 파일 쓰기 | `.tmp` → 이동(덮어쓰기), WriteThrough | `std::fs::rename`+`File::sync_all`, 또는 `tempfile::persist`, `atomic-write-file` |
| CLI 인자 | 시작 모드 | `clap` |

이식 시 눈여겨볼 위험 **[추정]**: (1) STJ와 동일한 해시용 직렬화(PascalCase, `\uXXXX` 이스케이프, `DateTimeOffset` 형식). (2) `.NET Normalize`, `ToLowerInvariant`와 Rust의 동작 차이. (3) PDF 텍스트 추출 품질. (4) Windows 명명된 뮤텍스 이름(`Local\…`) 의미의 보존(다른 프로세스와의 상호 배제를 위해 이름을 그대로 써야 함).

---

## 9. 이식 규모 추정 (줄 수·바이트)

줄 수는 `wc -l`, 바이트는 `wc -c`입니다. 소수점 이하는 반올림했습니다. 난이도 열은 **[추정]**입니다.

### 9-1. 프로젝트 합계
| 프로젝트 | .cs 줄 | .cs 바이트 | 기타 |
|---|---|---|---|
| Core | 2,264 | 143KB | — |
| AI | 3,072 | 212KB | — |
| App | 1,655 | 101KB | XAML 635줄 |
| Portable | 78 | 5KB | web 약 60줄 |
| Tests | 2,308 | 200KB | — |
| 문서 | README 149, docs/*.md 약 640, openapi.json 1,520 | | |

### 9-2. 파일 단위 (줄/바이트) 및 난이도

**Core**

| 파일 | 줄 | 바이트 | 난이도 |
|---|---|---|---|
| CommandRoomService | 249 | 19.5KB | 상 (밀집 코드, 상태 기계) |
| LocalChatReader | 325 | 20.4KB | 상 (암호·메모리 스캔·WAL, Windows API) |
| TestSend | 202 | 11.7KB | 중상 (상태 기계, 호환) |
| RelayApi | 206 | 14.5KB | 중상 (HTTP 계약·호환) |
| SummaryForwarder | 158 | 10.2KB | 중 |
| AutoReplySession | 134 | 10.1KB | 중 |
| BackgroundDelegations | 152 | 7.4KB | 중 |
| KakaoTestTransport | 112 | 7.3KB | 상 (Win32, 실환경 검증 필요) |
| DiagnosticReport | 106 | 6.2KB | 하 |
| KakaoImageTransport | 92 | 6.2KB | 상 (Win32) |
| DiagnosticCollector | 105 | 6.0KB | 중상 (UIA) |
| ImageSender | 86 | 5.5KB | 중 |
| ProbeRunner | 66 | 3.3KB | 하 |
| NativeWindows | 51 | 3.3KB | 중 (Win32) |
| AutoReplyReadiness | 52 | 3.2KB | 하 |
| ReplyRuntime | 47 | 2.9KB | 하 |
| ConversationCatalog | 54 | 2.7KB | 하 |
| DailyReplyHistory | 39 | 1.8KB | 하 |
| AsyncWorkQueue | 27 | 0.7KB | 하 |

**AI**

| 파일 | 줄 | 바이트 | 난이도 |
|---|---|---|---|
| AiService | 442 | 37.1KB | 상 (프롬프트 텍스트 동일성, 폴백, CLI 실행) |
| StructuredKnowledge | 272 | 20.0KB | 상 (마크다운 생성, 해시, 마이그레이션) |
| AutoKnowledgeService | 291 | 23.0KB | 상 (트랜잭션) |
| PersonaImages | 276 | 16.0KB | 중 |
| ResidentCli | 259 | 15.1KB | 상 (JSON-RPC, 프로세스 수명) |
| AiSettings | 155 | 11.3KB | 중 (검증·직렬화 호환) |
| PersonaImageCliGenerator | 151 | 11.4KB | 중 |
| ObsidianKnowledge | 160 | 9.2KB | 중 |
| ProviderStorage | 170 | 9.1KB | 중 |
| KnowledgeDocuments | 148 | 8.6KB | 중 (PDF/DOCX crate) |
| ModelCatalog | 100 | 7.5KB | 중 |
| ImportedChatReader | 103 | 6.6KB | 하 |
| ReceivedAttachments | 87 | 6.2KB | 중 |
| ContextReplyJudge | 72 | 4.9KB | 중 (경주 로직) |
| ChatImageGeneration | 63 | 4.8KB | 하 |
| RelayCommand | 60 | 4.4KB | 하 |
| SummaryGenerator | 55 | 4.1KB | 하 |
| AgentAttachment | 37 | 2.7KB | 하 |
| GrokDelegation | 27 | 2.4KB | 하 |
| MagentaTransparency | 42 | 1.9KB | 하 |
| AiConversation | 36 | 1.9KB | 하 (해시 호환 주의) |
| ChatModels | 28 | 1.5KB | 하 |
| SummaryForwardSettings | 25 | 1.4KB | 하 |
| PromptJson | 13 | 0.5KB | 하 (STJ 이스케이프 재현이 관건) |

**App** (Rust에서는 UI 프레임워크 선택에 따라 크게 달라짐)

| 파일 | 줄(.cs) | 바이트 | 비고 |
|---|---|---|---|
| AiPanel | 340 (+ xaml 243) | 22.9KB | 가장 큼 |
| MainWindow | 303 (+ xaml 145) | 18.5KB | |
| KnowledgePanel | 226 (+ xaml 83) | 14.8KB | |
| PersonaImagesPanel | 169 (+ xaml 76) | 11.3KB | |
| SummaryForwardPanel | 154 (+ xaml 29) | 8.8KB | |
| AutoKnowledgePanel | 137 (+ xaml 31) | 7.8KB | |
| App | 140 (+ xaml 18) | 6.5KB | 단일 인스턴스·시작 모드 |
| CommandRoomPanel | 69 (+ xaml 10) | 4.6KB | |
| MagentaPng | 46 | 2.6KB | `image` crate로 대체 |
| TrayHost | 42 | 1.8KB | |
| RoomPersonaNameConverter | 29 | 1.4KB | UI 의존 |

### 9-3. 이식 규모 총평 **[추정]**
- **Windows 비의존 코어(Core 일부, AI 전체)**는 약 4,900줄·330KB입니다. 호환 항목(해시 직렬화, 설정 역직렬화 순서, 마크다운·해시 규칙, 프롬프트 문구)을 지키면 로직 1:1 이식이 가능합니다.
- **Windows 의존 계층**은 약 500줄·40KB입니다. `LocalChatReader`의 메모리 스캔, `KakaoTestTransport`, `KakaoImageTransport`, `NativeWindows`, `DiagnosticCollector`가 해당하며, 줄 수는 적지만 실제 카카오톡 환경에서의 검증이 필요한 부분이라 위험이 가장 큽니다.
- **UI(App)**는 약 1,650줄·100KB + XAML 635줄이고 프레임워크 선택에 좌우됩니다. 이식하지 않는 선택지로는 Portable처럼 브라우저 UI(`axum` + 정적 페이지)로 대체하는 방법이 있습니다. 현재 Portable이 78줄이라 가장 값싼 대안이지만, 트레이·단일 인스턴스·드래그앤드롭이 사라집니다.
- **테스트**는 약 2,300줄(430+ 검사)을 `cargo test`로 옮기는 작업이 필요합니다. 가짜 CLI를 같은 실행 파일이 겸하는 방식은 Rust에서 별도 `[[bin]]`이나 `CARGO_BIN_EXE_*`로 대체해야 합니다.

가장 먼저 확정할 계약은 다음 네 가지입니다.
1. 4-0의 해시용 직렬화 바이트 호환
2. `ai-settings.json`의 8개 미문서화 필드와 persona 마이그레이션 순서
3. 영수증·예약 원장과 `daily-replies`의 PascalCase 형식
4. 옵시디언 관리 영역 마커·해시 규칙

---

## 부록: 검증 한계
- 코드는 전부 읽었으나 **실행·빌드는 하지 않았습니다** (macOS 환경에서 Windows 전용 프로젝트는 빌드되지 않음). 케이스 수는 정적 호출 횟수에서 추정한 값입니다.
- `PromptJson.Encoder`의 정확한 이스케이프 집합과 `DateTimeOffset` 직렬화 형식은 STJ 기본 동작에 대한 지식에 기반한 **[추정]**입니다. 이식 전에 실제 기존 파일과 .NET 출력을 바이트 비교로 확인하세요.
- `README.md`, `docs/AI.md`의 서술 일부는 코드와 다릅니다(3-6 참조). 이 목록은 코드를 기준으로 작성했습니다.