#!/usr/bin/env bash
# Tạo keychain riêng để ký app iPad, dùng khi không nhớ mật khẩu đăng nhập Mac (không mở được quyền codesign
# trên login keychain → lỗi errSecInternalComponent). Chạy MỘT lần trong Terminal trên Mac:
#   ./scripts/ipad-keychain-setup.sh
# Script:
#   1. tạo ~/Library/Keychains/ipad-signing.keychain-db với mật khẩu bạn tự đặt (không tự khoá khi ngủ/hết giờ);
#   2. lưu mật khẩu đó vào login keychain (mục "librarian-ipad-signing") để ipad-install.sh tự mở khoá sau khi khởi động lại;
#   3. tạm đặt keychain mới làm mặc định để Xcode tạo chứng chỉ Apple Development mới vào đó; bạn thu hồi chứng chỉ cũ;
#   4. cho phép codesign dùng khoá mới không cần hỏi, rồi ký thử.
set -euo pipefail

KC="$HOME/Library/Keychains/ipad-signing.keychain-db"
LOGIN_KC="$HOME/Library/Keychains/login.keychain-db"
SERVICE=librarian-ipad-signing
BUNDLE_ID=vn.hungtrieu.mylibrarian.ipad

if [ ! -f "$KC" ]; then
  echo "▶ Đặt mật khẩu cho keychain ký iPad (mật khẩu MỚI, tự chọn, ghi lại ở chỗ an toàn)"
  while :; do
    read -rsp "  Mật khẩu: " PW; echo
    read -rsp "  Nhập lại: " PW2; echo
    [ -n "$PW" ] && [ "$PW" = "$PW2" ] && break
    echo "  Không khớp hoặc để trống, nhập lại."
  done
  security create-keychain -p "$PW" "$KC"
else
  echo "▶ Đã có $KC"
  read -rsp "  Mật khẩu keychain ký iPad: " PW; echo
fi
security unlock-keychain -p "$PW" "$KC"
security set-keychain-settings "$KC"   # không tự khoá theo thời gian / khi ngủ

# Thêm vào danh sách tìm kiếm (giữ các keychain cũ).
CURRENT=()
while IFS= read -r line; do
  k=$(sed -E 's/^[[:space:]]*"(.*)"$/\1/' <<<"$line")
  [ "$k" = "$KC" ] || [ "$k" = "/Library/Keychains/System.keychain" ] || CURRENT+=("$k")
done < <(security list-keychains -d user)
security list-keychains -d user -s "$KC" "${CURRENT[@]}"

# Lưu mật khẩu vào login keychain; item do /usr/bin/security tạo thì chính nó đọc lại được, không hỏi.
security add-generic-password -U -s "$SERVICE" -a "$USER" -w "$PW" "$LOGIN_KC"

if ! security find-identity -v -p codesigning "$KC" | grep -q "Apple Development"; then
  OLD_DEFAULT=$(security default-keychain -d user | sed -E 's/^[[:space:]]*"(.*)"$/\1/')
  security default-keychain -d user -s "$KC"
  trap 'security default-keychain -d user -s "$OLD_DEFAULT"' EXIT
  cat <<'EOF'

▶ Làm trong Xcode (keychain mới đang là mặc định, chứng chỉ mới sẽ vào đó):
  1. Xcode → Settings… → Accounts → chọn Apple ID → chọn team "(Personal Team)" → Manage Certificates…
  2. Chuột phải (Control-click) vào chứng chỉ "Apple Development" CŨ → Revoke Certificate → Revoke.
     (Tài khoản miễn phí chỉ có 1 chứng chỉ Development; chưa thu hồi thì "+" báo
      "You already have a current Development certificate". Delete KHÔNG đủ – phải Revoke.)
  3. Bấm "+" (góc dưới trái) → Apple Development. Đợi chứng chỉ mới hiện ra.
  4. Bấm Done.
  Nếu macOS hỏi mật khẩu keychain "ipad-signing": nhập mật khẩu vừa đặt ở trên.
EOF
  read -rp "Xong thì nhấn Enter… " _
  until security find-identity -v -p codesigning "$KC" | grep -q "Apple Development"; do
    read -rp "Chưa thấy chứng chỉ trong keychain ipad-signing. Làm lại bước 2 rồi nhấn Enter (Ctrl-C để thoát)… " _
  done
  security default-keychain -d user -s "$OLD_DEFAULT"; trap - EXIT
fi

echo "▶ Cho phép codesign dùng khoá không cần hỏi"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$PW" "$KC" >/dev/null

ID=$(security find-identity -v -p codesigning "$KC" | awk '/Apple Development/ {print $2; exit}')
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
cp /bin/echo "$TMP/t"
if codesign -f -s "$ID" --keychain "$KC" "$TMP/t" 2>"$TMP/err"; then
  echo "  ký thử OK bằng $ID"
else
  cat "$TMP/err"; echo "Ký thử lỗi – xem trên."; exit 1
fi

# Profile cũ chỉ chứa chứng chỉ cũ → bỏ đi để ipad-install.sh tạo lại (chuyển sang thư mục backup, không xoá).
BACKUP="$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles (backup)"
for f in "$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles"/*.mobileprovision \
         "$HOME/Library/MobileDevice/Provisioning Profiles"/*.mobileprovision; do
  [ -f "$f" ] || continue
  if security cms -D -i "$f" 2>/dev/null | grep -q "\.$BUNDLE_ID<"; then
    mkdir -p "$BACKUP"; mv "$f" "$BACKUP/"; echo "  đã cất profile cũ: $(basename "$f")"
  fi
done

echo "✔ Xong. Chạy lại ./build-ipad.command như thường."
