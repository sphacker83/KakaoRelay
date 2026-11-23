# Phase 3 — macOS 어댑터 작업지시서

- 워커명: `macos` (CARGO_TARGET_DIR=target/macos)
- 담당 crate: `crates/kakaorelay-macos/`만. 실험용 스크립트는 scratchpad(`/private/tmp/claude-501/-Users-ethan-Workspace-dev-KakaoRelay/e1a52315-10cd-4bfb-b870-0148e6e354e6/scratchpad/macos`)에 둔다.
- 필독: `dev/active/rust-port/rust-port-macos-research.md`(실증 결과와 사용자 결정), `crates/kakaorelay-core/src/platform.rs`, `crates/kakaorelay-ai/src/chat_models.rs`, 원본 `src/KakaoRelay.Core/LocalChatReader.cs`(Windows 판 읽기·메시지 매핑·첨부 메타데이터·워터마크 규칙), `KakaoTestTransport.cs`, `KakaoImageTransport.cs`, `ConversationCatalog.cs`, `TestSend.cs`.
- 이전 조사 스크립트(Python/Swift)가 scratchpad 루트에 있을 수 있다(`verify3.py`, `decrypt.py`, `recover.py`, `axwin.swift`, `ax2/`). 참고만 하고, 결과물은 Rust로 만든다.

## 목표
`kakaorelay_core::platform`의 `ChatReader`, `RoomCatalog`, `TextTransport`, `ImageTransport`를 macOS용으로 구현하고, 실제 카카오톡(26.10.0)에서 검증한다. `Platform` 구현체(`MacPlatform`)를 공개한다.

## 계약과 제약
- DB 읽기
  - 키 유도·SQLCipher compatibility 3·WAL은 research 문서를 따른다.
  - 사용자 ID는 생성자 인자(설정 `selfId`)를 먼저 쓰고, 없으면 plist에서 찾고, 마지막으로 SHA512 대조를 한다. 대조 비용이 크므로 결과를 메모리에 캐시하고, 파일에 평문으로 저장하지 않는다.
  - **카카오톡 파일(DB, `-wal`, `-shm`, plist)을 절대 수정하지 않는다.** 실행 전후 크기·수정 시각·해시를 비교해 보고한다.
  - 새 메시지는 폴링(기본 5초)마다 보여야 한다. DB가 약 1GB이므로 매 폴링 전체 복호화는 허용하지 않는다.
  - 방법(`rusqlite` + SQLCipher 읽기 전용 연결, 스냅샷 복사, 수동 페이지 복호화 등)은 성능·무수정·최신성 근거로 고르고 보고한다.
  - 공통 모델 매핑: `LocalRoom.profile`은 활성 DB 식별자(파일명), `id`는 chatId, `direct_member_id`는 `directChatMemberUserId`(>0일 때)다.
  - 메시지 `kind`·삭제 표시·첨부 메타데이터(`attachment_metadata`)는 Windows 판 `LocalChatReader`가 만드는 값과 의미가 같도록 맞춘다. 이 값들은 자동답장 후보 판정(type 1/18/2)과 첨부 다운로드에 쓰인다. 다른 점은 보고한다.
  - 조회 한도는 1~500개다.
- 열린 방 목록(`RoomCatalog`)
  - 카카오톡 메뉴바 '창' 메뉴 항목 중 대화창 항목을 방 제목으로 쓴다. 같은 제목이 둘 이상이면 `ambiguous`로 표시한다.
  - `handle`은 대화창의 CGWindowID(`0x` + 16진수)다. 메뉴 항목과 CGWindowID를 연결할 수 없으면 그 한계와 대안을 보고한다.
  - 메뉴 고정 항목(최소화, 확대/축소 등)과 대화창 항목을 구분하는 규칙을 근거와 함께 둔다.
- 텍스트 발송(`TextTransport`)
  - `validate_target`: 프로세스 생존, 화면 잠금 여부, '창' 메뉴에서 해당 제목이 정확히 1개인지 확인한다.
  - `write_draft`: '창' 메뉴 항목 `AXPress`로 대상을 전면화한다. 전면 앱이 카카오톡이고 key window가 대상 창인지 확인한 뒤, `CGEventKeyboardSetUnicodeString`으로 클립보드 없이 입력한다.
  - `post_enter`: Return을 보낸다.
  - `read_draft`: `Ok(None)`을 반환한다.
  - `confirm_after_enter`: DB에서 해당 chatId에 authorId=내 ID이고 본문이 일치하는 새 메시지(발송 전 마지막 logId 이후)를 제한 시간 안에 찾는다.
  - 끝나면 이전 전면 앱으로 포커스를 되돌린다.
  - 사용자 키 입력 혼입을 줄이려고 발송 구간을 짧게 유지한다. 대상 확인이 하나라도 실패하면 입력 전에 중단한다.
- 이미지 발송(`ImageTransport`)
  - 순서: 현재 클립보드 전체(모든 타입)를 보관 → 파일 URL을 클립보드에 넣음 → 대상 전면화·확인 → Cmd+V → 전송 확인 창 확인 후 Return → 클립보드 복원 → DB에서 첨부 메시지 확인.
  - 확인 창을 식별하는 방법(CGWindowList 등)을 근거와 함께 둔다.
- 권한: 전체 디스크 접근(DB 읽기 실패 형태로 판별)과 손쉬운 사용(`AXIsProcessTrusted`)을 확인한다. 권한이 없으면 원인과 설정 위치를 담은 명확한 오류(`ApiFailure` 409 등)를 반환한다. 권한 상태를 진단용 공개 함수로 제공한다.
- 단일 인스턴스와 발송 잠금은 Phase 5에서 정한다. 이 작업 범위가 아니다.

## 실기 검증 (사용자 승인 범위)
- 실제 발송은 **'나와의 채팅' 방에서만** 한다. 대상은 DB로 확인한다(구성원이 나뿐인 방). 제목만으로 판단하지 않는다.
- 본문은 `[KakaoRelay 테스트] <순번>`처럼 식별 가능한 짧은 문구로 한다. 텍스트는 최대 5회, 이미지(작은 PNG)는 최대 2회 보낸다.
- '나와의 채팅'이 별도 창으로 열려 있지 않으면 UI 클릭으로 열지 않는다. 구현과 자동 테스트를 마치고 "사용자가 창을 열어야 함"을 blocker로 보고한다.
- 다른 방에는 어떤 입력도 보내지 않는다. 대화 본문·방 이름·사람 이름·키·사용자 ID를 출력·보고·파일에 남기지 않는다. 개수·형식·성공 여부만 보고한다.

## 완료 조건
- 오프라인 단위 테스트가 통과한다: 키 유도의 알려진 벡터(가짜 UUID·ID), WAL 반영, 스키마 매핑, 메뉴 항목 분류, 동명 판정.
- 실제 환경 검증 결과를 보고한다: 방 수, 최근 메시지 조회 성공·소요 시간, 텍스트·이미지 발송 성공(DB 확인), 클립보드 복원, 포커스 복원, 카카오톡 파일 무수정.
- `cargo clippy -p kakaorelay-macos --all-targets -- -D warnings` 통과.
