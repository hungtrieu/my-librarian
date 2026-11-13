#!/usr/bin/env bash
# Build và cài app My Librarian lên iPad – bấm đúp file này trong Finder (máy Mac), hoặc chạy ./build-ipad.command
#   - Lấy code mới nhất (git pull) nếu trên Mac không có thay đổi dở.
#   - Mã nguồn không đổi kể từ lần cài trước: không build, chỉ báo. Riêng khi bản trên iPad sắp hết hạn 7 ngày
#     (Apple ID miễn phí) thì tự build + cài lại, giữ nguyên số phiên bản.
#   - Có thay đổi: tự tăng phiên bản, chạy test, build, ký, cài lên iPad.
#   Build lại dù không đổi gì: ./build-ipad.command --force
#   Không có iPad ở đây (vd. remote vào Mac công ty): ./build-ipad.command --ipa → chỉ build + xuất file .ipa đã ký,
#     mang về cài bằng install-ios.cmd (Windows) / install-ios.command (Mac khác). Thư mục chép .ipa sang (Google Drive,
#     OneDrive, iCloud Drive…): ghi đường dẫn vào file .ipa-share (không commit) hoặc đặt LIBRARIAN_IPA_SHARE.
# Chuẩn bị máy Mac một lần: docs/IPAD.md (mục "Chuẩn bị một lần").
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
# Cùng app cho iPhone: build-iphone.command đặt LIBRARIAN_DEVICE_KIND=iPhone (trạng thái build/hết hạn theo dõi riêng).
DEV="${LIBRARIAN_DEVICE_KIND:-iPad}"
case "$DEV" in iPhone) TARGET=iphone ;; *) DEV=iPad; TARGET=ipad ;; esac
export LIBRARIAN_DEVICE_KIND="$DEV"
DOC=docs/IPAD.md; [ "$DEV" = iPhone ] && DOC=docs/IPHONE.md

say() { printf '\n\033[1;36m%s\033[0m\n' "$*"; }
ok() { printf '\n\033[1;32m%s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31m%s\033[0m\n' "$*"; exit 1; }

command -v node >/dev/null || fail "Thiếu Node.js – cài từ https://nodejs.org rồi chạy lại."
command -v cargo >/dev/null || fail "Thiếu Rust – xem $DOC mục Chuẩn bị một lần."

# Team ID (trường OU của chứng chỉ "Apple Development") – hỏi một lần, lưu vào .ipad-team (không commit).
if [ -z "${LIBRARIAN_IPAD_TEAM:-}" ]; then
  if [ -s .ipad-team ]; then
    LIBRARIAN_IPAD_TEAM=$(tr -d '[:space:]' < .ipad-team)
  else
    echo "Cần Team ID của Apple ID (10 ký tự). Lấy bằng lệnh:"
    echo '  security find-certificate -c "Apple Development" -p | openssl x509 -noout -subject   (lấy phần OU=…)'
    read -rp "Team ID: " LIBRARIAN_IPAD_TEAM
    [ -n "$LIBRARIAN_IPAD_TEAM" ] || fail "Chưa nhập Team ID."
    echo "$LIBRARIAN_IPAD_TEAM" > .ipad-team
  fi
fi
export LIBRARIAN_IPAD_TEAM

IPA_ONLY=""
for a in "$@"; do [ "$a" = --ipa ] && IPA_ONLY=1; done
if [ -z "${LIBRARIAN_IPA_SHARE:-}" ] && [ -s .ipa-share ]; then
  LIBRARIAN_IPA_SHARE=$(head -1 .ipa-share | sed 's/[[:space:]]*$//')
  LIBRARIAN_IPA_SHARE="${LIBRARIAN_IPA_SHARE/#\~/$HOME}"
fi
export LIBRARIAN_IPA_SHARE="${LIBRARIAN_IPA_SHARE:-}"

# Lấy code mới (bỏ qua nếu Mac đang có sửa đổi chưa commit, để không ghi đè).
if git diff --quiet && git diff --cached --quiet; then
  say "Lấy code mới nhất (git pull)"
  git pull --ff-only || echo "  (không pull được – build bằng code hiện có trên Mac)"
else
  echo "Trên Mac có thay đổi chưa commit – không git pull, build bằng code hiện có."
fi

eval "$(node scripts/release-state.mjs check "$TARGET" --shell "$@")"
if [ "$action" = none ]; then
  ok "Không có thay đổi mã nguồn kể từ bản $lastVersion (build lúc $lastBuiltAt) – không build gì."
  echo "Bản mới nhất: $lastFolder"
  if [ -n "$IPA_ONLY" ]; then
    IPA=$(ls -t "$lastFolder"/*.ipa 2>/dev/null | head -1 || true)
    if [ -n "$IPA" ]; then
      echo "File .ipa: $IPA"
      [ -n "$LIBRARIAN_IPA_SHARE" ] && cp "$IPA" "$LIBRARIAN_IPA_SHARE/" && echo "Đã chép vào: $LIBRARIAN_IPA_SHARE"
    else
      echo "Bản này chưa có file .ipa – chạy ./build-$TARGET.command --ipa --force để build lại."
    fi
  fi
  echo "App trên $DEV hết hạn 7 ngày sau lần cài; khi gần hết hạn, chạy lại file này là tự cài lại."
  exit 0
fi

say "Bản $DEV $version – lý do: $reason"
say "Kiểm tra (test phần đọc sách dùng chung)"
cargo test -p librarian-core --no-default-features --features reader --lib --quiet || fail "TEST HỎNG – không build, phiên bản giữ nguyên."

OUT="$PWD/dist/$TARGET/$version-$(date +%Y%m%d-%H%M%S)"
LIBRARIAN_IPA_ONLY="$IPA_ONLY" LIBRARIAN_IPAD_VERSION="$version" LIBRARIAN_IPAD_OUT="$OUT" scripts/ipad-install.sh || fail "BUILD/CÀI HỎNG – xem lỗi phía trên và bảng 'Nếu gặp lỗi' trong $DOC. Phiên bản giữ nguyên."

node scripts/release-state.mjs record "$TARGET" "$version" "$OUT" "$fingerprint"
if [ -n "$IPA_ONLY" ]; then
  ok "XONG – đã build bản $DEV $version (chưa cài)."
  echo "File .ipa: $(ls "$OUT"/*.ipa 2>/dev/null | head -1)"
  echo "Cài: cắm $DEV vào máy Windows → bấm đúp install-ios.cmd (Mac khác: install-ios.command). Xem $DOC."
  exit 0
fi
ok "XONG – đã cài bản $DEV $version lên $DEV."
echo "Bản build: $OUT"
echo "Lần đầu cài: $DEV → Cài đặt → Cài đặt chung → VPN & Quản lý thiết bị → tin cậy Apple ID của bạn."
