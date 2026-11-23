# 공통 작업 규칙 (모든 워커)

- 모든 보고·주석·커밋 메시지 본문은 한국어로 쓴다(코드 식별자·기술 용어만 원문). 코드 주석은 코드로 드러나지 않는 이유·제약만 쓴다.
- 저장소: `/Users/ethan/Workspace/dev/KakaoRelay`. 먼저 `dev/active/rust-port/rust-port-plan.md`, `rust-port-context.md`의 "중요한 의사결정", `rust-port-inventory.md`의 담당 절을 읽는다.
- 이미 만든 계약: `crates/kakaorelay-ai/src/chat_models.rs`(공통 모델, `ChatReader`), `crates/kakaorelay-core/src/platform.rs`(OS 어댑터 trait). 계약 변경이 필요하면 직접 바꾸지 말고 이유와 제안을 보고한다. 단, 담당 crate 안에서만 쓰는 타입은 자유롭게 만든다.
- 편집 범위: 지시서에 적은 crate 디렉터리만 수정한다. 루트 `Cargo.toml`, 다른 crate, C# 원본(`src/`, `tests/`)은 수정하지 않는다. 의존성은 담당 crate의 `Cargo.toml`에 버전을 직접 적는다(루트 `[workspace.dependencies]`에 있는 것은 `.workspace = true` 사용 가능).
- 다른 워커가 같은 저장소에서 동시에 작업한다. 빌드 잠금 충돌을 피하려고 `CARGO_TARGET_DIR=target/<워커명>`을 지정하고, 빌드·테스트는 `-p <담당 crate>`로 한정한다. `cargo fmt`도 `-p`로 한정한다.
- 툴체인은 `rust-toolchain.toml`(1.100.0, edition 2024)이 자동 적용된다. 전역 설치·시스템 설정 변경은 하지 않는다.
- 원칙: YAGNI·KISS, 기존 C# 동작의 1:1 이식이 목표다. 실패를 더미 결과나 오류 무시로 숨기지 않는다. 비밀(키·토큰·사용자 ID)과 대화 본문·방 이름을 로그·테스트 픽스처·보고에 남기지 않는다.
- 해시·파일 형식 호환: `crates/kakaorelay-ai/tests/fixtures/dotnet-golden.json`은 .NET이 만든 기준 출력이다(생성기 `tools/golden/Program.cs`). .NET 기본 직렬화는 비 ASCII와 `"<>&'+`를 `\uXXXX`(대문자 hex)로 이스케이프하고, `PromptJson`은 `"<>&'+`·제어문자·서로게이트만 이스케이프한다. `DateTimeOffset`은 소수 초 끝의 0을 생략한다(예: `.12`, `.1234568`).
- 사용자에게 보이는 동작(기능·규칙·수치·화면)을 새로 정해야 하는 선택이 생기면 임의로 정하지 말고, 그 부분을 남겨 두고 선택지와 권장안을 보고한다. 구현 세부는 근거를 갖고 직접 정한다.
- 커밋·push는 하지 않는다.
- 완료 보고: 변경 파일 목록, 실행한 검증 명령과 결과(통과/실패 수), C# 대비 의도적으로 달라진 점, 남은 한계·미해결, 계약 변경 제안.
