# Phase 4 — Windows 어댑터 작업지시서

- 워커명: `windows` (CARGO_TARGET_DIR=target/windows)
- 담당 crate: `crates/kakaorelay-windows/`만.
- 필독: `crates/kakaorelay-core/src/platform.rs`, `crates/kakaorelay-ai/src/chat_models.rs`, 원본 `src/KakaoRelay.Core/` 중 `NativeWindows.cs`, `KakaoTestTransport.cs`, `KakaoImageTransport.cs`, `ConversationCatalog.cs`, `LocalChatReader.cs`, `DiagnosticCollector.cs`, `ProbeRunner.cs`(worker 프로토콜 확인용), `TestSend.cs`, 테스트 `tests/KakaoRelay.Tests/CipherChecks.cs`, `WorkspaceChecks.cs`. inventory §1-1, §6.

## 목표
기존 Win32 동작을 `windows` crate로 1:1 이식하고 `kakaorelay_core::platform` trait을 구현한다. `Platform` 구현체(`WindowsPlatform`)를 공개한다.

## 계약과 제약
- 모든 Win32 코드는 `#[cfg(windows)]`로 둔다. OS 비의존 로직(`ChatCipher` SQLCipher 4 검증·복호화·WAL 반영, `ConversationCatalog.SelectTargets`, 메모리 키 후보 판정)은 cfg 없이 두고 맥에서 테스트한다.
- 원본 동작 유지 항목
  - 포커스·클립보드·물리 키보드 입력 금지.
  - `EM_REPLACESEL`(0x00C2), RichEdit ID 1006, `WM_KEYDOWN/UP` Enter.
  - 잠금 데스크톱 확인.
  - `WM_DROPFILES` + 확인 창 제목 목록, 대기 횟수·간격.
  - `OpenProcess/VirtualQueryEx/ReadProcessMemory` 키 스캔.
  - 64/512 창 한도.
  - UIA 진단 한도(250개, 깊이 16, 4초, 창 12개).
- trait 대응
  - `read_draft`는 `Some(text)`를 반환한다.
  - `confirm_after_enter`는 원본 `TestSender`의 입력창 비움 확인(150ms × 8회, `IsEmptyDraft` 규칙)을 옮긴다. 빈 안내 문구 `메시지 입력` 처리 규칙도 포함한다.
  - `write_draft`의 입력 응답 시간 초과는 `TransportError::InputTimeout`으로 반환한다.
- 메모리 DB 조회: 원본의 `winsqlite3.dll` P/Invoke 대신 `rusqlite`(bundled)의 deserialize를 쓴다.
- 다중 인스턴스 방지용 named mutex·event 도우미(`Local\KakaoRelay.MainWindow`, `Local\KakaoRelay.ShowWindow`)를 공개 함수로 제공한다. 앱 연결은 Phase 5에서 한다. 발송 잠금 `Local\KakaoRelay.TestSend`는 Phase 2에서 OS 공통 파일 잠금으로 바뀔 수 있으니 만들지 않는다.

## 검증
- 이 맥에서는 Windows 실행이 불가능하다. 다음을 통과시킨다.
  - `cargo test -p kakaorelay-windows`(맥, OS 비의존 로직: CipherChecks 9개와 WorkspaceChecks의 선택 필터 관련 검사 대응)
  - `cargo clippy -p kakaorelay-windows --target x86_64-pc-windows-msvc --all-targets -- -D warnings`
  - `cargo clippy -p kakaorelay-windows --all-targets -- -D warnings`
- Windows PC에서 실행할 실기 검증 절차서를 `crates/kakaorelay-windows/VERIFY.md`로 작성한다(확인할 동작, 명령, 기대 결과, 안전 조건). 실제 발송 대상은 '나와의 채팅'으로 한다.

## 완료 조건
위 검증 통과. 원본 대비 바뀐 점과 Windows 실기에서만 확인할 수 있는 위험 목록을 보고한다.
