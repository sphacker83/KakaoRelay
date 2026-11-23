# 맥 카카오톡 연동 조사

Last Updated: 2026-11-23

> 2026-11-23 이 맥(macOS 26.3 arm64, KakaoTalk.app 26.10.0)에서 읽기 전용으로 실증했다. 메시지 발송은 시도하지 않았다. 키·사용자 ID·UUID·대화 내용은 기록하지 않는다.

## 결론

| 영역 | 판정 | 맥 구현 방식 |
|---|---|---|
| 로컬 DB 읽기 | 가능(전체 복호화 실증) | 기기 UUID + 사용자 ID로 키 재계산, SQLCipher compatibility 3 |
| 텍스트 발송 | Windows 방식 불가, 대체 경로 확정 | '창' 메뉴 AXPress로 대화창 전면화 → 합성 키 입력 → DB로 발송 확인 |
| 이미지 발송 | 무인 첨부 불가, 대체 경로 확정 | 클립보드에 파일 → 붙여넣기 → 전송 확인 창 Enter → 클립보드 복원 → DB 확인 |
| 열린 방 식별 | 창 열거 불가, 메뉴로 대체 | '창' 메뉴 항목 이름으로 열린 방·동명 방 판정 |

사용자 결정(2026-11-23): 맥 발송은 "전면 전환 + 키 입력 + DB 확인", 이미지 발송은 "클립보드 사용 허용, 기존 클립보드 복원".

## DB 읽기

- 위치: `~/Library/Containers/com.kakao.KakaoTalkMac/Data/Library/Application Support/com.kakao.KakaoTalkMac/<해시 이름>` 단일 DB와 `-wal`/`-shm`. 다른 앱 컨테이너라 배포 앱은 **전체 디스크 접근(TCC)** 권한이 필요하다.
- 키 재료
  - 기기값: `ioreg -rd1 -c IOPlatformExpertDevice`의 `IOPlatformUUID`(IOKit로 직접 조회 가능).
  - 사용자 ID: 평문 저장 없음. 앱 설정 `selfId`(자동답장의 "내 사용자 ID")가 있으면 그것을 쓴다. 없으면 `~/Library/Containers/com.kakao.KakaoTalkMac/Data/Library/Preferences/com.kakao.KakaoTalkMac.plist`의 키 접미사로 구한다. `DESIGNATEDFRIENDSREVISION:<sha512>` 등은 `SHA512(십진 user_id)`이므로 대조할 수 있고, 일부 키(`AlertKakaoIDsList`, `FSChatWindowFrame_…` 등)에서 직접 얻는 경로도 있다. 마지막 수단은 SHA512 대조 브루트포스로, 9자리 ID 기준 10코어에서 수 분이 걸린다.
  - DB 파일명은 사용자 ID·UUID에서 유도된 해시라서, 다계정이면 활성 계정 DB를 대조해 골라야 한다.
- 키 유도: `passphrase = hex(PBKDF2-HMAC-SHA256(reverse("F".join(["A", base64(SHA1(uuid)+SHA256(uuid)), "|", "F", uuid[:5], "H", user_id, "|", uuid[7:]])), salt=uuid[int(len(uuid)*0.3):], iter=100000, dklen=128))` → 256자 hex 문자열을 SQLCipher passphrase로 사용한다. 출처의 Swift 구현과 Python 재현이 바이트 단위로 일치했다.
- SQLCipher 파라미터: **compatibility 3**. page_size 1024, kdf_iter 64000, PBKDF2·HMAC SHA1, reserve 48, AES-256-CBC.
  - Windows `.edb`는 page 4096·HMAC-SHA512·raw 키이므로 복호화 상수가 다르다.
  - 1페이지 HMAC 검증 후 전체 복호화(약 94.7만 페이지, 5.6초)와 `sqlite_master` 조회(81개 객체)까지 성공했다.
- WAL: magic `0x377f0682`의 표준 SQLite WAL이다. 프레임 페이지는 같은 키로 암호화되어 있다. Windows의 커밋 프레임 반영 로직을 page 1024로 바꿔 적용할 수 있다. `rusqlite` `bundled-sqlcipher`로 원본을 열면 WAL을 SQLite가 처리하지만, 원본 무수정 원칙(읽기 전용·`-shm` 갱신 금지)을 지켜야 한다. 구현 시 스냅샷 복사 후 읽기와 직접 열기 중 하나를 선택하고 근거를 남긴다.
- 스키마(테이블 69개 중 핵심)
  - `NTChatRoom`: `chatId, type, chatName, activeMembersCount, directChatMemberUserId, displayMemberIds(BLOB), lastLogId, lastSeenLogId …`. 1:1 판정은 `directChatMemberUserId > 0`.
  - `NTChatMessage`: `chatId, logId, msgId, authorId, type, status, attachment(TEXT), supplement, extra, message, readAt, sentAt(Unix 초), localFilePath, threadId …`.
  - `NTUser`: `userId, nickName, friendNickName, displayName …`.
  - 그 밖에 `NTChatLogMeta`, `NTChatThread`, `NTChatMeta`, `NTOpenLink`가 있다.
  - 메시지 type 값과 첨부 JSON 형식이 Windows와 같은지는 Phase 3에서 확인한다.

