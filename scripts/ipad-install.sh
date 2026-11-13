#!/usr/bin/env bash
# Build app iPad và cài lên iPad qua cáp hoặc Wi-Fi từ Mac. Dùng cho cả lần đầu lẫn ký lại mỗi 7 ngày (Apple ID miễn phí).
# Cần: Xcode, Rust (rustup), target aarch64-apple-ios, `cargo install tauri-cli --version "^2"`, Node.
#   LIBRARIAN_IPAD_TEAM=<Team ID>     bắt buộc (trường OU của chứng chỉ "Apple Development", xem docs/IPAD.md)
#   LIBRARIAN_IPAD_DEVICE=<tên|UDID>  tuỳ chọn; mặc định lấy iPad (hoặc iPhone) thật đầu tiên đã ghép với Mac
#   LIBRARIAN_DEVICE_KIND=iPad|iPhone tuỳ chọn (mặc định iPad). Cùng một app cho cả hai; trên iPhone app tự chỉ
#                                     nhận sách chữ và dồn nút nhanh sang trái (build-iphone.command đặt sẵn).
#   LIBRARIAN_IPAD_VERSION=<x.y.z>    tuỳ chọn; số phiên bản ghi vào app (build-ipad.command tự đặt)
#   LIBRARIAN_IPAD_OUT=<thư mục>      tuỳ chọn; chép bản .app đã ký + file .ipa vào đây sau khi build
#   LIBRARIAN_IPA_ONLY=1              tuỳ chọn; chỉ build + xuất .ipa, không cần thiết bị cắm/ghép với Mac (máy đã
#                                     đăng ký vào team từ lần cài trước). Cài .ipa ở máy khác: install-ios.cmd /
#                                     install-ios.command (docs/IPAD.md mục "Build ở Mac khác, cài ở nhà").
#   LIBRARIAN_IPA_SHARE=<thư mục>     tuỳ chọn; chép thêm .ipa vào đây (vd. thư mục Google Drive/OneDrive/iCloud Drive)
# Thường không gọi trực tiếp – dùng ./build-ipad.command ở thư mục gốc.
# Đã chạy được trên iPad Air 5 / iPadOS 27 / Xcode 27.2 (2026-11-09).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

: "${LIBRARIAN_IPAD_TEAM:?Đặt LIBRARIAN_IPAD_TEAM=<Team ID> (xem docs/IPAD.md)}"
export APPLE_DEVELOPMENT_TEAM="$LIBRARIAN_IPAD_TEAM"
BUNDLE_ID=vn.hungtrieu.mylibrarian.ipad
KIND="${LIBRARIAN_DEVICE_KIND:-iPad}"
case "$KIND" in iPad|iPhone) ;; *) echo "LIBRARIAN_DEVICE_KIND phải là iPad hoặc iPhone"; exit 1 ;; esac

command -v cargo-tauri >/dev/null || { echo "Thiếu tauri-cli: cargo install tauri-cli --version '^2'"; exit 1; }
rustup target add aarch64-apple-ios >/dev/null

IPA_ONLY="${LIBRARIAN_IPA_ONLY:-}"
UDID=""
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
if [ -n "$IPA_ONLY" ]; then
  echo "▶ Chỉ build .ipa (không cài) – cài sau bằng install-ios.cmd / install-ios.command trên máy có cắm $KIND"
