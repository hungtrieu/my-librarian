# 47 Librarian trên iPhone – cài và dùng

App iPhone đọc sách đã gửi từ app Windows qua Wi-Fi nhà. **Không dùng App Store/TestFlight**: bạn tự build trên Mac và cài bằng Apple ID miễn phí. Hướng dẫn này đủ để cài iPhone, không cần đọc tài liệu iPad. (Cài iPad: [IPAD.md](IPAD.md).)

> **Trạng thái (2026-11-10):** đã chạy thử trên iPhone Simulator (iOS 27.2). Chưa cài lên iPhone thật, chưa thử đồng bộ với Windows trên máy thật.

## Khác gì bản iPad
Cùng một app với iPad (cùng mã nguồn, cùng Bundle ID `vn.hungtrieu.librarian47.ipad`). App tự nhận ra đang chạy trên iPhone:
- **Chỉ sách chữ:** EPUB, MOBI/AZW/AZW3, FB2, TXT. Windows không xếp PDF và truyện tranh (CBZ/CBR) vào hàng chờ của iPhone. iPhone cũng tự bỏ qua chúng nếu lỡ có trong hàng chờ.
- **Cầm tay trái:** trong trình đọc, thanh nút nhanh nằm ở **góc dưới trái**: đánh dấu trang, mục lục / dấu trang, tìm trong sách, Aa. Nút quay lại thư viện (‹) và "⋯" ở bên phải. Ở thư viện, nút Đồng bộ và Dung lượng cũng ở bên trái.
- Chạm giữa trang để hiện/ẩn thanh nút; chạm mép trái/phải để lật trang.

## Giới hạn của Apple ID miễn phí
- App **hết hạn sau 7 ngày** → mở không lên. Chạy lại script là cài đè, **dữ liệu trong app giữ nguyên** (sách, vị trí đọc, dấu trang).
- Tối đa 3 app tự ký cùng lúc trên một máy; 10 App ID mới mỗi 7 ngày. iPhone và iPad dùng chung một App ID nên không tốn thêm.
- Mỗi iPhone hết hạn riêng: bản cài trên iPhone được theo dõi riêng với iPad (`dist/iphone-release.json`).

## Chuẩn bị một lần trên Mac
Đã làm cho iPad rồi thì **bỏ qua bước 1–3**, chỉ làm bước 4–6 cho iPhone.

1. Cài **Xcode** từ App Store, mở một lần để chấp nhận điều khoản. `xcode-select -p` phải trỏ vào Xcode.app.
2. Đăng nhập Apple ID: Xcode → Settings → Accounts → `+`. Tạo chứng chỉ ký: chọn tài khoản → **Manage Certificates…** → `+` → **Apple Development**. Team ID là trường `OU` (10 ký tự) của chứng chỉ:
   ```bash
   security find-certificate -c "Apple Development" -p | openssl x509 -noout -subject
   ```
   Ví dụ `… OU=ABCDE12345, …` → Team ID là `ABCDE12345`.
3. Cài công cụ (cần thêm Node.js):
   ```bash
   curl https://sh.rustup.rs -sSf | sh
   ```
   ```bash
   rustup target add aarch64-apple-ios
   ```
   ```bash
   cargo install tauri-cli --version "^2"
   ```
4. **Ghép iPhone với Mac:** cắm cáp, mở khoá iPhone, bấm **Tin cậy** (Trust) và nhập mật mã iPhone nếu được hỏi. Kiểm tra trong Terminal:
   ```bash
   xcrun devicectl list devices
   ```
   Thấy dòng iPhone với `physical` và `connected` là xong. Xcode bản mới (27) **không còn** mục Window → Devices and Simulators, nên không cần tìm nó. Sau lần ghép bằng cáp này, iPhone cùng Wi-Fi với Mac thường hiện `connected` mà không cần cắm cáp; nếu hiện `unavailable` thì mở khoá iPhone hoặc cắm cáp lại.
