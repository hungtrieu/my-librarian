#!/usr/bin/env bash
# Build app My Librarian cho điện thoại Android (APK debug) – bấm đúp file này trong Finder (máy Mac), hoặc ./build-android.command
# Cùng mã nguồn với bản iPad/iPhone (crates/ipad); trên Android app dùng bố cục iPhone: chỉ tải/đọc sách chữ
# (EPUB, MOBI/AZW3, FB2, TXT), nút nhanh của trình đọc ở góc dưới trái.
#   - Lấy code mới nhất (git pull) nếu trên Mac không có thay đổi dở.
#   - Mã nguồn không đổi kể từ lần build trước: không build, chỉ cài lại APK cũ nếu đang cắm điện thoại.
#   - Có thay đổi: tự tăng phiên bản, chạy test, build APK, lưu vào dist/android/, cài lên điện thoại đang cắm (adb).
#   Build lại dù không đổi gì: ./build-android.command --force
#   LIBRARIAN_ANDROID_DEVICE=<serial>   chọn máy khi có nhiều điện thoại/máy ảo (cột đầu của `adb devices`)
#   LIBRARIAN_ANDROID_TARGETS="aarch64 armv7"   kiến trúc CPU đóng vào APK (mặc định: arm 64-bit + 32-bit)
# Chuẩn bị máy Mac một lần: docs/ANDROID.md (mục "Chuẩn bị một lần").
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
DOC=docs/ANDROID.md

say() { printf '\n\033[1;36m%s\033[0m\n' "$*"; }
ok() { printf '\n\033[1;32m%s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31m%s\033[0m\n' "$*"; exit 1; }

command -v node >/dev/null || fail "Thiếu Node.js – cài từ https://nodejs.org rồi chạy lại."
command -v cargo >/dev/null || fail "Thiếu Rust – xem $DOC mục Chuẩn bị một lần."
command -v cargo-tauri >/dev/null || fail "Thiếu tauri-cli: cargo install tauri-cli --version '^2'"

