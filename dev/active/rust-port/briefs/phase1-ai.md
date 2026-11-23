# Phase 1 — AI 엔진 이식 작업지시서

- 워커명: `ai` (CARGO_TARGET_DIR=target/ai)
- 담당 crate: `crates/kakaorelay-ai/`, `crates/kakaorelay-testkit/`
- 원본: `src/KakaoRelay.AI/*.cs` 전부, 대응 테스트 `tests/KakaoRelay.Tests/`의 AiChecks, AttachmentChecks, AutoKnowledgeChecks, ChatToolChecks, ImageCliChecks, InstructionChecks, KnowledgeChecks, PersonaChecks, ReplyModeChecks, ResidentCliChecks, StructuredKnowledgeChecks와 `tests/KakaoRelay.ModelCatalog.Tests/Program.cs`(WPF 바인딩 검사 제외), `tests/KakaoRelay.Portable.Tests`.
- 참조: inventory §1-2, §4(특히 4-0, 4-1-a, 4-1-c, 4-2, 4-3), §5, §6의 AI 관련 행, §7, §8.

## 목표
KakaoRelay.AI의 모든 책임을 Rust로 옮기고, 대응 테스트의 모든 동작 검사를 `cargo test -p kakaorelay-ai`로 옮긴다.

## 계약과 제약
- 파일 형식·경로·해시는 기존 데이터와 호환한다. 골든 픽스처의 모든 항목을 Rust 출력과 바이트 비교하는 테스트를 만든다(`settingsDefault`, `settingsLegacyResaved` 포함: 이전 단일 `persona` 형식이 첫 슬롯으로 승격되는 순서 의존 동작).
- 프롬프트 문구는 C# 원문과 같게 옮긴다(모델 동작이 바뀌지 않게). CLI 인자·환경변수·JSON-RPC/stream-json 프로토콜은 inventory §5 그대로다.
- 경로: Windows는 `%LOCALAPPDATA%/KakaoRelay`, 실행 파일 옆 `data/`. macOS는 `~/Library/Application Support/KakaoRelay`, 프로바이더 데이터는 `~/Library/Application Support/KakaoRelay/data`(context의 결정). 경로 결정 함수는 한 곳에 둔다.
- 실행 파일 탐색은 OS별 규칙(inventory §5 공통)을 유지한다. Windows 전용 분기(`.exe`, `%LOCALAPPDATA%/OpenAI/Codex/bin`, `windows.sandbox`)는 `cfg`로 유지한다.
- 프로세스 트리 종료: 취소·시간 초과 시 자식까지 종료한다(Unix는 프로세스 그룹, Windows는 Job Object 등). 맥에서 실제로 검증한다.
- PDF: PdfPig 대체 crate를 고른다. 한글 텍스트 PDF 표본으로 추출 품질을 비교하고 근거를 보고한다. 200쪽 제한 등 기존 한도를 유지한다.
- 정규식 lookahead(`StructuredKnowledge.cs:78,263`)는 동작이 같게 옮긴다.
- 이미지 PNG 재인코딩·마젠타 투명화는 `image` crate로 한다(원본 WPF 처리와 결과가 같아야 하는 부분은 테스트로 고정).
- 가짜 CLI: 원본 테스트 실행 파일이 Codex/Claude/Grok 프로토콜을 흉내 내던 방식을 `kakaorelay-testkit`의 바이너리로 옮긴다.
- 공개 API는 Phase 2(Core)와 Phase 5(앱)가 쓴다. C# 공개 멤버에 대응하는 Rust 공개 API를 두고, 모듈 구조를 `lib.rs` 문서 주석에 한 줄씩 남긴다. `ChatReader::read_attachment`의 반환은 `AgentAttachment`가 생기면 확장해도 된다(그 경우 보고).
- 실제 CLI를 사용하는 수동 검사(`--live`, `--ai-smoke`)는 `#[ignore]` 테스트로 옮긴다. 실제 AI 호출을 자동 테스트에서 하지 않는다.

## 완료 조건
- 원본 대응 검사가 모두 Rust 테스트로 존재하고 통과한다(원본 검사 묶음별 수를 보고에 표로 대조).
- 골든 비교 테스트 통과.
- `cargo clippy -p kakaorelay-ai -p kakaorelay-testkit --all-targets -- -D warnings` 통과, windows 타깃 `cargo check -p kakaorelay-ai --target x86_64-pc-windows-msvc` 통과.
