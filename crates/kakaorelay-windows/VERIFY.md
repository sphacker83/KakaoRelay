# kakaorelay-windows 실기 검증 절차

맥에서는 OS 비의존 로직 테스트와 `x86_64-pc-windows-msvc` 대상 clippy만 확인했다. 이 문서는 Windows PC에서 Win32 동작을 확인하는 절차다. 모든 단계는 저장소 루트에서 실행한다.

## 안전 조건

- 실제 발송 대상은 **'나와의 채팅'** 하나뿐이다. 다른 대화방 창은 모두 닫는다.
- 발송 단계 중에는 키보드·마우스를 만지지 않는다. Shift·Ctrl·Alt가 눌려 있으면 Enter를 보내지 않고 중단해야 정상이다.
- 출력에 방 제목, 프로필 폴더(40hex), 사용자 ID가 나온다. 결과를 공유할 때는 이 값을 지우고 통과 여부와 개수만 남긴다. 대화 본문은 도구가 출력하지 않는다.
- 같은 발송 명령을 자동으로 반복하지 않는다. 결과가 불확실하면 카카오톡 화면을 직접 확인한다.
- 기존 C# 앱(KakaoRelay.exe)은 종료한 상태로 진행한다. 단, 8단계의 상호 배제 확인에서는 C# 앱을 함께 쓴다.

## 준비

1. Windows 10/11 x64, 카카오톡 PC 로그인 상태, Visual Studio Build Tools(MSVC, Windows SDK), rustup이 필요하다. 툴체인은 `rust-toolchain.toml`이 자동으로 맞춘다.
2. 카카오톡에서 '나와의 채팅' 창을 따로 연다(더블클릭으로 별도 창). 최소화하지 않는다.
3. 창 제목을 확인한다. 아래 명령의 `<제목>`에는 이 창 제목을 그대로 쓴다.

## 1. 빌드와 자동 검사

```powershell
cargo test -p kakaorelay-windows
cargo clippy -p kakaorelay-windows --all-targets -- -D warnings
cargo build -p kakaorelay-windows --example verify
```

기대 결과: 테스트 16개 통과(Windows 전용 1개 포함), clippy 경고 없음, `target\debug\examples\verify.exe` 생성. `winsqlite3.dll`은 Windows 기본 구성 요소이며 가져오기 라이브러리 없이 링크되어야 한다.

## 2. 열린 방 목록 (`ConversationCatalog.Scan`)

```powershell
cargo run -p kakaorelay-windows --example verify -- rooms
```

기대 결과:
- '나와의 채팅' 한 줄이 `열린 대화방`으로 나온다. pid·handle(`0x…`)이 표시된다.
- 메인 창(친구·채팅 목록), 숨긴 창, 최소화한 창은 나오지 않는다.
- 같은 제목의 창을 하나 더 열면 `같은 이름의 방이 여러 개 있어 선택할 수 없습니다`로 바뀐다. 확인 후 하나를 닫는다.
- 이 명령은 포커스·전면 창을 바꾸지 않아야 한다.

## 3. 로컬 DB 읽기 (SQLCipher 4 + 메모리 키)

```powershell
cargo run -p kakaorelay-windows --example verify -- local-rooms
cargo run -p kakaorelay-windows --example verify -- read <profile> <roomId> 20
```

기대 결과:
- `local-rooms`가 방 목록을 낸다. 카카오톡에서 연 적 있는 방은 `readable=true`다. 1:1 방은 `direct=true`일 수 있다.
- `read`가 `messages=N`과 메시지별 id·시각·type·길이를 낸다. 본문은 출력되지 않는다.
- 카카오톡 종료 상태: `409 kakao_not_running`.
- 키가 없는 방: `409 key_unavailable`. 해당 방을 카카오톡에서 연 뒤 `local-rooms`를 다시 실행하면 읽혀야 한다.
- 메시지를 새로 보낸 직후에도 읽혀야 한다(WAL 반영). 쓰는 중이면 `409 database_busy`가 날 수 있다. 잠시 뒤 다시 실행한다.
- 원본 C# 앱의 같은 방 읽기 결과와 메시지 수·마지막 id가 같아야 한다.

## 4. 텍스트 발송 (`EM_REPLACESEL` + Enter 게시)

먼저 검증만 한다.

```powershell
cargo run -p kakaorelay-windows --example verify -- send-text "<제목>" "Rust 검증 1"
```

기대 결과: `검증 통과. 초안 길이=0`, `--confirm이 없어 입력하지 않았습니다.` 입력창은 바뀌지 않는다.

다음으로 실제로 발송한다.

```powershell
cargo run -p kakaorelay-windows --example verify -- send-text "<제목>" "Rust 검증 1" --confirm
```

