# KakaoRelay Rust 이식 Context

Last Updated: 2026-11-23

## Current Execution Contract

- 유효 plan: rust-port-plan.md
- Active Phase: Phase 5(워커 `app`)·Phase 3(워커 `macos`, 화면 기록 기반 대상 확인 수정 후 실기 발송) 병렬 진행
- Active Task: T-5.x, T-3.2~3.3 (작업지시서 `briefs/`)
- 완료 조건: 각 작업지시서의 완료 조건과 검증 명령 통과, 결과 회수·통합 확인
- 금지 사항:
  - 실제 카카오톡 발송은 '나와의 채팅'에서만 한다.
  - 대화 내용·키·사용자 ID를 로그·문서·Git에 남기지 않는다.
  - Windows 실기 검증 전에는 C#을 삭제하지 않는다.
  - 커밋은 사용자 요청이 있을 때만 한다.

## 빠른 재개 안내

- 재시작 시 바로 실행할 명령: `cargo check --workspace && cargo test --workspace`
- 현재 blocker: 없음. Windows 실기 검증에는 Windows PC가 필요하다.
- 남아 있는 임시 우회책: 없음

## 다음 세션 읽기 순서

1. 이 파일의 현재 계약·빠른 재개 안내·최근 SESSION PROGRESS
2. rust-port-plan.md의 해당 Phase
3. rust-port-tasks.md의 active task
4. 필요 시 [rust-port-inventory.md](rust-port-inventory.md)(C# 기능·계약 전수 목록), [rust-port-macos-research.md](rust-port-macos-research.md)(맥 연동 실증)

## SESSION PROGRESS

### 2026-11-23 — Phase 2 회수, Phase 5 배정

- 완료: `core` 결과 회수. 코디네이터 통합 검증에서 workspace 테스트(ai 261, core 202, 골든 10, macos 15, windows 15)와 두 타깃 clippy가 모두 통과했다(`tmp/integration-3.log`).
- 맥 실기: '창' 메뉴 체크 표시가 key window 판정에 쓸 수 없음을 확인했다. 사용자 결정으로 화면 기록 권한을 발송 필수로 바꿨고, `macos` 워커가 수정 중이다. 발송은 아직 0회다.
- 환경: 루트 `Cargo.toml`에 `[profile.dev]` 디버그 축소 설정을 추가했다(디스크 절약). `target/coord`는 비웠다.
- 다음: `app`(Opus 5.5 medium, 화면 캡처 반복 UI 작업) 결과를 회수한다. 맥 실기 결과를 회수한다. 맥 catalog가 `ConversationTarget.room_id`를 채우도록 `macos`에 요청한다.

### 2026-11-23 — Phase 1·4 회수와 통합 검증, Phase 2 배정

- 완료: `ai` 결과 회수(검사 261개+골든 4개). `windows` 결과 회수(테스트 15개). 코디네이터 통합 검증에서 두 타깃 workspace clippy 통과(`tmp/integration-2.log`). AI 테스트의 Unix 전용 함수 `running_now`에 `cfg(unix)`를 붙여 windows 타깃 clippy를 해소했다.
- 결정: 맥 초안 보호는 입력 전 Cmd+A·Cmd+C 검사 후 클립보드 복원(사용자 결정). 화면 기록 권한은 선택 권한(사용자 결정). 계약에 `SendTarget::room_id`와 `TransportError::PermissionDenied`를 추가했다. 발송 잠금은 named mutex 대신 OS 공통 파일 잠금으로 한다. 메시지 `time`은 원본처럼 UTC(+00:00)다(코디네이터 KST 지시를 철회).
- 환경: 디스크 여유 부족(워커별 target 폴더로 가득 참). 끝난 워커의 target을 지웠고, 이후 빌드는 `target/coord` 하나를 공유한다. `tmp/`를 `.gitignore`에 추가했다.
- 다음: `core` 결과 회수, `macos` 실기 발송 결과 회수, 이어서 Phase 5(Tauri 앱) 배정.

### 2026-11-23 — Phase 0 완료, 병렬 배정

- 완료: Cargo workspace(`crates/kakaorelay-{ai,core,macos,windows,app,testkit}`), `rust-toolchain.toml` 1.100.0, 플랫폼 trait(`crates/kakaorelay-core/src/platform.rs`), .NET 골든 픽스처 19건(`crates/kakaorelay-ai/tests/fixtures/dotnet-golden.json`, 생성기 `tools/golden/`).
- 결정: OS 어댑터를 crate 두 개로 분리해 병렬 편집 충돌을 막았다. 맥은 입력창을 읽을 수 없어 `TextTransport::read_draft`가 `Option`이고, Enter 이후 확인은 `confirm_after_enter`로 OS별 구현한다.
- 배정: 워커 `ai`(Opus 5.5 medium, 대형 이식이지만 명세가 분명), `macos`(Opus 5.5 medium, 실환경 탐색 포함), `windows`(Opus 5.5 medium)를 병렬로 배정했다. 근거는 ai-team routing의 "명세가 분명한 마이그레이션 → Opus"다.
- 다음: 결과를 회수해 통합 확인한다. 이어서 Phase 2(Core)를 배정한다.

### 2026-11-23 — 조사·결정·계획 수립

- 완료: C# 기능·계약 전수 목록 작성. 맥 DB 복호화 실증. 맥 AX 미노출 확인과 '창' 메뉴 경로 발견. Dev Docs 작성.
- 결정: 아래 "중요한 의사결정" 참조.
- 다음: Phase 0 골격 작성 후 Phase 1(AI)·Phase 3(맥 어댑터)·Phase 4(Windows 어댑터)를 병렬 배정한다.

## 핵심 파일과 역할

- `src/KakaoRelay.AI/*` — Phase 1 이식 원본
- `src/KakaoRelay.Core/*` — Phase 2(공통)·Phase 4(Win32) 이식 원본
- `src/KakaoRelay.App/*` — Phase 5 화면 원본
- `src/KakaoRelay.Portable/web/*` — 가져오기 UI 원본
- `tests/KakaoRelay.Tests/*` — 이식할 동작 검사
- `crates/*` — Rust 구현
- `tools/golden/` — .NET 직렬화 기준 출력 생성기(`dotnet run --project tools/golden -- <출력>`; 이 폴더의 global.json이 설치된 SDK 10.0.1xx를 허용)
- `briefs/` — 워커 작업지시서

## 중요한 의사결정

- OS 범위는 맥+Windows 동시(사용자 결정 2026-11-23). Windows는 이 맥에서 타깃 빌드까지만 확인하고, 실기 검증은 Windows PC에서 한다.
- C#은 Rust 검증 후 삭제한다(사용자 결정). 기각한 대안은 즉시 삭제와 계속 병행이다.
- UI는 Tauri 2와 정적 HTML/JS로 만든다(사용자 결정). Node 빌드 도구는 쓰지 않는다. 기각한 대안은 egui와 로컬 브라우저 UI다.
- 맥 발송은 '창' 메뉴 AXPress 전면화, CGEvent 유니코드 입력, Return, DB 재조회 확인으로 한다(사용자 결정). 맥 카카오톡이 입력창을 AX에 노출하지 않기 때문이다. 기각한 대안은 맥 발송 미지원과 LOCO 직접 접속(약관·제재 위험)이다.
- 맥 이미지 발송은 클립보드 붙여넣기 후 클립보드를 복원한다(사용자 결정).
- 실기 검증 방은 '나와의 채팅'이다(사용자 결정).
- 기존 사용자 데이터와 호환한다. 해시 직렬화는 .NET 골든 픽스처로 검증한다. 근거는 기존 Windows 사용자의 세션·원장·보관함 연속성이다.
- 맥 앱의 프로바이더 데이터(`data/`)는 `.app` 번들 안이 아니라 `~/Library/Application Support/KakaoRelay/data`에 둔다. Windows는 기존처럼 실행 파일 옆에 둔다. 서명된 번들은 쓰기 불가이기 때문이다.
- 맥 DB 키의 사용자 ID는 설정 `selfId`를 먼저 쓰고, 없으면 plist에서 찾고, 마지막으로 SHA512 대조를 한다.
- 툴체인은 `rust-toolchain.toml`로 1.100.0에 고정한다. 전역 stable은 바꾸지 않는다.
- 맥 초안 보호: 입력 직전 Cmd+A·Cmd+C로 초안을 읽어 있으면 중단하고, 클립보드는 복원한다(사용자 결정). 이 경로만 '텍스트는 클립보드 없이' 원칙의 예외다. 기각한 대안은 보호 없이 한계만 표시하는 것이다.
- 맥 화면 기록 권한은 **발송 필수 권한**이다(사용자 결정 변경, 2026-11-23). 실기에서 '창' 메뉴 체크 표시가 전면화 1~3초 뒤 다른 대화창으로 옮겨 가 key window 판정에 쓸 수 없음을 확인했다. 발송 직전과 Return 직전에 CGWindowList 최상위 카카오톡 창의 이름·CGWindowID가 대상과 같을 때만 입력한다. 읽기는 권한 없이 동작한다. 기각한 대안: 다른 대화창이 열려 있으면 중단(여러 방 동시 사용 불가), 맥 발송 보류.
- 메시지 `time`은 원본 `FromUnixTimeSeconds`처럼 UTC(+00:00)로 둔다. KST 변환은 프롬프트에서만 한다. 메시지 지문 해시 호환 때문이다.
- Windows TLS는 `native-tls`를 쓴다. `ring`·`aws-lc` 계열은 맥에서 windows 타깃 C 빌드가 불가능하다. SQLite는 원본처럼 시스템 `winsqlite3.dll`을 raw-dylib로 호출한다.
- 빌드 폴더는 `target/coord` 하나를 공유한다(디스크 여유 부족).

- 2026-11-23 사용자 요청으로 원본에 없는 동작을 추가했다(원본 대조 검사는 그대로 통과).
  - 자동답장: 내 메시지라도 방의 `@봇이름`으로 시작하면 답한다(`candidates_with_self_call`). 봇이 보낸 최근 50개 본문과 같은 내 메시지는 제외해 반복을 막는다.
  - 자동인사(`core::greetings`): 방별로 켜며 설정은 `auto-greetings.json`에 둔다(`ai-settings.json` 원본 형식 유지). 한국 시간 오전(0~12시)·오후(12~24시) 구간마다 처음 말한 사람에게 `🤖 <두 글자> 안녕!`을 보낸다. 82년생 이하이거나 연도가 없으면 `안녕하세요`다. 요청 ID를 방·사람·날짜로 고정해 중복을 막는다.
  - 카카오 ID: 비어 있으면 맥 어댑터가 찾은 ID로 채운다(`Platform::detected_self_id`).
  - 봇 답장(자동답장·이미지 안내·위임 결과)은 앞에 `🤖 `를 붙인다(`auto_reply::bot_text`). 정리감시·제어방·API 발송에는 붙이지 않는다.
- 2026-11-23 맥 방 제목 순서: chatName → `NTChatMeta` type 3 `groupNickname` → 1:1이 아닌 방의 `NTOpenLink.linkName` → 구성원 이름. 이름 없는 단체방과 오픈채팅이 열린 방 목록에서 빠지던 문제를 고쳤다.
- 2026-11-23 맥 번들 서명: 전용 키체인 `kakaorelay-signing`의 로컬 인증서로 서명한다. 다시 빌드해도 TCC 권한이 유지된다. 인증서가 없으면 ad-hoc 서명으로 대신한다.
