# Phase 5 — Tauri 앱 작업지시서

- 워커명: `app`
- 빌드 폴더: `CARGO_TARGET_DIR=target/coord`(공유)
  - 디스크 여유가 약 18GB뿐이다. 새 target 폴더나 workspace 사본을 만들지 않는다.
  - 루트 `Cargo.toml`의 `[profile.dev]` 디버그 축소 설정을 바꾸지 않는다.
- 담당 crate: `crates/kakaorelay-app/`
- 예외적으로 수정할 수 있는 곳(최소 수정만):
  - `crates/kakaorelay-core/src/platform.rs`: `ConversationTarget`에 `room_id: Option<String>`을 추가한다. `#[serde(skip)]`이고 기본값은 `None`이다. 기존 생성부가 깨지지 않게 생성자나 `Default`를 함께 둔다.
  - `crates/kakaorelay-core/src/` 안에서 `ConversationTarget` 생성부를 맞춘다.
  - `crates/kakaorelay-windows/src/`의 `ConversationTarget` 생성부를 맞춘다.
  - `crates/kakaorelay-macos/`는 `macos` 워커가 작업 중이므로 수정하지 않는다. 맥 catalog가 `room_id`를 채우는 연결은 코디네이터가 `macos` 워커에게 따로 요청한다. 앱 쪽 연결(`ConversationTarget.room_id` → 요청의 `SendTarget::room_id`)은 이 작업에서 한다.
- 원본
  - 화면·트레이·앱 수명: `src/KakaoRelay.App/` 전부(xaml과 xaml.cs). 주요 파일은 다음과 같다.
    - 창: `MainWindow`
    - 패널: `AiPanel`, `KnowledgePanel`, `AutoKnowledgePanel`, `PersonaImagesPanel`, `SummaryForwardPanel`, `CommandRoomPanel`
    - 앱 수명·트레이: `App.xaml.cs`, `TrayHost.cs`
  - 가져온 대화 UI: `src/KakaoRelay.Portable/`
  - 아이콘: `src/KakaoRelay.App/Assets/`
- 참조
  - inventory §2(사용자 기능 전체), §3(API), §6(타이머)
  - core의 `host.rs`(`RelayHost`): 앱 조립 서비스. 발송 큐·종료 순서·주기 루프·런타임 스냅숏을 담당한다.
  - [common.md](common.md)
- 결정
  - context의 "중요한 의사결정"을 따른다.
  - UI 기술: Tauri 2 + 정적 HTML/CSS/JS. Node 빌드 도구는 쓰지 않는다.
  - 맥 권한 모델: 전체 디스크 접근, 손쉬운 사용, 화면 기록(발송 필수).

## 목표

WPF 앱의 모든 화면 기능과 앱 수명 동작을 Tauri 앱으로 제공한다. 같은 바이너리가 macOS와 Windows에서 동작한다. OS 어댑터는 `cfg`로 고른다. macOS는 `kakaorelay_macos::MacPlatform`, Windows는 `kakaorelay_windows::WindowsPlatform`이다.

## 계약과 제약

- **창**
  - 490×760 고정 크기이고, 최소화만 허용한다.
  - 제목은 `KakaoRelay`다.
  - 닫기(X)는 창을 숨긴다. 세션 종료는 막지 않는다.
- **트레이·메뉴바**
  - 메뉴 항목은 "열기"와 "완전 종료"다.
  - 왼쪽 클릭하면 창을 복원한다.
  - 종료 중에는 메뉴를 "종료 중…"으로 바꾸고 비활성화한다.
  - 종료 순서는 `RelayHost` 종료 함수를 따른다.
  - 맥에서는 메뉴바 아이콘으로 표시한다. Dock 아이콘 노출 여부는 원본(작업 표시줄에 창 표시)에 맞춰 창이 보일 때만 보이게 하는 것을 권장한다. 다르게 정해야 하면 보고한다.
- **단일 인스턴스**
  - 두 번째로 실행하면 기존 창을 복원하고 종료한다.
  - Windows는 `kakaorelay_windows` instance 도우미(named mutex·event)를 쓴다.
  - 맥은 Tauri single-instance 플러그인이나 파일 잠금·소켓 중 근거 있는 방법을 쓴다.
- **명령줄 모드**
  - 원본 `App.xaml.cs:21-58`을 지킨다: `--send-image <json> <out>`, `--worker <path>`, `--diagnose <path>`, `--resume-replies`, `--report <path>`. 그 밖의 인자는 종료 코드 64다.
  - 진단 worker는 트레이·API·주 인스턴스 잠금을 만들지 않는다.
