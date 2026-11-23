#!/bin/sh
# KakaoRelay.app 번들을 만든다(서명·공증 제외). tauri-cli 없이 배포 빌드 실행 파일·아이콘·Info.plist만 묶는다.
# 결과: $CARGO_TARGET_DIR/release/bundle/KakaoRelay.app
# 프로바이더 데이터는 번들 밖(~/Library/Application Support/KakaoRelay/data)에 둔다(ai `paths` 결정).
set -eu
root=$(cd "$(dirname "$0")/../../.." && pwd)
crate="$root/crates/kakaorelay-app"
target="${CARGO_TARGET_DIR:-$root/target/coord}"
case "$target" in /*) ;; *) target="$root/$target" ;; esac
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)

cd "$root"
CARGO_TARGET_DIR="$target" cargo build --release -p kakaorelay-app

app="$target/release/bundle/KakaoRelay.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target/release/KakaoRelay" "$app/Contents/MacOS/KakaoRelay"

# 아이콘: 512px 원본에서 iconset을 만든다(1024px는 확대본).
iconset=$(mktemp -d)/KakaoRelay.iconset
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z $size $size "$crate/icons/icon.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z $double $double "$crate/icons/icon.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/icon.icns"
rm -rf "$(dirname "$iconset")"

# 권한 안내 문자열: 손쉬운 사용·화면 기록·전체 디스크 접근은 Info.plist 문자열 없이 시스템 설정에서 허용한다.
# Apple Events는 쓰지 않는다. 메뉴바 아이콘은 있지만 창이 보이면 Dock에도 보이므로 LSUIElement는 두지 않는다.
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>ko</string>
  <key>CFBundleDisplayName</key><string>KakaoRelay</string>
  <key>CFBundleExecutable</key><string>KakaoRelay</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundleIdentifier</key><string>com.kakaorelay.app</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>KakaoRelay</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>KakaoRelay</string>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null
# 링커가 실행 파일에만 붙이는 ad-hoc 서명은 식별자가 실행 파일 이름이고 Info.plist와 묶이지 않는다.
# 이 상태로는 TCC가 시스템 설정에서 허용한 번들과 실행 중인 프로세스를 같은 앱으로 보지 않으므로 번들 전체를 서명한다.
# 로컬 서명 인증서(전용 키체인 kakaorelay-signing)가 있으면 그것으로 서명한다. 지정 요구 조건이 인증서에 묶여
# 다시 빌드해도 TCC 권한(손쉬운 사용·화면 기록)이 유지된다. 없으면 ad-hoc 서명이며 빌드마다 권한을 다시 줘야 한다.
sign_dir="$HOME/Library/Application Support/KakaoRelay/signing"
keychain="$HOME/Library/Keychains/kakaorelay-signing.keychain-db"
if [[ -f "$sign_dir/cert.pem" && -f "$keychain" ]]; then
  security unlock-keychain -p "$(cat "$sign_dir/kc-pass")" "$keychain"
  identity=$(openssl x509 -in "$sign_dir/cert.pem" -noout -fingerprint -sha1 | sed 's/.*=//; s/://g')
  codesign --force --sign "$identity" --keychain "$keychain" --identifier com.kakaorelay.app "$app"
else
  codesign --force --sign - --identifier com.kakaorelay.app "$app"
fi
codesign --verify --strict "$app"
echo "$app"
