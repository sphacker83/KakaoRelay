#!/bin/sh
# 맥에서 Windows 타깃 타입 검사만 한다(`cargo check`는 링크하지 않는다).
# tauri-build는 Windows 타깃이면 아이콘·버전 리소스를 llvm-rc로 컴파일하는데, 맥에는 llvm-rc가 없다.
# 검사에서는 리소스 결과를 쓰지 않으므로 빈 출력을 만드는 대체 컴파일러를 RC로 지정한다. 실제 Windows 빌드는 MSVC rc.exe를 쓴다.
set -eu
root=$(cd "$(dirname "$0")/../../.." && pwd)
stub="$root/tmp/rc-check-stub.sh"
mkdir -p "$root/tmp"
cat > "$stub" <<'STUB'
#!/bin/sh
case "$*" in *"/?"*) echo "LLVM Resource Converter (검사용 대체) /no-preprocess"; exit 0;; esac
while [ $# -gt 0 ]; do
  if [ "$1" = "/fo" ]; then : > "$2"; fi
  shift
done
STUB
chmod +x "$stub"
cd "$root"
RC="$stub" CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/coord}" cargo check -p kakaorelay-app --target x86_64-pc-windows-msvc "$@"
