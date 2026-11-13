# My Librarian trên iPad – cài và dùng

App iPad đọc sách đã đồng bộ từ app Windows qua Wi-Fi nhà. **Không dùng App Store/TestFlight**: bạn tự build trên Mac và cài từ Xcode bằng Apple ID miễn phí.

> **Trạng thái (2026-11-09):** đã build, ký và cài được lên iPad Air 5 (iPadOS 27.0.1, Xcode 27.2). Chưa thử đồng bộ với Windows trên máy thật.

## Giới hạn của Apple ID miễn phí
- App **hết hạn sau 7 ngày** → mở không lên. Cài đè lại (chạy lại script) là xong, **dữ liệu trong app giữ nguyên** miễn Bundle ID không đổi (`vn.hungtrieu.mylibrarian.ipad`, đừng sửa).
- Tối đa 3 app tự ký cùng lúc trên một thiết bị; 10 App ID mới mỗi 7 ngày.
- Muốn khỏi ký lại: AltStore/SideStore (tự ký lại qua Wi-Fi) – tuỳ chọn, ngoài phạm vi repo này.

## Chuẩn bị một lần (trên Mac)
1. Cài **Xcode** từ App Store, mở một lần để chấp nhận điều khoản. `xcode-select -p` phải trỏ vào Xcode.app.
2. Đăng nhập Apple ID: Xcode → Settings → Accounts (Apple Accounts) → `+`.
   - Xcode mới **không hiện Team ID** ở đó. Tạo chứng chỉ ký: chọn tài khoản → **Manage Certificates…** → `+` → **Apple Development**.
   - Team ID là trường `OU` (10 ký tự) của chứng chỉ:
     ```bash
     security find-certificate -c "Apple Development" -p | openssl x509 -noout -subject
     ```
     Ví dụ `… OU=ABCDE12345, …` → Team ID là `ABCDE12345` (không phải `UID=` hay mã trong ngoặc ở `CN=`).
   - Thường **không cần làm gì thêm**: chạy script trong Terminal của chính Mac (không qua SSH) thì `codesign` dùng được khoá ký. Chỉ khi script báo `errSecInternalComponent` mới chạy lệnh dưới (hỏi mật khẩu đăng nhập Mac – mật khẩu mở máy, không phải mật khẩu riêng):
     ```bash
     security set-key-partition-list -S apple-tool:,apple:,codesign: -s ~/Library/Keychains/login.keychain-db
     ```
3. Cài công cụ:
   ```bash
   curl https://sh.rustup.rs -sSf | sh
   rustup target add aarch64-apple-ios
   cargo install tauri-cli --version "^2"
   ```
   Cần thêm Node.js (để ghép giao diện).
4. **Ghép iPad với Mac**: cắm cáp, mở khoá iPad, bấm "Tin cậy". Kiểm bằng `xcrun devicectl list devices` (bước 6). Xcode 27 không còn mục Window → Devices and Simulators; sau lần ghép bằng cáp, iPad cùng Wi-Fi thường tự hiện `connected`.
5. **Bật Developer Mode** trên iPad: Cài đặt → Quyền riêng tư & Bảo mật → Chế độ nhà phát triển → bật → khởi động lại.
6. Kiểm: `xcrun devicectl list devices` thấy dòng iPad, `physical`, `connected`. Trạng thái `connected (no DDI)` không chặn việc cài.

## Cài app
Bấm đúp **`build-ipad.command`** ở thư mục gốc dự án (hoặc `./build-ipad.command` trong Terminal). Lần đầu nó hỏi Team ID rồi lưu vào `.ipad-team`. Script tự `git pull`, chỉ build khi code đổi (hoặc khi bản trên iPad sắp hết hạn 7 ngày), tự tăng phiên bản, chạy test rồi gọi `scripts/ipad-install.sh`. Xem [BUILD.md](BUILD.md).

Có nhiều iPad cắm/ghép với Mac thì đặt thêm `LIBRARIAN_IPAD_DEVICE=<tên hoặc UDID>` trước khi chạy (UDID ở cột Identifier của `xcrun devicectl list devices`). Nhập giá trị thật, đừng gõ nguyên `<…>` – zsh sẽ báo `parse error`.

