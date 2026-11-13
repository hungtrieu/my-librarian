#!/usr/bin/env bash
# Cài file .ipa 47 Librarian (build ở Mac khác bằng build-ipad.command --ipa) lên iPad/iPhone cắm cáp vào Mac này.
# Dành cho Mac không build được (vd. MacBook Pro 2016 – không chạy được Xcode mới). Không ký lại, không cần Xcode.
#   ./install-ios.command                 lấy file 47-Librarian-*.ipa mới nhất (thư mục ghi trong .ipa-share,
#                                         $LIBRARIAN_IPA_DIR, dist/ipad, dist/iphone, ~/Downloads)
#   ./install-ios.command <file.ipa>      cài file chỉ định
#   LIBRARIAN_IPAD_DEVICE=<UDID> ./install-ios.command    chọn máy khi cắm nhiều máy
# Công cụ cài: go-ios (MIT, github.com/danielpaulus/go-ios) – lần đầu tự tải về ~/Library/Caches/47-librarian/go-ios (cần Node.js).
set -euo pipefail
cd "$(dirname "$0")"
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
GO_IOS_VERSION=1.3.2

say() { printf '\n\033[1;36m%s\033[0m\n' "$*"; }
ok() { printf '\n\033[1;32m%s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31m%s\033[0m\n' "$*"; exit 1; }

case "$(uname -m)" in x86_64) ARCH=amd64 ;; arm64) ARCH=arm64 ;; *) fail "Kiến trúc máy không hỗ trợ: $(uname -m)" ;; esac
TOOL_DIR="$HOME/Library/Caches/47-librarian/go-ios/$GO_IOS_VERSION"
IOS="$TOOL_DIR/ios"
if [ ! -x "$IOS" ]; then
  say "Tải công cụ cài go-ios $GO_IOS_VERSION (một lần, ~150 MB gói npm)"
  command -v npm >/dev/null || fail "Thiếu Node.js/npm (cài từ https://nodejs.org) – cần để tải go-ios."
  DL=$(mktemp -d)
  # npm pack chỉ tải gói, không chạy script cài đặt của gói.
  ( cd "$DL" && npm pack "go-ios@$GO_IOS_VERSION" --silent >/dev/null ) || fail "Không tải được go-ios (kiểm mạng)."
  tar -xzf "$DL"/go-ios-*.tgz -C "$DL" "package/dist/go-ios-darwin-${ARCH}_darwin_${ARCH}/ios" || fail "Không giải nén được go-ios."
  mkdir -p "$TOOL_DIR"
  mv "$DL/package/dist/go-ios-darwin-${ARCH}_darwin_${ARCH}/ios" "$IOS"
  chmod +x "$IOS"
  xattr -d com.apple.quarantine "$IOS" 2>/dev/null || true
  rm -rf "$DL"
fi

# --- File .ipa
IPA="${1:-}"
if [ -z "$IPA" ]; then
  DIRS=()
  if [ -s .ipa-share ]; then d=$(head -1 .ipa-share | sed 's/[[:space:]]*$//'); DIRS+=("${d/#\~/$HOME}"); fi
  [ -n "${LIBRARIAN_IPA_DIR:-}" ] && DIRS+=("$LIBRARIAN_IPA_DIR")
  DIRS+=("$PWD/dist/ipad" "$PWD/dist/iphone" "$HOME/Downloads")
  IPA=""
  for d in "${DIRS[@]}"; do
    [ -d "$d" ] || continue
    while IFS= read -r f; do
      if [ -z "$IPA" ] || [ "$f" -nt "$IPA" ]; then IPA="$f"; fi
    done < <(find "$d" -maxdepth 3 -name '47-Librarian-*.ipa' -type f 2>/dev/null)
  done
  [ -n "$IPA" ] || fail "Không thấy file 47-Librarian-*.ipa. Chạy: ./install-ios.command <file.ipa>, hoặc ghi thư mục chứa .ipa vào file .ipa-share. Đã tìm trong: ${DIRS[*]}"
fi
[ -f "$IPA" ] || fail "Không có file: $IPA"
say "File: $IPA ($(du -h "$IPA" | cut -f1))"