기대 결과:
- `되읽기 일치=true`, `Enter 게시됨. 입력창 비움 확인=true`.
- '나와의 채팅'에 말풍선 하나가 생긴다. 전면 창·클립보드는 바뀌지 않는다.
- 여러 줄 본문(`"첫 줄`n둘째 줄"`)도 줄바꿈이 유지된 한 말풍선이어야 한다.

다음 차단 동작도 확인한다. 이 경우 발송이 일어나면 안 된다.

| 조건 | 기대 결과 |
|---|---|
| 입력창에 다른 글을 써 둔 상태 | `입력창이 비어 있지 않아 중단했습니다.` |
| 대화창 최소화 | `Recipient window is missing or ambiguous` 또는 `Observed conversation identity or state changed` |
| 화면 잠금(Win+L) 중 실행(작업 스케줄러 등으로 30초 지연 실행) | `Interactive desktop is unavailable or locked` 또는 `Default interactive desktop is unavailable` |
| 실행 직후 Shift를 누르고 있음 | `A keyboard modifier is held; Enter was not sent`(입력된 초안은 직접 지운다) |

참고: 검증 도구는 안내 문구 정책(`verified_empty_placeholder`)을 끈 상태로 보낸다. 빈 입력창의 안내 문구 `메시지 입력`이 실제 텍스트로 읽히면 비어 있지 않다고 보고 중단한다. 이 현상이 보이면 기록한다. 앱에서는 Phase 2 상태 기계가 사용자 설정에 따라 이 값을 넘긴다.

## 5. 이미지 발송 (`WM_DROPFILES` + 확인 창 Enter)

작은 PNG 파일 하나를 준비한다(예: `C:\Temp\verify.png`).

```powershell
cargo run -p kakaorelay-windows --example verify -- send-image "<제목>" C:\Temp\verify.png
cargo run -p kakaorelay-windows --example verify -- send-image "<제목>" C:\Temp\verify.png --confirm
```

기대 결과:
- 첫 명령은 `검증 통과(빈 초안 확인).`만 출력한다.
- 두 번째 명령은 확인 창(`사진 전송` 등 또는 제목 없음)을 3초 안에 찾아 Enter를 보낸다. `첨부 요청됨. 미리보기 전송·닫힘 확인=true`가 나오고 이미지가 '나와의 채팅'에 올라간다.
- 확인 창이 나타나지 않으면 `첨부 요청 후 이 대화방의 이미지 전송 창을 확인하지 못했습니다…`가 나온다. 이때 다시 보내지 않고 화면을 확인한다.
- 카카오톡 버전에 따라 확인 창 제목이 목록(`사진 전송`, `사진 보내기`, `이미지 전송`, `이미지 보내기`, `파일 전송`, `파일 보내기`)에 없을 수 있다. 실패하면 6단계 진단으로 창 클래스와 자식 ID(1001, 1002)를 기록한다.

## 6. UIA 진단

```powershell
cargo run -p kakaorelay-windows --example verify -- diagnose
```

기대 결과:
- `status=complete` 또는 `partial`, `processes>=1`. 창마다 `complete/limited/skipped-hidden/skipped-limit/stale/error` 중 하나가 표시된다.
- 창당 컨트롤은 250개 이하이며, UIA 탐색은 창당 4초 안에 끝난다. 탐색하는 창은 12개 이하다.
- 대화방 창에서 `inputCandidates>=1`이 보여야 한다.
- `foreground before`와 `after`가 같아야 한다. 진단이 전면 창을 바꾸면 안 된다.
- 카카오톡이 응답하지 않아 명령이 멈추면 Ctrl+C로 종료한다. 앱에서는 Phase 2의 `ProbeRunner`가 20초 뒤 worker를 종료한다.

## 7. 단일 인스턴스 (`Local\KakaoRelay.MainWindow`, `Local\KakaoRelay.ShowWindow`)

터미널 두 개를 연다.

```powershell
# 터미널 A
cargo run -p kakaorelay-windows --example verify -- instance
# 터미널 B (A가 기다리는 60초 안에)
cargo run -p kakaorelay-windows --example verify -- instance
```

기대 결과: A는 `첫 실행입니다…` 후 B 실행 시 `표시 요청 수신=true`. B는 `이미 실행 중입니다. 표시 요청을 보냈습니다.`

## 8. C# 앱과의 상호 배제

기존 C# 앱(KakaoRelay.exe)을 실행한 상태에서 7단계의 `instance`를 실행한다. 기대 결과는 `이미 실행 중입니다`이고 C# 앱 창이 다시 표시되는 것이다. 이름이 같아야 두 판이 동시에 뜨지 않는다.

## 결과 기록

단계별 통과/실패, 카카오톡 버전(`diagnose`의 version은 출력하지 않으므로 카카오톡 설정의 버전), Windows 빌드 번호를 기록한다. 방 제목·ID·본문은 남기지 않는다.

## 남은 한계

- 첨부 원본 내려받기(`read_attachment`)는 AI crate의 `ReceivedAttachments` 이식 후 연결한다.
