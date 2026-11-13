# My Librarian trên điện thoại Android – cài và dùng

App Android đọc sách đã gửi từ app Windows qua Wi-Fi nhà. **Không dùng Google Play**: bạn tự build file APK trên Mac rồi cài lên điện thoại (qua cáp, hoặc chép file APK sang). Hướng dẫn này đủ để cài Android, không cần đọc tài liệu iPad/iPhone. (Cài iPhone: [IPHONE.md](IPHONE.md), iPad: [IPAD.md](IPAD.md).)

> **Trạng thái (2026-11-11):** xem mục [Đã kiểm gì](#đã-kiểm-gì) ở cuối.

## Khác gì bản iPhone
Cùng mã nguồn với bản iPad/iPhone (`crates/ipad`, giao diện `ui/ipad/`), và dùng **bố cục iPhone**:
- **Chỉ sách chữ:** EPUB, MOBI/AZW/AZW3, FB2, TXT. Với máy tính, điện thoại Android được coi như iPhone: Windows không xếp PDF và truyện tranh (CBZ/CBR) vào hàng chờ, app cũng tự bỏ qua nếu lỡ có.
- **Cầm tay trái:** trong trình đọc, thanh nút nhanh ở **góc dưới trái** (đánh dấu trang, mục lục / dấu trang, tìm trong sách, Aa); nút quay lại thư viện (‹) và "⋯" ở bên phải.
- Chạm giữa trang để hiện/ẩn thanh nút; chạm mép trái/phải để lật trang.

Riêng Android:
- **Nút / cử chỉ Quay lại** của hệ thống đóng lớp trên cùng: bảng chọn → bảng mục lục/tìm kiếm trong trình đọc → trình đọc → trang Đồng bộ / bộ sách. Ở trang Thư viện thì thoát app.
- **Không hết hạn 7 ngày** như iPhone. Cài một lần dùng mãi; chỉ cần build lại khi có code mới.
- App có mã riêng `vn.hungtrieu.mylibrarian.android` (đặt ở `crates/ipad/tauri.android.conf.json`). Cần Android 8.0 trở lên.
- Sách nằm trong vùng dữ liệu riêng của app (`…/Android/data` không thấy; không hiện trong app Tệp). Gỡ app là mất sách đã tải, nhưng gửi lại từ máy tính được.

## Chuẩn bị một lần trên Mac
1. Cài Rust, Node.js và tauri-cli (đã làm cho iPad/iPhone thì bỏ qua):
   ```bash
   curl https://sh.rustup.rs -sSf | sh
   ```
   ```bash
   cargo install tauri-cli --version "^2"
   ```
2. Cài **JDK 17** (hoặc mới hơn):
   ```bash
   brew install openjdk@17
   ```
3. Cài **Android SDK + NDK**. Cách dễ nhất: cài [Android Studio](https://developer.android.com/studio), mở một lần, rồi vào Settings → Languages & Frameworks → Android SDK → thẻ **SDK Tools** → tick **NDK (Side by side)**, **Android SDK Command-line Tools**, **Android SDK Platform-Tools** → Apply. SDK nằm ở `~/Library/Android/sdk`.
   Không dùng Android Studio thì cài bằng dòng lệnh (sau khi có command-line tools):
   ```bash
   ~/Library/Android/sdk/cmdline-tools/latest/bin/sdkmanager "platform-tools" "platforms;android-36" "build-tools;36.0.0" "ndk;27.1.12297006"
   ```
4. Thêm target Rust cho Android (script cũng tự làm):
   ```bash
   rustup target add aarch64-linux-android armv7-linux-androideabi
   ```
5. **Bật Gỡ lỗi USB trên điện thoại** (để script cài thẳng qua cáp): Cài đặt → Giới thiệu về điện thoại → chạm **Số bản dựng** 7 lần → quay lại Cài đặt → Hệ thống → **Tuỳ chọn nhà phát triển** → bật **Gỡ lỗi USB**. Cắm cáp vào Mac, trên điện thoại bấm **Cho phép** gỡ lỗi từ máy tính này. Kiểm tra:
   ```bash
   ~/Library/Android/sdk/platform-tools/adb devices
   ```
   Thấy một dòng kết thúc bằng `device` là xong. (`unauthorized` = chưa bấm Cho phép trên điện thoại.)
   **Máy Xiaomi / Redmi / POCO:** mục cần chạm 7 lần là **Phiên bản OS**; Tuỳ chọn nhà phát triển nằm trong Cài đặt bổ sung; phải bật thêm **Cài đặt qua USB** thì script mới cài được.
   Không muốn bật Gỡ lỗi USB: bỏ qua bước này, xem "Cài bằng file APK" bên dưới.

Script tự tìm JDK (Homebrew hoặc Android Studio), SDK ở `~/Library/Android/sdk` và bản NDK mới nhất trong đó. Cài ở chỗ khác thì đặt `JAVA_HOME`, `ANDROID_HOME`, `NDK_HOME` trước khi chạy.

## Cài app
Bấm đúp **`build-android.command`** ở thư mục gốc dự án, hoặc chạy trong Terminal:
```bash
./build-android.command
```
- Script tự `git pull`, chạy test, build APK rồi lưu vào `dist/android/<phiên bản>-…/My-Librarian-<phiên bản>.apk`.
- Điện thoại đang cắm cáp (đã bật Gỡ lỗi USB) thì script **cài luôn**. Cài đè giữ nguyên sách, vị trí đọc, dấu trang.
- Lần build đầu mất 10–20 phút (tải Gradle, biên dịch Rust cho Android); các lần sau nhanh hơn nhiều.
- Mã nguồn không đổi thì không build lại; nếu có điện thoại đang cắm thì cài lại bản APK gần nhất. Build lại bằng mọi giá:
  ```bash
  ./build-android.command --force
  ```
- **Có nhiều điện thoại / máy ảo** đang nối: chọn bằng serial (cột đầu của `adb devices`):
  ```bash
  LIBRARIAN_ANDROID_DEVICE=R58M123ABC ./build-android.command
  ```

**Cài bằng file APK (không cần cáp):** gửi file `My-Librarian-<phiên bản>.apk` sang điện thoại (Google Drive, Zalo, cáp chép file…), mở file trên điện thoại → cho phép "Cài ứng dụng không rõ nguồn gốc" cho app vừa dùng để mở → Cài đặt. Google Play Protect có thể hỏi → **Vẫn cài đặt**.

**Về chữ ký:** APK là bản *debug*, ký bằng **khoá chung** `crates/ipad/android/debug.keystore` (script `scripts/android-signing.mjs` gắn khoá này vào dự án Gradle mỗi lần build). File khoá **không commit vào git** (là khoá riêng, đã có trong `.gitignore`). Lần đầu chưa có file này, script lấy khoá debug của máy đang build (`~/.android/debug.keystore`) làm khoá chung. Build trên nhiều máy (Mac, Windows) mà muốn cài đè được: tự chép file này sang đúng đường dẫn đó ở máy kia (USB, thư mục đồng bộ riêng…). Phần Rust vẫn biên dịch tối ưu nên tốc độ không kém bản release.

APK mặc định chứa mã cho CPU ARM 64-bit và 32-bit (mọi điện thoại hiện nay). Chạy trên máy ảo Intel thì thêm `LIBRARIAN_ANDROID_TARGETS="aarch64 x86_64"`.

## Ghép với app Windows
1. Windows: My Librarian → mục **iPad / iPhone** → **Bật kết nối**. Lần đầu Windows hỏi tường lửa → cho phép mạng riêng tư.
2. Windows: **Hiện mã QR mới** (mỗi máy một mã, mã dùng một lần, hết hạn sau 5 phút).
3. Điện thoại: mở app → biểu tượng ⇅ **Đồng bộ** (góc dưới trái) → đặt **Tên máy này**, ví dụ "Điện thoại của Hưng". Tên này hiện trên Windows để phân biệt khi có nhiều máy. Sau đó **Quét mã QR** (cho phép camera) hoặc mở "Nhập tay" và gõ IP + mã 6 số.
4. Windows: chuột phải sách hoặc bộ sách → **Gửi sang <tên điện thoại>** (hoặc "Gửi sang tất cả … thiết bị"). PDF và truyện tranh tự bị bỏ qua cho điện thoại; Windows báo số cuốn bị bỏ.
5. Điện thoại: ⇅ → **Tải**. Mặc định app tự tải khi mở nếu thấy máy tính.

Hai máy phải cùng mạng Wi-Fi (không dùng mạng khách hay mạng cô lập thiết bị). Trên Windows, điện thoại Android hiện trong danh sách thiết bị như một iPhone (loại "điện thoại"), có hàng chờ riêng, dung lượng trống và nút **Xem sách đã có trên máy** – xem thêm ở [IPHONE.md](IPHONE.md#ghép-với-app-windows).

## Đọc và đánh dấu trang
- Mở sách: chạm bìa. Thanh nút ẩn sau vài giây; chạm giữa trang để hiện lại.
- **Đánh dấu trang:** bấm biểu tượng dấu trang (nút đầu tiên, góc dưới trái). Bấm lần nữa để bỏ dấu.
- **Danh sách dấu trang:** nút mục lục (thứ hai) → thẻ **Dấu trang** → chạm để nhảy tới, ✕ để xoá.
- Về thư viện: nút ‹ hoặc nút/cử chỉ Quay lại của Android.
- Xoá sách khỏi điện thoại rồi gửi lại: vị trí đọc và dấu trang vẫn còn.

## Nếu gặp lỗi
| Triệu chứng | Thử |
|---|---|
| Script báo "Thiếu JDK" / "Không thấy Android SDK" / "Thiếu Android NDK" | Làm bước 2–3 của mục Chuẩn bị. Cài ở chỗ khác: đặt `JAVA_HOME`, `ANDROID_HOME`, `NDK_HOME`. |
| "Không thấy điện thoại nào qua adb" | Cắm cáp dữ liệu, mở khoá điện thoại, bật Gỡ lỗi USB, bấm Cho phép. Hoặc cài bằng file APK. |
| `adb devices` hiện `unauthorized` | Mở khoá điện thoại, bấm **Cho phép** ở hộp thoại gỡ lỗi USB; không thấy hộp thoại thì rút cắm lại cáp. |
| `INSTALL_FAILED_USER_RESTRICTED: Install canceled by user` (Xiaomi / Redmi / POCO) | Tuỳ chọn nhà phát triển → bật **Cài đặt qua USB** (cần đăng nhập tài khoản Mi và có SIM). Khi script cài, điện thoại hiện hộp thoại đếm ngược 10 giây → bấm **Cài đặt**. Không bật được: cài bằng file APK. |
| `INSTALL_FAILED_UPDATE_INCOMPATIBLE` / "chữ ký không khớp" | Bản đang cài được ký bằng khoá khác `crates/ipad/android/debug.keystore` (bản build trước khi có khoá chung). Gỡ app trên điện thoại rồi cài lại một lần; hoặc chép `~/.android/debug.keystore` của máy đã build bản đang cài vào `crates/ipad/android/debug.keystore` để cài đè không mất sách. |
| `INSTALL_FAILED_NO_MATCHING_ABIS` | Máy ảo Intel: build lại với `LIBRARIAN_ANDROID_TARGETS="aarch64 x86_64"`. |
| Build lỗi ở bước Gradle sau khi nâng cấp tauri-cli | Xoá `crates/ipad/gen/android` (thư mục sinh tự động, không commit) rồi chạy lại script. |
| "Không thấy máy tính" | Windows đang chạy My Librarian và đã bật kết nối? Cùng Wi-Fi? Tường lửa cho phép? IP máy tính đổi → ghép lại. Tắt VPN / "DNS riêng tư" trên điện thoại nếu có. |
| Gửi sách mà điện thoại không tải | Sách là PDF/truyện tranh (điện thoại không nhận). Hoặc đã gửi nhầm sang máy khác: xem hàng chờ từng máy trên Windows. |
| Camera không bật khi quét QR | Cài đặt → Ứng dụng → My Librarian → Quyền → Máy ảnh. Hoặc nhập tay IP + mã. |

## Kiểm tay trên điện thoại thật
1. Gửi 1 EPUB → tải xong → mở đọc, lật trang, thoát, mở lại đúng chỗ.
2. Đánh dấu 2 trang → thoát → mở lại → thẻ Dấu trang có đủ, chạm nhảy đúng trang.
3. Gửi 1 PDF và 1 EPUB sang "tất cả thiết bị" → điện thoại chỉ nhận EPUB.
4. Nút Quay lại: trong trình đọc mở mục lục → Quay lại đóng mục lục → Quay lại lần nữa về thư viện → lần nữa thoát app.
5. Thanh trạng thái (giờ/pin) và thanh điều hướng phía dưới không che nút nào.

## Đã kiểm gì
- Đã kiểm (2026-11-11, trên Mac): mã Rust biên dịch được cho `aarch64-linux-android` (NDK 27.1, JDK 17 Homebrew); `cargo tauri android init` tạo được dự án.
- **Chưa kiểm:** bước Gradle đóng gói APK chưa được xác nhận chạy xong; chưa cài lên máy ảo hay điện thoại thật; chưa thử mở thư viện, đọc EPUB, nút Quay lại, vùng an toàn (thanh trạng thái/điều hướng), quét QR, ghép và đồng bộ với Windows. `build-android.command` chưa được chạy trọn một lần.