# --- Hạn ký + các máy được phép cài (provisioning profile trong .ipa)
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
PROF=$(unzip -Z1 "$IPA" | grep -E '^Payload/[^/]+\.app/embedded\.mobileprovision$' | head -1 || true)
[ -n "$PROF" ] || fail "File .ipa không có embedded.mobileprovision – không phải bản đã ký cho máy cụ thể."
unzip -p "$IPA" "$PROF" > "$TMP/p.mobileprovision"
security cms -D -i "$TMP/p.mobileprovision" > "$TMP/p.plist" 2>/dev/null || : > "$TMP/p.plist"
EXPIRES=$(plutil -extract ExpirationDate raw -o - "$TMP/p.plist" 2>/dev/null || true)
UDIDS=$(plutil -extract ProvisionedDevices xml1 -o - "$TMP/p.plist" 2>/dev/null | sed -n 's:.*<string>\(.*\)</string>.*:\1:p' | tr '[:lower:]' '[:upper:]' || true)
if [ -n "$EXPIRES" ]; then
  EXP_EPOCH=$(date -j -u -f '%Y-%m-%dT%H:%M:%SZ' "$EXPIRES" +%s 2>/dev/null || echo 0)
  NOW=$(date +%s)
  [ "$EXP_EPOCH" -gt "$NOW" ] || fail "Bản này đã hết hạn ký ($EXPIRES). Build lại trên Mac build: ./build-ipad.command --ipa"
  LEFT_H=$(( (EXP_EPOCH - NOW) / 3600 ))
  echo "Hạn ký: $(date -r "$EXP_EPOCH" '+%d/%m/%Y %H:%M') (còn $((LEFT_H / 24)) ngày $((LEFT_H % 24)) giờ). App ngừng mở được sau thời điểm này."
fi

# --- Chọn máy
LIST=$("$IOS" list 2>/dev/null | sed -n 's/.*"deviceList":\[\(.*\)\].*/\1/p' | tr -d '"' | tr ',' '\n' | sed '/^$/d' || true)
[ -n "$LIST" ] || fail 'Không thấy iPad/iPhone. Cắm cáp (cáp truyền dữ liệu), mở khoá máy, bấm "Tin cậy" rồi chạy lại.'
if [ -n "${LIBRARIAN_IPAD_DEVICE:-}" ]; then
  UDID=$(grep -ix "$LIBRARIAN_IPAD_DEVICE" <<<"$LIST" | head -1 || true)
  [ -n "$UDID" ] || fail "Không thấy máy $LIBRARIAN_IPAD_DEVICE. Đang cắm: $(echo $LIST)"
elif [ "$(wc -l <<<"$LIST" | tr -d ' ')" -gt 1 ]; then
  UDID=""
  for u in $LIST; do grep -qx "$(tr '[:lower:]' '[:upper:]' <<<"$u")" <<<"$UDIDS" && { [ -z "$UDID" ] || fail "Đang cắm nhiều máy: $(echo $LIST). Chọn: LIBRARIAN_IPAD_DEVICE=<UDID> ./install-ios.command"; UDID=$u; }; done
  [ -n "$UDID" ] || fail "Đang cắm nhiều máy: $(echo $LIST). Chọn: LIBRARIAN_IPAD_DEVICE=<UDID> ./install-ios.command"
else
  UDID="$LIST"
fi
NAME=$("$IOS" devicename --udid="$UDID" 2>/dev/null | sed -n 's/.*"devicename":"\([^"]*\)".*/\1/p' || true)
say "Máy: ${NAME:-?} ($UDID)"
if [ -n "$UDIDS" ] && ! grep -qx "$(tr '[:lower:]' '[:upper:]' <<<"$UDID")" <<<"$UDIDS"; then
  fail "Máy này chưa có trong chữ ký của bản build. Cắm máy này vào Mac build một lần, chạy build-ipad.command / build-iphone.command (không có --ipa) để đăng ký, rồi build lại .ipa."
fi

say "Cài app (giữ nguyên dữ liệu)…"
if ! OUT=$("$IOS" install --path="$IPA" --udid="$UDID" 2>&1) || grep -qE '"level":"(error|fatal)"' <<<"$OUT"; then
  echo "$OUT"
  fail "Cài lỗi. Thử: mở khoá máy và cắm lại cáp; máy bật Chế độ nhà phát triển; còn dung lượng. Lỗi về chữ ký/profile: build lại .ipa trên Mac."
fi
ok "XONG – đã cài 47 Librarian."
echo "Lần đầu cài trên máy này: Cài đặt → Cài đặt chung → VPN & Quản lý thiết bị → tin cậy Apple ID của bạn."