5. **Bật Developer Mode** trên iPhone: Cài đặt → Quyền riêng tư & Bảo mật → **Chế độ nhà phát triển** (cuối danh sách) → bật → khởi động lại → mở khoá → bấm **Bật**. Không thấy mục này: chạy `./build-iphone.command` một lần (bước đăng ký iPhone làm mục này hiện ra; lần đó báo lỗi cũng không sao), rồi mở lại Cài đặt.
6. Không thấy iPhone trong danh sách ở bước 4: thử cáp khác (cáp truyền dữ liệu, không phải cáp chỉ sạc), cắm thẳng vào Mac, mở khoá iPhone. Finder → thanh bên **Vị trí** cũng hiện iPhone và nút Tin cậy.

## Cài app
Bấm đúp **`build-iphone.command`** ở thư mục gốc dự án, hoặc chạy trong Terminal:
```bash
./build-iphone.command
```
- Lần đầu script hỏi Team ID rồi lưu vào `.ipad-team` (dùng chung với iPad).
- Script tự `git pull`, chạy test, build, ký rồi cài lên iPhone. Lần đầu nó tự **đăng ký iPhone vào Personal Team**, nên cần iPhone mở khoá và đang cắm cáp hoặc ở cùng Wi-Fi.
- Mã nguồn không đổi thì không build lại, trừ khi bản trên iPhone sắp hết 7 ngày. Build lại bằng mọi giá:
  ```bash
  ./build-iphone.command --force
  ```
- **Có nhiều iPhone** ghép với Mac: chọn máy bằng tên hoặc UDID (cột Identifier của `xcrun devicectl list devices`), ví dụ:
  ```bash
  LIBRARIAN_IPAD_DEVICE="iPhone của Na" ./build-iphone.command
  ```
  Mỗi iPhone chạy một lần. Lưu ý: trạng thái hết hạn trong `dist/iphone-release.json` chỉ nhớ lần cài gần nhất. Có từ hai iPhone thì thêm `--force` để chắc chắn cài cho từng máy.

Lần đầu mở app: iPhone → Cài đặt → Cài đặt chung → **VPN & Quản lý thiết bị** → tin cậy Apple ID của bạn.

**Ký lại mỗi 7 ngày:** chạy lại `build-iphone.command` khi iPhone mở khoá và cùng Wi-Fi với Mac.

