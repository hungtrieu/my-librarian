# Build bản cài mới

Sửa code xong, mỗi app chỉ cần **một thao tác**. Script tự lo phần còn lại:
- kiểm tra mã nguồn có thay đổi không;
- tự tăng phiên bản (0.1.3 → 0.1.4);
- chạy test, build, đóng gói.

Không có thay đổi thì script không build gì, chỉ báo và chỉ ra bản mới nhất.

## App Windows

Bấm đúp **`build-windows.cmd`** ở thư mục gốc của dự án, rồi đợi 5–10 phút.

- **Xong:** thư mục chứa bộ cài tự mở ra (`dist\<phiên bản>-…\`). Đóng 47 Librarian rồi chạy file `…-setup.exe` trong đó. Bản mới cài đè lên bản cũ, dữ liệu thư viện giữ nguyên.
- **Không có thay đổi:** script báo "không build gì" và mở thư mục bộ cài mới nhất.
- **Hỏng:** script báo đỏ, không tăng phiên bản. Gửi nội dung file `target\ci\latest.log` để xem lỗi.

Muốn build lại dù không đổi gì: chạy `build-windows.cmd -Force` trong cửa sổ dòng lệnh.

## App iPad (trên máy Mac)

> **iPhone:** cùng cách, dùng `build-iphone.command` – hướng dẫn riêng ở [IPHONE.md](IPHONE.md).
>
> **Android:** dùng `build-android.command` (ra file APK, cài qua cáp hoặc chép file) – hướng dẫn riêng ở [ANDROID.md](ANDROID.md).

Chuẩn bị máy Mac **một lần** theo [IPAD.md](IPAD.md#chuẩn-bị-một-lần-trên-mac): Xcode, Apple ID, Rust, Node, ghép iPad.

Mỗi lần cần cập nhật:
1. Mở khoá iPad. iPad phải cùng Wi-Fi với Mac, hoặc cắm cáp.
2. Bấm đúp **`build-ipad.command`** ở thư mục gốc của dự án trên Mac. Hoặc chạy `./build-ipad.command` trong Terminal.

Script tự:
- lấy code mới nhất (`git pull`);
- kiểm tra thay đổi, tăng phiên bản, chạy test;
- build, ký, cài thẳng lên iPad;
- lưu một bản (kèm file `.ipa`) vào `dist/ipad/`.

Lần chạy đầu tiên, script hỏi Team ID (cách lấy có in sẵn trên màn hình) và nhớ luôn cho các lần sau.

Lưu ý:
- **Hạn 7 ngày:** app iPad ký bằng Apple ID miễn phí hết hạn sau 7 ngày. Cứ chạy lại `build-ipad.command`. Nếu code không đổi nhưng bản trên iPad đã quá 6 ngày, script tự build và cài lại, giữ nguyên số phiên bản.
- **Dữ liệu:** sách và vị trí đọc trên iPad giữ nguyên khi cài lại.
- **iPad/iPhone không ở cạnh Mac** (remote vào Mac công ty): `./build-ipad.command --ipa` chỉ build và xuất file `.ipa` đã ký; ở nhà cắm máy vào Windows bấm đúp `install-ios.cmd` (Mac cũ: `./install-ios.command`). Xem [IPAD.md – Build ở Mac khác, cài ở nhà](IPAD.md#build-ở-mac-khác-cài-ở-nhà-windows--mac-cũ).
- **Gặp lỗi:** xem bảng [Nếu gặp lỗi](IPAD.md#nếu-gặp-lỗi).

App iPad lấy code qua git, nên sửa code trên Windows xong thì nhớ **commit + push** trước khi build iPad.