else
# Tìm UDID của thiết bị qua JSON (cột chữ của `devicectl list devices` có dấu cách trong tên/model, không tách được).
xcrun devicectl list devices --json-output "$TMP/devices.json" >/dev/null 2>&1 || true
UDID=$(node -e '
  const want = process.argv[2] || "", kind = process.argv[3];
  let devs = [];
  try { devs = require(process.argv[1]).result.devices; } catch {}
  const d = devs.find(x => x.hardwareProperties?.reality === "physical" && (want || x.hardwareProperties?.deviceType === kind)
    && (!want || want === x.hardwareProperties?.udid || want === x.deviceProperties?.name));
  if (d) console.log(d.hardwareProperties.udid);
' "$TMP/devices.json" "${LIBRARIAN_IPAD_DEVICE:-}" "$KIND")
[ -n "$UDID" ] || { echo "Không thấy $KIND. Cắm cáp, mở khoá $KIND, bấm Tin cậy; hoặc kiểm LIBRARIAN_IPAD_DEVICE."; echo "(Không có $KIND ở đây: chạy với --ipa để chỉ xuất .ipa rồi cài ở máy khác.)"; xcrun devicectl list devices; exit 1; }
echo "▶ $KIND: $UDID"
fi

echo "▶ Ghép giao diện (dùng chung iPad/iPhone)"
node scripts/build-ipad-ui.mjs

cd crates/ipad
if [ ! -d gen/apple ]; then
  echo "▶ Khởi tạo dự án Xcode (lần đầu)"
  cargo tauri ios init
fi

# Info.plist: thư mục Books hiện trong app Tệp, mở file tại chỗ, quyền camera + mạng nội bộ. Chạy lại không hại.
PLIST=$(ls gen/apple/*/Info.plist | head -1)
pb() { /usr/libexec/PlistBuddy -c "$1" "$PLIST" 2>/dev/null || true; }
set_bool() { pb "Delete :$1"; pb "Add :$1 bool true"; }
set_str() { pb "Delete :$1"; pb "Add :$1 string $2"; }
set_bool UIFileSharingEnabled
set_bool LSSupportsOpeningDocumentsInPlace
set_str NSCameraUsageDescription "Quét mã QR để ghép nối với My Librarian trên máy tính"
set_str NSLocalNetworkUsageDescription "Kết nối với My Librarian trên máy tính trong mạng Wi-Fi để tải sách"

# Personal Team chỉ cấp provisioning profile cho thiết bị đã đăng ký vào team. `cargo tauri ios build` build cho
# "Any iOS Device" nên không tự đăng ký → lỗi "Your team has no devices". Lần đầu: gọi xcodebuild với đúng iPad để
# Xcode đăng ký thiết bị và tạo profile. Bước Rust trong lần gọi này sẽ lỗi (nó cần tauri CLI chạy song song) – bỏ qua.
has_profile() {
  for f in "$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles"/*.mobileprovision \
           "$HOME/Library/MobileDevice/Provisioning Profiles"/*.mobileprovision; do
    [ -f "$f" ] || continue
    P=$(security cms -D -i "$f" 2>/dev/null) || continue
    grep -q "$LIBRARIAN_IPAD_TEAM.$BUNDLE_ID" <<<"$P" && { [ -z "$UDID" ] || grep -q "$UDID" <<<"$P"; } && return 0
  done
  return 1
}
if [ -n "$IPA_ONLY" ] && ! has_profile; then
  echo "Mac này chưa có provisioning profile cho $BUNDLE_ID. Cần cài trực tiếp một lần (cắm $KIND vào Mac này, chạy"
  echo "không có --ipa) để đăng ký thiết bị vào Personal Team $LIBRARIAN_IPAD_TEAM."; exit 1
fi
if ! has_profile; then
  echo "▶ Đăng ký $KIND vào Personal Team và tạo provisioning profile (lần đầu)"
  xcodebuild -project gen/apple/*.xcodeproj -scheme "$(basename gen/apple/*.xcodeproj .xcodeproj)_iOS" \
    -destination "id=$UDID" -configuration release -allowProvisioningUpdates -allowProvisioningDeviceRegistration \
    DEVELOPMENT_TEAM="$LIBRARIAN_IPAD_TEAM" build >"$TMP/provision.log" 2>&1 || true
  has_profile || { grep -E "error:" "$TMP/provision.log" | head -5; echo "Chưa tạo được profile. Xem docs/IPAD.md mục Nếu gặp lỗi."; exit 1; }
fi

# `ios build` build + ký .app rồi archive/export ra .ipa. Bước archive có thể lỗi errSecInternalComponent (codesign
# không đọc được khoá trong Keychain) dù .app đã ký xong. Ta cài thẳng .app nên chỉ cần bước build thành công.
# Keychain ký riêng (scripts/ipad-keychain-setup.sh): sau khi khởi động lại Mac nó bị khoá → mở bằng mật khẩu đã lưu.
SIGN_KC="$HOME/Library/Keychains/ipad-signing.keychain-db"
if [ -f "$SIGN_KC" ]; then
  KC_PW=$(security find-generic-password -s librarian-ipad-signing -a "$USER" -w 2>/dev/null || true)
  if [ -n "$KC_PW" ]; then security unlock-keychain -p "$KC_PW" "$SIGN_KC"; else security unlock-keychain "$SIGN_KC"; fi
  unset KC_PW
fi

echo "▶ Build (release, ký bằng Personal Team $LIBRARIAN_IPAD_TEAM)"
touch "$TMP/start"
# Số phiên bản: ghi đè "version" của tauri.conf.json khi build (không sửa file trong repo).
VERSION_ARGS=()
if [ -n "${LIBRARIAN_IPAD_VERSION:-}" ]; then
  printf '{"version":"%s"}\n' "$LIBRARIAN_IPAD_VERSION" > "$TMP/version.json"
  VERSION_ARGS=(--config "$TMP/version.json")
  echo "  phiên bản $LIBRARIAN_IPAD_VERSION"
fi
cargo tauri ios build --export-method debugging ${VERSION_ARGS[@]+"${VERSION_ARGS[@]}"} \
  || echo "  (bước đóng gói .ipa lỗi – dùng .app đã ký nếu có)"

APP=$(find "$HOME/Library/Developer/Xcode/DerivedData" -path '*/Build/Products/release-iphoneos/*.app' -maxdepth 6 \
  -type d -newer "$TMP/start" 2>/dev/null | head -1)
[ -n "$APP" ] || { echo "Build không ra .app mới – xem lỗi phía trên."; exit 1; }
codesign --verify --strict "$APP" 2>/dev/null || {
  echo "$APP chưa được ký hợp lệ (thường là errSecInternalComponent: codesign không được phép dùng khoá ký)."
  echo "Chạy một lần (nhập mật khẩu đăng nhập Mac) rồi chạy lại script:"
  echo "  security set-key-partition-list -S apple-tool:,apple:,codesign: -s ~/Library/Keychains/login.keychain-db"
  echo "Không nhớ mật khẩu đăng nhập Mac: chạy ./scripts/ipad-keychain-setup.sh (tạo keychain ký riêng) rồi chạy lại."
  exit 1; }
echo "  → $APP"

# .ipa = thư mục Payload/<app>.app nén zip. Giữ nguyên chữ ký nên cài được từ máy khác (Windows/Mac cũ) lên các
# thiết bị có trong provisioning profile – không cần ký lại.
PROFILE_PLIST="$TMP/profile.plist"
security cms -D -i "$APP/embedded.mobileprovision" >"$PROFILE_PLIST" 2>/dev/null || : >"$PROFILE_PLIST"
EXPIRES=$(plutil -extract ExpirationDate raw -o - "$PROFILE_PLIST" 2>/dev/null || true)
NDEV=$(plutil -extract ProvisionedDevices json -o - "$PROFILE_PLIST" 2>/dev/null | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{try{console.log(JSON.parse(s).length)}catch{console.log("?")}})')
LEFT=$(node -e 'const t=Date.parse(process.argv[1]);console.log(isNaN(t)?"?":((t-Date.now())/864e5).toFixed(1))' "$EXPIRES")
VER_TAG="${LIBRARIAN_IPAD_VERSION:-dev}"
IPA_NAME="My-Librarian-$VER_TAG.ipa"
mkdir -p "$TMP/ipa/Payload"
ditto "$APP" "$TMP/ipa/Payload/$(basename "$APP")"
( cd "$TMP/ipa" && zip -qry "$TMP/$IPA_NAME" Payload ) || { echo "Không tạo được .ipa"; exit 1; }
echo "  .ipa: $IPA_NAME – hạn ký đến $EXPIRES (còn $LEFT ngày), dùng được cho $NDEV thiết bị đã đăng ký"

if [ -z "$IPA_ONLY" ]; then
  echo "▶ Cài lên $UDID"
  xcrun devicectl device install app --device "$UDID" "$APP"
fi
if [ -n "${LIBRARIAN_IPAD_OUT:-}" ]; then
  mkdir -p "$LIBRARIAN_IPAD_OUT"
  ditto "$APP" "$LIBRARIAN_IPAD_OUT/$(basename "$APP")"
  cp "$TMP/$IPA_NAME" "$LIBRARIAN_IPAD_OUT/"
  echo "  đã lưu bản build: $LIBRARIAN_IPAD_OUT"
fi
if [ -n "${LIBRARIAN_IPA_SHARE:-}" ]; then
  mkdir -p "$LIBRARIAN_IPA_SHARE" && cp "$TMP/$IPA_NAME" "$LIBRARIAN_IPA_SHARE/"     && echo "  đã chép .ipa vào: $LIBRARIAN_IPA_SHARE" || echo "  (không chép được .ipa vào $LIBRARIAN_IPA_SHARE)"
fi
case "$LEFT" in [0-2].*) echo "  ⚠ Chữ ký còn dưới 3 ngày – Xcode sẽ cấp profile mới khi profile cũ hết hạn; build lại sau đó." ;; esac
if [ -n "$IPA_ONLY" ]; then
  echo "✔ Xong. Cài .ipa: cắm $KIND vào máy Windows → install-ios.cmd (hoặc Mac cũ → install-ios.command)."
else
  echo "✔ Xong. Lần đầu: $KIND → Cài đặt → Cài đặt chung → VPN & Quản lý thiết bị → tin cậy Apple ID của bạn. Nhớ chạy lại script này trước khi hết 7 ngày."
fi
