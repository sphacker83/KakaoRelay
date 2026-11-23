# KakaoRelay Rust 이식 Tasks

Last Updated: 2026-11-23

## Phase 0 — 기반과 계약 고정 [상태: 완료]

- [x] T-0.1 workspace·crate 골격, 툴체인, 공통 의존성, `.gitignore`
- [x] T-0.2 공통 모델과 플랫폼 trait 계약
- [x] T-0.3 .NET 골든 픽스처 생성

## Phase 1 — AI 엔진 이식 [상태: 완료]

- [x] T-1.1 설정·페르소나·세션 상태·직렬화 호환
- [x] T-1.2 CLI 실행·프로바이더 폴백·상주 세션·모델 목록·프로바이더 저장소
- [x] T-1.3 지식베이스(검색·자료 가져오기·구조화 지식·자동 정리)
- [x] T-1.4 페르소나 이미지·이미지 생성·마젠타 투명화·첨부·위임·명령 해석·요약·맥락 판단·가져온 대화
- [x] T-1.5 AI 테스트 이식 완료와 골든 비교 (원본 269개 중 251개 이식·통과, Core 의존 18개는 Phase 2로 이관, Rust 추가 10개, 골든 4개)

## Phase 2 — Core 공통 이식 [상태: 완료]

- [x] T-2.1 발송 원장·이미지 발송 상태 기계·작업 큐
- [x] T-2.2 HTTP API(axum)와 ApiSendService
- [x] T-2.3 자동답장·런타임·일일 기록·위임 작업
- [x] T-2.4 정리감시·제어방·진단 보고 (core 검사 202개+골든 6개, 이관 18개 포함. 통합 검증 `tmp/integration-3.log`)

## Phase 3 — macOS 어댑터 [상태: 완료]

작업지시서: [briefs/phase3-macos.md](briefs/phase3-macos.md)

- [x] T-3.1 맥 DB 읽기(키 유도·compat 3·WAL·NT* 스키마 매핑) — 실제 DB 방 494개, 최근 50개 조회 약 9ms, 카카오톡 파일 무수정
- [x] T-3.2 열린 방 목록과 텍스트 발송 — '나와의 채팅'에 한글 한 줄·2줄 전송, DB 확인 true
- [x] T-3.3 이미지 발송 — PNG 1건 전송(type 2), DB 확인 true, 클립보드 항목·타입·데이터 동일 복원
- [x] T-3.4 권한 진단(전체 디스크 접근·손쉬운 사용·화면 기록) — 단일 인스턴스는 T-5.1로 이관
  - 남은 연결: `ConversationTarget.room_id`를 맥 catalog가 채우는 일(T-5.1에서 필드 추가 후 코디네이터가 처리)

## Phase 4 — Windows 어댑터 [상태: 진행 중]

작업지시서: [briefs/phase4-windows.md](briefs/phase4-windows.md)

- [x] T-4.1 SQLCipher 4 복호화·WAL·메모리 키 스캔·로컬 방/메시지 (맥 테스트 15개, 두 타깃 clippy 통과. 첨부 원본 읽기는 core 공통 래퍼로 이관)
  - 목표: LocalChatReader 이식, CipherChecks 이식
  - 의존성: T-0.2
  - 완료 조건: 맥에서 cipher 테스트 통과, windows 타깃 빌드 통과
- [x] T-4.2 Win32 창 열거·텍스트 발송·이미지 발송·UIA 진단·named mutex (실제 저장소 windows 타깃 clippy 통과, 실기는 T-4.3)
  - 목표: NativeWindows·KakaoTestTransport·KakaoImageTransport·ConversationCatalog·DiagnosticCollector 이식
  - 의존성: T-0.2
  - 완료 조건: windows 타깃 clippy 통과
- [ ] T-4.3 Windows 실기 검증 절차서
  - 목표: Windows PC에서 실행할 검증 시나리오와 명령
  - 의존성: T-4.1, T-4.2, T-5.x
  - 완료 조건: 절차서가 context에 연결됨

## Phase 5 — Tauri 앱 [상태: 완료]

작업지시서: [briefs/phase5-app.md](briefs/phase5-app.md) (워커 `app`)