`scripts/ipad-install.sh` (build-ipad.command gọi): tìm iPad → ghép giao diện → `cargo tauri ios init` (lần đầu) → thêm khoá Info.plist (thư mục Tệp, camera, mạng nội bộ) → **lần đầu: đăng ký iPad vào Personal Team** (xcodebuild với `-allowProvisioningDeviceRegistration`; nếu không, Xcode báo *"Your team has no devices"*) → `cargo tauri ios build` → cài thẳng `My Librarian.app` đã ký trong DerivedData bằng `xcrun devicectl device install app`. Script không dùng `.ipa`: bước archive/export của Tauri hay lỗi `errSecInternalComponent` dù `.app` đã ký xong.

**Đừng bấm Run trong Xcode**: bước "Build Rust Code" gọi `cargo tauri ios xcode-script`, cần tauri CLI chạy song song (và Xcode mở từ Dock không thấy `~/.cargo/bin` → `cargo: command not found`). Luôn build bằng script. Cũng đừng để Xcode build cùng lúc với script (lỗi `database is locked`).

Lần đầu mở app: iPad → Cài đặt → Cài đặt chung → **VPN & Quản lý thiết bị** → tin cậy Apple ID của bạn.

**Ký lại mỗi 7 ngày:** chạy lại `build-ipad.command` (iPad cùng Wi-Fi, mở khoá). Code không đổi nhưng bản trên iPad đã quá 6 ngày thì script tự build + cài lại. iPad không ở cạnh Mac: xem mục [Build ở Mac khác, cài ở nhà](#build-ở-mac-khác-cài-ở-nhà-windows--mac-cũ).

## Build ở Mac khác, cài ở nhà (Windows / Mac cũ)
Dùng khi Mac build ở chỗ khác (vd. Mac mini công ty, remote vào), còn iPad/iPhone ở nhà. Mac build ký sẵn app cho các máy **đã đăng ký** vào Personal Team, đóng thành file `.ipa`; ở nhà chỉ cần chép lên máy qua cáp, **không ký lại**, dữ liệu trong app giữ nguyên.

**Điều kiện:** mỗi iPad/iPhone phải được cài trực tiếp từ Mac build **một lần** (cắm cáp, chạy `build-ipad.command` / `build-iphone.command` bình thường) để đăng ký vào team. Máy mới chưa đăng ký thì `.ipa` không cài được lên máy đó.

**Trên Mac build (remote):**
```bash
./build-ipad.command --ipa
```
(`./build-iphone.command --ipa` cũng được – cùng một app.) Không cần iPad cắm/ghép. Script build, ký, lưu `My-Librarian-<phiên bản>.ipa` vào `dist/ipad/<phiên bản>-…/` và in **hạn ký** (ngày app ngừng mở được). Một file `.ipa` cài được cho mọi máy đã đăng ký (cả iPad lẫn iPhone).
- Muốn file tự về nhà: ghi đường dẫn một thư mục đồng bộ (Google Drive, OneDrive, iCloud Drive…) vào file `.ipa-share` ở thư mục gốc dự án trên Mac build, ví dụ `~/Library/CloudStorage/GoogleDrive-…/My Drive/librarian-ipa`. Script tự chép `.ipa` sang đó. (Hoặc tải về bằng scp / công cụ remote.)
- Code không đổi thì không build lại, chỉ in (và chép) file `.ipa` của bản gần nhất. Build lại bằng mọi giá: `./build-ipad.command --ipa --force`.

**Ở nhà – Windows 11:**
1. Một lần: cài app **Apple Devices** từ Microsoft Store (hoặc iTunes) để có driver; cài Node.js (để tải công cụ cài go-ios lần đầu).
2. Cắm iPad/iPhone, mở khoá, bấm **Tin cậy**.
3. Bấm đúp **`install-ios.cmd`** ở thư mục gốc dự án – tự lấy file `My-Librarian-*.ipa` mới nhất trong thư mục ghi ở `.ipa-share` (trên máy Windows, ví dụ `G:\My Drive\librarian-ipa`), `dist\ipad`, `dist\iphone` hoặc **Downloads**. Hoặc kéo-thả file `.ipa` lên `install-ios.cmd`. Nhiều máy cắm cùng lúc: `install-ios.cmd -Device <UDID>`.

**Ở nhà – Mac cũ** (vd. MacBook Pro 2016, không chạy được Xcode mới): cần Node.js, không cần Xcode. Cắm máy, rồi:
```bash
./install-ios.command            # lấy .ipa mới nhất (như trên, thêm ~/Downloads)
./install-ios.command ~/Downloads/My-Librarian-0.1.10.ipa
```

Script cài kiểm trước: bản `.ipa` còn hạn ký không, máy đang cắm có trong chữ ký không. Công cụ cài là [go-ios](https://github.com/danielpaulus/go-ios) (MIT), tự tải một lần vào `%LOCALAPPDATA%\my-librarian\go-ios` (Windows) / `~/Library/Caches/my-librarian/go-ios` (Mac).

**Hạn 7 ngày tính từ lúc Mac build tạo provisioning profile**, không phải lúc cài. Xcode dùng lại profile cũ đến khi hết hạn, nên bản build hôm nay có thể chỉ còn vài ngày – xem dòng "hạn ký" script in ra (cả lúc build lẫn lúc cài). Hết hạn thì build lại `--ipa` trên Mac (Xcode tự cấp profile mới) rồi cài lại.

> **Cài lên iPhone?** Xem [IPHONE.md](IPHONE.md) – hướng dẫn riêng, đầy đủ (cùng app, bản iPhone chỉ đọc sách chữ).

## Đánh dấu trang (iPad và iPhone)
Trong trình đọc bấm biểu tượng dấu trang (🔖) để đánh dấu / bỏ đánh dấu trang đang đọc; biểu tượng tô màu khi trang đã có dấu. Danh sách: nút mục lục → thẻ **Dấu trang**. Dấu trang giữ lại khi xoá sách khỏi máy rồi gửi lại (như vị trí đọc).

## Sách không đáng đọc (Windows, iPad, iPhone, Android)
- **Windows:** chuột phải sách hoặc bộ → **Đánh dấu không đáng đọc**. Sách biến khỏi thư viện (cả tìm kiếm, Đang đọc) và nằm ở mục **Không đáng đọc** trên sidebar; chuột phải ở đó → **Đáng đọc – đưa lại vào thư viện**. Toast có nút Hoàn tác.
- **iPad / iPhone / Android:** nhấn giữ sách hoặc bộ → **Không đáng đọc**. Thư viện có thêm tab **Không đáng đọc (n)**; nhấn giữ trong tab đó → **Đáng đọc** để đưa lại.
- **Đồng bộ tự động** khi thiết bị cùng Wi-Fi với máy Windows đang mở My Librarian: lúc mở app, lúc app quay lại màn hình (tối đa 30 giây một lần), ngay sau khi đánh dấu trên thiết bị, và mỗi lần tải sách. Máy tính không bật thì để lần sau. Hai bên đổi khác nhau thì **lần đổi sau cùng thắng**.
- Cờ giữ lại khi xoá sách khỏi thiết bị rồi gửi lại. Chỉ áp dụng cho sách đang có trên thiết bị.
- Cần cập nhật **cả** app Windows và app thiết bị; bản Windows cũ thì thiết bị chỉ đánh dấu được cục bộ.

## Ghép với app Windows
1. Windows: My Librarian → mục **iPad / iPhone** → **Bật kết nối**. Lần đầu Windows hỏi tường lửa → cho phép mạng riêng tư. Cổng mặc định 47470. Nếu Wi-Fi nhà đang là mạng **Công cộng**, màn này hiện cảnh báo **Bị chặn** – bấm **Sửa** (Windows hỏi quyền quản trị) để chuyển sang Riêng tư và mở cổng; không sửa thì thiết bị báo "connection timed out".
2. Windows: **Hiện mã QR mới**.
3. iPad: biểu tượng ⇅ → đặt **Tên máy này** (vd. "iPad của Na" – tên hiện trên Windows để phân biệt khi có nhiều máy) → **Quét mã QR** (cho phép camera) hoặc nhập tay IP + mã 6 số. iOS hỏi quyền **Mạng cục bộ** → Cho phép.
4. Windows: chuột phải sách / bộ → **Gửi sang <tên iPad>** (hoặc "Gửi sang tất cả … thiết bị"). Trên iPad mở ⇅ → Tải. Mặc định app tự tải khi mở nếu thấy máy tính.

Mã QR dùng một lần, hết hạn sau 5 phút – mỗi máy ghép bằng một mã mới. Hai máy phải cùng mạng Wi-Fi (không phải mạng khách/cô lập client).

**Nhiều iPad / iPhone dùng chung thư viện:** mỗi máy có **hàng chờ riêng**. Mục iPad / iPhone trên Windows liệt kê từng máy: hàng chờ của máy đó (bỏ từng cuốn hoặc cả hàng chờ), dung lượng trống, và **Xem sách đã có trên máy**. Chuột phải một cuốn có trên thiết bị → **Có trên thiết bị nào?**. Đổi tên máy bằng nút **Đổi tên**; tên trùng thì Windows tự thêm số ("iPad 2"). Gỡ một máy thì hàng chờ của nó bị xoá, sách đã tải trên máy vẫn còn.

## Đọc sách có sẵn trên iPad
Thư mục **My Librarian → Books** hiện trong app Tệp. (Mở file tại chỗ từ Tệp: chưa làm, I1-06.)

## Nếu gặp lỗi
| Triệu chứng | Thử |
|---|---|
| "Không thấy máy tính" | Windows đang chạy My Librarian và đã bật kết nối (mục iPad / iPhone)? Cùng Wi-Fi? Tường lửa Windows cho phép app? IP đổi → ghép lại. |
| iPad không có trong `devicectl list devices` | Mở khoá iPad, cắm cáp (cáp truyền dữ liệu) thử một lần. |
| `Your team has no devices` / `No profiles for 'vn.hungtrieu.mylibrarian.ipad'` | iPad chưa đăng ký vào team. Script tự làm lần đầu; nếu vẫn lỗi: cắm cáp, mở khoá iPad, kiểm `LIBRARIAN_IPAD_TEAM`, chạy lại. |
| `errSecInternalComponent` / ".app chưa được ký hợp lệ" | Chạy script trong Terminal trên Mac (không qua SSH/công cụ khác). Vẫn lỗi: chạy `security set-key-partition-list …` ở mục Chuẩn bị (bước 2), hoặc khi macOS hỏi "codesign muốn dùng khoá" nhập mật khẩu mở máy và bấm **Always Allow**. Không nhớ mật khẩu mở máy: chạy `./scripts/ipad-keychain-setup.sh` một lần – tạo keychain `ipad-signing` với mật khẩu mới, hướng dẫn tạo chứng chỉ Apple Development mới trong Xcode (thu hồi cái cũ), cho codesign dùng khoá không cần hỏi; `ipad-install.sh` tự mở khoá keychain này mỗi lần build. |
| `cargo: command not found` khi build trong Xcode | Đừng build trong Xcode, dùng script. |
| `database is locked` | Đang có build khác (Xcode hoặc lần chạy trước). Dừng nó rồi chạy lại. |
| `install-ios`: "Chưa có driver Apple" / "Không thấy iPad/iPhone" | Cài app Apple Devices (Microsoft Store), mở một lần; cắm cáp truyền dữ liệu, mở khoá, bấm Tin cậy. |
| `install-ios`: "Máy này chưa có trong chữ ký" | Máy chưa đăng ký vào team: cắm máy vào Mac build, chạy `build-ipad.command` (không `--ipa`) một lần, rồi build lại `.ipa`. |
| `install-ios`: "đã hết hạn ký" | Build lại trên Mac: `./build-ipad.command --ipa --force`. |
| Mở app báo không tin cậy / văng ngay | Hết 7 ngày → chạy lại script; hoặc chưa tin cậy nhà phát triển (xem trên). |
| Camera không bật | Cài đặt → My Librarian → Camera. Hoặc nhập tay IP + mã. |

## Kiểm tay trên iPad thật (I6-05)
1. Gửi 1 EPUB nhỏ → tải xong → mở đọc, lật trang, thoát, mở lại đúng chỗ.
2. Gửi 1 bộ CBZ ≥ 1 GB → tắt Wi-Fi iPad giữa chừng, bật lại, Tải lại → tải tiếp (không từ đầu), file đúng.
3. Xoá một vol đã đọc → gửi lại → mở đúng chỗ cũ.
4. Để iPad gần đầy → màn Đồng bộ báo thiếu chỗ, không tải dở.
5. Ghi tốc độ MB/s thực tế (mục tiêu ≥ 15 MB/s) vào SI5.