# Android SDK + NDK + JDK: dùng biến môi trường nếu có, không thì tìm ở chỗ cài mặc định.
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
[ -d "$ANDROID_HOME/platforms" ] || fail "Không thấy Android SDK ở $ANDROID_HOME – xem $DOC mục Chuẩn bị một lần."
if [ -z "${NDK_HOME:-}" ]; then
  NDK_HOME=$(ls -d "$ANDROID_HOME"/ndk/*/ 2>/dev/null | sort -V | tail -1); NDK_HOME="${NDK_HOME%/}"
fi
[ -n "$NDK_HOME" ] && [ -d "$NDK_HOME" ] || fail "Thiếu Android NDK – cài bằng: sdkmanager \"ndk;27.1.12297006\" (xem $DOC)."
export NDK_HOME
if [ -z "${JAVA_HOME:-}" ] || [ ! -x "$JAVA_HOME/bin/java" ]; then
  JAVA_HOME=""
  for j in "$(/usr/libexec/java_home -v 17+ 2>/dev/null || true)" \
           /opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home \
           /opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home \
           /opt/homebrew/opt/openjdk/libexec/openjdk.jdk/Contents/Home \
           "/Applications/Android Studio.app/Contents/jbr/Contents/Home"; do
    if [ -n "$j" ] && [ -x "$j/bin/java" ]; then JAVA_HOME="$j"; break; fi
  done
fi
[ -n "$JAVA_HOME" ] || fail "Thiếu JDK 17 trở lên – cài bằng: brew install openjdk@17 (xem $DOC)."
export JAVA_HOME
ADB="$ANDROID_HOME/platform-tools/adb"

TARGETS="${LIBRARIAN_ANDROID_TARGETS:-aarch64 armv7}"
for t in $TARGETS; do
  case "$t" in
    aarch64) rustup target add aarch64-linux-android >/dev/null ;;
    armv7) rustup target add armv7-linux-androideabi >/dev/null ;;
    x86_64) rustup target add x86_64-linux-android >/dev/null ;;
    i686) rustup target add i686-linux-android >/dev/null ;;
    *) fail "LIBRARIAN_ANDROID_TARGETS: không biết kiến trúc '$t' (aarch64, armv7, x86_64, i686)." ;;
  esac
done

# Điện thoại (hoặc máy ảo) đang nối qua adb – không có cũng build được, chỉ không cài.
pick_device() {
  [ -x "$ADB" ] || return 0
  if [ -n "${LIBRARIAN_ANDROID_DEVICE:-}" ]; then echo "$LIBRARIAN_ANDROID_DEVICE"; return 0; fi
  "$ADB" devices | awk 'NR > 1 && $2 == "device" { print $1; exit }'
}
install_apk() {
  local apk="$1" serial; serial=$(pick_device)
  if [ -z "$serial" ]; then
    echo "Không thấy điện thoại nào qua adb – chưa cài. Chép file APK sang điện thoại rồi mở để cài, hoặc cắm cáp"
    echo "(bật Gỡ lỗi USB, xem $DOC) và chạy lại file này."
    return 0
  fi
  say "Cài lên $serial"
  "$ADB" -s "$serial" install -r "$apk" || fail "CÀI HỎNG – xem bảng 'Nếu gặp lỗi' trong $DOC."
  ok "Đã cài lên $serial."
}

# Lấy code mới (bỏ qua nếu Mac đang có sửa đổi chưa commit, để không ghi đè).
if git diff --quiet && git diff --cached --quiet; then
  say "Lấy code mới nhất (git pull)"
  git pull --ff-only || echo "  (không pull được – build bằng code hiện có trên Mac)"
else
  echo "Trên Mac có thay đổi chưa commit – không git pull, build bằng code hiện có."
fi

eval "$(node scripts/release-state.mjs check android --shell "$@")"
if [ "$action" = none ]; then
  ok "Không có thay đổi mã nguồn kể từ bản $lastVersion (build lúc $lastBuiltAt) – không build gì."
  echo "Bản mới nhất: $lastFolder"
  OLD=$(ls "$lastFolder"/*.apk 2>/dev/null | head -1 || true)
  [ -n "$OLD" ] && install_apk "$OLD"
  exit 0
fi

say "Bản Android $version – lý do: $reason"
say "Kiểm tra (test phần đọc sách dùng chung)"
cargo test -p librarian-core --no-default-features --features reader --lib --quiet || fail "TEST HỎNG – không build, phiên bản giữ nguyên."

say "Ghép giao diện (dùng chung iPad/iPhone/Android)"
node scripts/build-ipad-ui.mjs

TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
pushd crates/ipad >/dev/null
if [ ! -d gen/android ]; then
  say "Khởi tạo dự án Android (lần đầu)"
  cargo tauri android init --ci || fail "Không khởi tạo được dự án Android – xem lỗi phía trên."
fi
# APK "debug": ký bằng khoá chung crates/ipad/android/debug.keystore (không commit – chép tay file này sang máy build khác) để build trên máy nào cũng cài đè được.
node ../../scripts/android-signing.mjs || fail "Không cài được khoá ký chung."
# Phần Rust vẫn biên dịch tối ưu và bỏ ký hiệu gỡ lỗi để app chạy nhanh, APK gọn.
export CARGO_PROFILE_DEV_OPT_LEVEL=2 CARGO_PROFILE_DEV_DEBUG=0
printf '{"version":"%s"}\n' "$version" > "$TMP/version.json"
say "Build APK (kiến trúc: $TARGETS)"
touch "$TMP/start"
# shellcheck disable=SC2086
cargo tauri android build --debug --apk --ci --target $TARGETS --config "$TMP/version.json" \
  || fail "BUILD HỎNG – xem lỗi phía trên và bảng 'Nếu gặp lỗi' trong $DOC. Phiên bản giữ nguyên."
APK=$(find gen/android/app/build/outputs/apk -name '*.apk' -newer "$TMP/start" | head -1)
[ -n "$APK" ] || fail "Build không ra file APK mới – xem lỗi phía trên."
popd >/dev/null

OUT="$PWD/dist/android/$version-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT"
cp "crates/ipad/$APK" "$OUT/My-Librarian-$version.apk"
node scripts/release-state.mjs record android "$version" "$OUT" "$fingerprint"
ok "XONG – bản Android $version: $OUT/My-Librarian-$version.apk"
install_apk "$OUT/My-Librarian-$version.apk"