- [x] T-5.1 앱 셸: 창·트레이·단일 인스턴스·종료 순서·명령줄 모드·API 기동
  - core 인계(2026-11-23): `RelayHost`(core `host.rs`)로 조립한다. 앱이 넣어 줄 값은 다음과 같다.
    - `set_settings_busy`, `set_known_rooms`
    - OS별 `RelayApiConfig.recent_messages_reason`
    - 진단 보고서의 `osVersion`·`sessionId`
  - 연결 결과(2026-11-23): `ConversationTarget.room_id`(직렬화 제외)와 `new`를 추가하고, compose·api_send·앱 preflight가 요청으로 넘긴다. 맥 catalog는 `None`을 유지한다. scan마다 DB를 대조하면 14초 이상 걸리고, 발송 시점의 제목 대조와 결과가 같기 때문이다(코디네이터 결정).
  - 정리 거리: 이미지 transfer root를 ai paths 모듈로 옮긴다.
  - 목표: App.xaml.cs·TrayHost 대응
  - 의존성: T-2.2
  - 완료 조건: 앱 실행, 트레이 열기/완전 종료, health 응답
- [x] T-5.2 메시지·API 탭
  - 목표: MainWindow 기능 대응
  - 의존성: T-5.1
  - 완료 조건: 방 새로고침·발송·이력·진단·OpenAPI 복사
- [x] T-5.3 AI 챗봇 탭(자동답장·페르소나·이미지·프로바이더·대화분석·정리감시·제어방)
  - 목표: AiPanel·PersonaImagesPanel·SummaryForwardPanel·CommandRoomPanel 대응
  - 의존성: T-5.1, Phase 1·2
  - 완료 조건: 각 하위 탭 기능 동작
- [x] T-5.4 지식베이스 탭(자료·연결 설정·대화 자동 정리)과 대화 가져오기
  - 목표: KnowledgePanel·AutoKnowledgePanel·Portable 가져오기 대응
  - 의존성: T-5.1, Phase 1
  - 완료 조건: 각 기능 동작
- [x] T-5.5 맥 `.app` 번들·아이콘·권한 설명 문자열
  - 목표: 배포 가능한 맥 앱
  - 의존성: T-5.1
  - 완료 조건: `.app` 실행
  - 결과: `scripts/bundle-macos.sh`로 ad-hoc 서명 `.app`(31MB)을 만든다. 다시 빌드하면 cdhash가 바뀌어 TCC 권한을 다시 줘야 한다. 첫 실행 때 '다른 앱 데이터' 허용 창이 뜬다. 권한 안내에 네 번째 항목으로 추가했다(사용자 결정).
  - 검증: workspace test(ai 261, core 202, 그 밖 45), mac·Windows clippy, fmt 통과(`tmp/integration-4.log`). 스크린샷 `tmp/screens/` 13장.
  - 후속: 맥 DB 방 목록 첫 조회 27.6초(494개). 조사 문서의 단건 9ms와 차이가 커서 Phase 6에서 원인을 확인한다.

## Phase 6 — 통합 검증과 정리 [상태: 대기]

- [ ] T-6.1 맥 실기 시나리오('나와의 채팅')
  - 목표: 자동답장·API 발송·이미지·정리감시 발송
  - 사용자 지정(2026-11-23): 정리감시 원본은 사용자가 지정한 그룹방 1개다. 방 이름은 개인정보라 문서에 남기지 않으며, 실기 직전 사용자에게 확인한다. 수신방은 '나와의 채팅'이다. 원본 방에는 아무것도 보내지 않고 읽기만 한다.
  - 의존성: Phase 3, 5
  - 완료 조건: 시나리오 기록
- [ ] T-6.2 독립 리뷰(Codex Sol)와 반영
  - 목표: 구현과 분리된 리뷰
  - 의존성: T-6.1
  - 완료 조건: 필수 지적 해소
- [ ] T-6.3 문서 갱신(README, docs/*, openapi)
  - 목표: 문서와 실제 동작 일치
  - 의존성: T-6.2
  - 완료 조건: 문서 갱신 완료
- [ ] T-6.4 C# 코드 삭제
  - 목표: Rust 단일 코드베이스
  - 의존성: T-4.3의 Windows 실기 통과(사용자 결정: Rust 검증 후 삭제)
  - 완료 조건: C# 파일·솔루션·스크립트 정리, 빌드 문서 갱신