- **화면 구성**
  - 최상위 탭 순서는 AI 챗봇 → 메시지 → API → 지식베이스다.
  - 하위 탭과 컨트롤 배치·문구는 원본 XAML을 따른다. 한국어 문구는 원문 그대로 옮긴다.
  - inventory §2-1~2-6의 기능을 하나도 빠뜨리지 않는다. 다음이 모두 포함된다.
    - 자동답장: 방 목록과 봇·답변방식 칩, 연결, 시작·중지, 상태 표시
    - 페르소나 3개와 이미지 하위 탭
    - 프로바이더: 설치·모델 새로고침, effort, 위임 모드
    - 대화분석: 읽기, 분석·답변·질문, JSON 저장, 발송란으로 가져오기, 요약을 노트로 저장
    - 정리감시·제어방
    - 메시지: 방 새로고침, 발송, 대화 확인 완료, 이력, 진단
    - API: 사용법·OpenAPI 전체 표시와 복사(`docs/API.md`, `docs/openapi.json` 임베드)
    - 지식베이스: 자료(목록·미리보기·검색·추가·끌어놓기·직접 쓰기·옵시디언에서 편집), 연결 설정, 대화 자동 정리
  - Portable의 TXT/JSON 가져오기 기능도 대화분석에 넣는다. 원본 WPF에는 JSON 저장만 있다.
- **타이머**
  - 자동답장 방 목록 2초 갱신(보일 때·미가동일 때)과 로컬 방 30초 재조회를 지킨다.
  - 설정 700ms 디바운스 자동 저장을 지킨다.
  - 정리감시 15초, 제어방 2초, 자동 정리 1분은 `RelayHost` 주기 루프를 쓴다.
- **맥 권한 안내**
  - 진단 화면과 첫 실행 상태 영역에 세 권한의 상태를 표시한다. 상태는 `kakaorelay_macos::permission_status` 등으로 얻는다.
  - 시스템 설정을 여는 버튼을 둔다(`x-apple.systempreferences:` URL).
- **보안**
  - API는 core 그대로 127.0.0.1 동적 포트에 띄운다.
  - 웹뷰 UI는 HTTP API가 아니라 Tauri command로 core를 호출한다.
  - 모델 출력·대화 본문은 HTML로 해석하지 않고 텍스트로 표시한다.
  - CSP를 설정한다.
- **번들**
  - `.app`을 만든다. 맥에서 실행할 수 있어야 하고, 아이콘과 권한 설명 문자열(`NSAppleEventsUsageDescription` 등 필요한 것)을 넣는다.
  - tauri-cli가 필요하면 `cargo install tauri-cli --locked --root .tools/tauri-cli`처럼 저장소 안 `.tools/`(Git 무시)에 설치한다. 전역 설치는 하지 않는다.
  - 서명·공증은 범위 밖이다.
  - 프로바이더 데이터 경로는 ai `paths` 모듈의 결정을 따른다. 맥은 `~/Library/Application Support/KakaoRelay/data`다.
- **의존성**: windows 타깃에서 맥 교차 검사가 가능한 crate만 쓴다. `ring`·`aws-lc` 같은 C 빌드 crate는 쓰지 않는다. 피할 수 없으면 보고한다.

## 실기 확인 (화면)

- 앱을 실행해 탭·하위 탭마다 스크린샷을 찍어 확인한다. 저장 위치는 `tmp/screens/`다. 개인 대화 내용이 보이는 화면은 저장하지 않거나 즉시 지운다. 보고에는 파일 경로만 적는다.
- 확인할 동작: 창 닫기 → 트레이 → 열기, 완전 종료, 두 번째 실행 시 기존 창 복원, API health 호출 성공.
- **실제 카카오톡 발송 버튼을 누르지 않는다.** 실제 발송 검증은 `macos` 워커와 코디네이터가 맡는다.
- 화면 잠금을 만나면 즉시 멈추고 코디네이터에게 보고한다. 암호 입력·잠금 해제를 직접 시도하지 않는다.

## 완료 조건

- inventory §2의 기능별 구현 여부를 표로 보고한다(원본 위치 → 앱 구현 위치).
- `cargo build -p kakaorelay-app`과 `.app` 번들 생성이 성공한다.
- `cargo clippy --workspace --all-targets -- -D warnings`가 통과한다.
- `cargo check -p kakaorelay-app --target x86_64-pc-windows-msvc`가 통과한다. 실패하면 원인과 대안을 보고한다.
- 스크린샷 확인 결과를 보고한다.
- 기존 테스트가 계속 통과한다: `cargo test --workspace`.