## 발송

- 실측 결과(`AXIsProcessTrusted=true`): 앱 AX 트리에 **창 콘텐츠·입력란·버튼이 없다**. `kAXWindowsAttribute`가 AXApplication 자신을 돌려주고, 하위에는 메뉴바만 있다. `AXManualAccessibility`/`AXEnhancedUserInterface` 설정도 거부된다.
- 메뉴바에는 '카카오톡/채팅/편집/창/도움말'이 노출된다. **'창' 메뉴(항목 25개)에 열린 대화창 이름이 나열되고, 항목에 `AXPress`가 있다.** 열린 방 목록, 동명 방 판정, 대상 창 전면화에 쓴다.
- `CGWindowListCopyWindowInfo`로 카카오톡 창 9개(화면 표시 1개)를 볼 수 있다. 전면화 후 대상 창 이름을 대조하는 보조 검증에 쓸 수 있다. 창 이름 읽기에 화면 기록 권한이 필요한지는 Phase 3에서 확인한다.
- 텍스트 입력은 `CGEventKeyboardSetUnicodeString`으로 한글을 포함한 문자열을 클립보드 없이 합성하고, Return으로 전송한다. 실행 전 대상 창이 key window인지 확인하고, 끝나면 이전 전면 앱으로 포커스를 되돌린다.
- 발송 확인은 Windows의 "입력창 비움"을 대신해 **DB 재조회**로 한다. 해당 chatId에서 authorId=내 ID이고 본문이 일치하는 새 logId가 제한 시간 안에 나타나는지 본다. 이 방법은 Windows보다 강한 확인이지만, 기존 초안 보호(입력창에 쓰던 글이 있으면 중단)는 직접 읽을 수 없다. 대체 방법은 Phase 3에서 설계한다. 우선 DB나 설정에 방별 초안이 저장되는지 확인하고, 확인할 수 없으면 그 한계를 화면·API 응답에 명시한다. 초안을 지우는 입력(전체 선택 후 삭제)은 사용자 글 손실이므로 쓰지 않는다.
- 이미지는 `NSPasteboard`에 파일 URL을 넣고 Cmd+V로 붙여넣은 뒤 전송 확인 창을 Enter로 처리한다. 클립보드는 발송 전 보관하고 발송 후 복원한다. 결과는 DB의 첨부 메시지로 확인한다.
- 필요 권한: 손쉬운 사용(메뉴 AXPress·CGEvent), 전체 디스크 접근(DB). 앱 첫 실행 시 권한 상태를 진단 화면에 표시한다.

## 실기 검증 결과 (2026-11-23)

- 실제 전송: '나와의 채팅'에 텍스트 2건(한글 한 줄, 2줄)과 이미지 1건을 보냈다. 세 건 모두 DB로 확인했다. 다른 방에는 입력하지 않았다.
- 발송 경로
  1. 초안 검사(Cmd+A·Cmd+C, 빈 값)
  2. 유니코드 입력(한글 2벌식 상태에서도 정상)
  3. 되읽기 일치 확인
  4. 카카오톡 창 중 맨 앞(OnScreenOnly z-order, layer 0)의 이름·CGWindowID가 대상인지 확인
  5. Return
  6. DB 확인
  - 텍스트는 약 1.2초, 이미지는 약 1.5초 걸렸다.
- 확인된 사실
  - '창' 메뉴 체크 표시는 key window 판정에 쓸 수 없다.
  - `kCGWindowListOptionAll`은 z-order 순서가 아니다. `OnScreenOnly`를 쓴다.
  - 비활성 카카오톡은 `CGEventPostToPid` 키를 처리하지 않는다. 키를 보내기 전에 활성화가 필요하다. 활성화 순서는 AXFrontmost → NSRunningApplication.activate → `open -b`다.
  - 런루프가 없는 프로세스에서는 NSWorkspace frontmost가 갱신되지 않는다. 창 서버 기준으로 판정한다.
  - 복사 직후 0.25초 미만 동안 이름 없는 알림 창이 맨 위에 뜬다. 판정은 최대 1초 다시 확인한다.
  - 줄바꿈 문자는 전송되지 않고 줄바꿈으로 입력된다.
  - 이미지는 붙여넣은 뒤 새 카카오톡 창(확인 창)이 맨 앞에 뜨고, Return으로 전송된다.
- 남은 한계
  - 발송 순간 카카오톡이 전면 앱이 된다.
  - 도중에 다른 앱이 전면을 가져가면 중단한다. 되읽기 이후에 중단되면 입력창에 글이 남을 수 있다.
  - 최소화된 대화창은 보낼 수 없다(handle 0x0).

## 출처

- https://gist.github.com/blluv/8418e3ef4f4aa86004657ea524f2de14
- https://github.com/silver-flight-group/kakaocli
- https://github.com/cskwork/kakao-userid-recover
- https://github.com/jiru/kakaodecrypt