**iPhone không ở cạnh Mac** (Mac build ở công ty, iPhone ở nhà): trên Mac chạy `./build-iphone.command --ipa` (không cần iPhone), mang file `.ipa` về rồi cắm iPhone vào máy Windows bấm đúp `install-ios.cmd` (hoặc Mac cũ: `./install-ios.command`). iPhone phải được cài trực tiếp từ Mac build một lần trước đó. Chi tiết: [IPAD.md – Build ở Mac khác, cài ở nhà](IPAD.md#build-ở-mac-khác-cài-ở-nhà-windows--mac-cũ).

**Đừng bấm Run trong Xcode** và đừng để Xcode build cùng lúc với script: luôn cài bằng script.

## Ghép với app Windows
1. Windows: 47 Librarian → mục **iPad / iPhone** → **Bật kết nối**. Lần đầu Windows hỏi tường lửa → cho phép mạng riêng tư.
2. Windows: **Hiện mã QR mới** (mỗi máy một mã, mã dùng một lần, hết hạn sau 5 phút).
3. iPhone: mở app → biểu tượng ⇅ **Đồng bộ** (góc dưới trái) → đặt **Tên máy này**, ví dụ "iPhone của Hưng". Tên này hiện trên Windows để phân biệt khi có nhiều máy. Sau đó **Quét mã QR** (cho phép camera) hoặc mở "Nhập tay" và gõ IP + mã 6 số. iOS hỏi quyền **Mạng cục bộ** → Cho phép.
4. Windows: chuột phải sách hoặc bộ sách → **Gửi sang <tên iPhone>** (hoặc "Gửi sang tất cả … thiết bị"). PDF và truyện tranh tự bị bỏ qua cho iPhone; Windows báo số cuốn bị bỏ.
5. iPhone: ⇅ → **Tải**. Mặc định app tự tải khi mở nếu thấy máy tính.

Hai máy phải cùng mạng Wi-Fi (không dùng mạng khách hay mạng cô lập thiết bị).

**Nhiều iPhone / iPad dùng chung thư viện:** mỗi máy có **hàng chờ riêng**: gửi cho iPhone không làm iPad mất sách và ngược lại. Trên Windows, mục iPad / iPhone liệt kê từng máy với:
- hàng chờ của máy đó (bỏ từng cuốn hoặc xoá cả hàng chờ);
- dung lượng còn trống;
- nút **Xem sách đã có trên máy**.

Chuột phải một cuốn có trên thiết bị → **Có trên thiết bị nào?** để xem máy nào đang có cuốn đó. Đổi tên máy bằng nút **Đổi tên**; tên trùng thì Windows tự thêm số ("iPhone 2"). **Gỡ** một máy thì hàng chờ của nó bị xoá, sách đã tải trên máy vẫn còn.

## Đọc và đánh dấu trang
- Mở sách: chạm bìa. Thanh nút ẩn sau vài giây; chạm giữa trang để hiện lại.
- **Đánh dấu trang:** bấm biểu tượng dấu trang (nút đầu tiên, góc dưới trái). Biểu tượng tô cam khi trang đã có dấu; bấm lần nữa để bỏ dấu.
- **Danh sách dấu trang:** nút mục lục (thứ hai) → thẻ **Dấu trang** → chạm để nhảy tới, ✕ để xoá.
- Xoá sách khỏi iPhone rồi gửi lại: vị trí đọc và dấu trang vẫn còn.
- Sách trên iPhone nằm trong app Tệp → **47 Librarian → Books**.

## Nếu gặp lỗi
| Triệu chứng | Thử |
|---|---|
| Script báo "Không thấy iPhone" | Mở khoá iPhone, cắm cáp, bấm Tin cậy; kiểm `xcrun devicectl list devices`. Có nhiều iPhone: đặt `LIBRARIAN_IPAD_DEVICE`. |
| `Your team has no devices` / `No profiles for 'vn.hungtrieu.librarian47.ipad'` | iPhone chưa đăng ký vào team. Script tự làm lần đầu. Vẫn lỗi: cắm cáp, mở khoá iPhone, chạy lại. |
| `errSecInternalComponent` / ".app chưa được ký hợp lệ" | Chạy script trong Terminal trên Mac (không qua SSH). Vẫn lỗi: xem dòng này trong bảng lỗi của [IPAD.md](IPAD.md) (cách sửa giống hệt). |
| Không thấy mục Chế độ nhà phát triển | Ghép iPhone với Mac (bước 4), chạy `./build-iphone.command` một lần, rồi mở lại Cài đặt. |
| Mở app báo không tin cậy / văng ngay | Hết 7 ngày → chạy lại script; hoặc chưa tin cậy Apple ID (VPN & Quản lý thiết bị). |
| "Không thấy máy tính" | Windows đang chạy 47 Librarian và đã bật kết nối? Cùng Wi-Fi? Tường lửa cho phép? IP máy tính đổi → ghép lại. |
| Gửi sách mà iPhone không tải | Sách là PDF/truyện tranh (iPhone không nhận). Hoặc đã gửi nhầm sang máy khác: xem hàng chờ từng máy trên Windows. |
| Camera không bật | Cài đặt → 47 Librarian → Camera. Hoặc nhập tay IP + mã. |

## Kiểm tay trên iPhone thật
1. Gửi 1 EPUB → tải xong → mở đọc, lật trang, thoát, mở lại đúng chỗ.
2. Đánh dấu 2 trang → thoát → mở lại → thẻ Dấu trang có đủ, chạm nhảy đúng trang.
3. Gửi 1 PDF và 1 EPUB sang "tất cả thiết bị" → iPhone chỉ nhận EPUB, iPad nhận cả hai.
4. Cầm tay trái: với tới được mọi nút nhanh bằng ngón cái.
