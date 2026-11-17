# Backlog – My Librarian cho iPad (bản đọc sách)

Trạng thái: **đang làm** (UI iPad `ui/ipad/` đã có trên backend giả: lưới, ô bộ, Đang đọc, tìm kiếm, xoá, dung lượng; lệnh Rust `crates/ipad` đã có (lib_*, reader_info, book://), build được cho aarch64-apple-ios; còn `tauri ios init` + Info.plist chia sẻ file trên Mac) (2026-11-08) – đã có `ipad_lib.rs` (DB iPad: thư viện, nhóm bộ, tìm không dấu, vị trí đọc sống sót khi xoá, dung lượng; phần lõi I1-04/I1-05/I1-07/I1-08, chưa có UI/app) – xong I1-01 (tách feature `desktop`/`reader`, `cargo check --target aarch64-apple-ios` qua) và phần lõi I3: `sync_proto.rs`, `sync.rs` (máy chủ HTTP v1, hàng chờ, ghép token, hash nền, Range; có `cargo test`). UI I3 xong: màn **iPad** (bật/tắt máy chủ, QR + mã 6 số, thiết bị đã ghép, hàng chờ + cảnh báo dung lượng) và menu chuột phải "Gửi sang iPad" (ẩn với CBR/DJVU). I3-06 xong (lọc + nhãn "Có trên iPad"). Còn lại của I3: I3-07/08 (P1). File này độc lập với [BACKLOG.md](BACKLOG.md); khi bắt đầu làm sẽ thêm một dòng trỏ sang từ backlog chính.

Ký hiệu giống backlog chính: cỡ **S** ≤ 1 ngày · **M** 2–3 ngày · **L** 4–7 ngày; ưu tiên **P0** bắt buộc · **P1** nên có · **P2** để sau; trạng thái ✅ ⬜ 🟡 ⛔. ID bắt đầu bằng **I** (iPad) để không trùng với E0–E10.

---

## 0. Câu hỏi đã trả lời (2026-11-07)

| # | Câu hỏi | Trả lời | Hệ quả |
|---|---|---|---|
| Q1 | Có máy Mac không? | **Có.** | Build và cài bằng Xcode trên Mac của bạn. Không cần Mac trên mây, không đụng D17. |
| Q2 | Có tài khoản Apple Developer (99 USD/năm) không? | **Không.** | **Không dùng được TestFlight** (TestFlight bắt buộc tài khoản trả phí). Thay bằng **cài trực tiếp từ Xcode với Apple ID miễn phí** – xem DI11 và I6. |
| Q3 | iPad nào? | **iPad Air 5 (chip M1, RAM 8 GB), 64 GB.** | RAM dư cho PDF/CBZ lớn (SI2 nhẹ đi). **Bộ nhớ 64 GB là giới hạn thật** (còn trống thực tế ~40–50 GB, mỗi vol truyện tới ~200 MB) → quản lý dung lượng thành **P0** (I1-08, I4-04). |
| Q4 | Đồng bộ vị trí đọc / đọc nhiều máy? | **Không cần.** | Bỏ I5-01; vị trí đọc chỉ lưu trên iPad. Giao thức bỏ phần trao đổi vị trí. |
| Q5 | Cùng Wi-Fi nhà với máy Windows? | **Có.** | Giữ nguyên thiết kế đồng bộ qua mạng nội bộ (mục 3). |

### 0.1 Không có tài khoản Developer thì cài thế nào?

Được, với Apple ID miễn phí (Xcode gọi là *Personal Team*):

- **Cài:** cắm cáp iPad vào Mac **một lần** để ghép đôi và bật *Developer Mode* trên iPad (Cài đặt → Quyền riêng tư & Bảo mật → Chế độ nhà phát triển). Từ đó Xcode cài được **qua Wi-Fi**, không cần cáp.
- **Hạn 7 ngày:** chữ ký của tài khoản miễn phí hết hạn sau 7 ngày; hết hạn thì app **không mở được** nhưng **sách và vị trí đọc vẫn còn**. Cài lại đè lên (không xoá app) là dùng tiếp, mất ~1 phút. Mình sẽ làm script `scripts/ipad-install.sh` trên Mac để việc này chỉ là **một lệnh** (I6-02).
- **Giới hạn khác của tài khoản miễn phí:** tối đa 3 app tự ký trên một thiết bị; không có iCloud / thông báo đẩy (kế hoạch này không cần). Camera (quét QR), mạng cục bộ, Bonjour, tải nền đều dùng được.
- **Muốn đỡ việc cài lại hằng tuần (tuỳ chọn):** *AltStore* / *SideStore* tự ký lại app qua Wi-Fi mỗi tuần (AltServer chạy trên Mac của bạn). Hoặc sau này đăng ký Developer 99 USD/năm thì chuyển sang TestFlight (hạn 90 ngày); **code giữ nguyên, chỉ đổi phần ký**.

---

## 1. Quyết định (đã duyệt 2026-11-07)

| # | Quyết định |
|---|---|
| DI1 | **Công nghệ: Tauri 2 bản iOS**, cùng công nghệ với app Windows. Dùng lại gần như nguyên trình đọc hiện có (`ui/reader.js`, foliate-js, pdf.js) và phần Rust đọc sách (`crates/core` – `reader.rs`, `formats/`). Không viết lại bằng Swift. |
| DI2 | **App iPad không có phân loại, không dedupe, không quét.** Chỉ đọc những file đang có trên iPad: thư viện (lưới bìa, nhóm truyện theo bộ, mục "Đang đọc"), trình đọc, đồng bộ, xoá khỏi iPad. |
| DI3 | **Kho chính vẫn là ổ cứng di động cắm vào máy Windows.** iPad chỉ giữ bản sao những bộ đang đọc. Xoá trên iPad **không bao giờ** ảnh hưởng tới ổ sách. |
| DI4 | **Đồng bộ qua Wi-Fi trong nhà, không cần iTunes / Finder / app nào của Apple, không qua đám mây.** App Windows bật một máy chủ nhỏ trong mạng nội bộ; app iPad kết nối tới và tải sách về. Chi tiết ở mục 3. |
| DI5 | **Ghép nối một lần bằng mã QR**: máy tính hiện mã QR, iPad quét bằng camera là xong; những lần sau iPad tự tìm thấy máy tính. Có mã 6 số để gõ tay nếu camera không quét được. |
| DI6 | **Chọn sách gửi từ máy tính**: trên app Windows, chuột phải một cuốn / một bộ / nhiều cuốn → "Gửi sang iPad" → vào hàng chờ. Mở app iPad, bấm "Đồng bộ" là tải hết hàng chờ. (P1: duyệt và tìm cả thư viện máy tính ngay trên iPad, bấm cuốn nào tải cuốn đó.) |
| DI7 | **Đọc được các file sẵn có trên iPad**, không chỉ file gửi từ máy tính: mở file ở bất kỳ đâu trong app Tệp (Trên iPad, iCloud Drive, USB…) **tại chỗ, không cần chép** (app nhớ file và vị trí đọc), hoặc chép vào app; "Mở bằng My Librarian" từ Safari/Mail/AirDrop. |
| DI8 | **Không hỗ trợ CBR trên iPad.** iPad không cần thư viện RAR; app Windows ẩn "Gửi sang iPad" với CBR; mở CBR trên iPad thì báo "Định dạng chưa hỗ trợ". |
| DI9 | **Không hỗ trợ DJVU trên iPad.** Xử lý giống CBR. |

Định dạng đọc được trên iPad: **EPUB, MOBI, AZW/AZW3, FB2, CBZ, PDF, TXT.**
| DI10 | Giao diện tiếng Việt, theo Apple Books iPad: cảm ứng là chính, có hỗ trợ bàn phím/bút. Chạy được cả iPhone nhưng không tối ưu cho iPhone. |
| DI11 | **Phân phối: cài từ Xcode trên Mac bằng Apple ID miễn phí** (Q2), ký lại mỗi 7 ngày bằng một lệnh. Không TestFlight, không App Store. |
| DI12 | **Vị trí đọc chỉ lưu trên iPad**, không đồng bộ ngược về máy tính (Q4). Máy tính chỉ biết cuốn nào đã gửi sang iPad. |

---

## 2. Kiến trúc

### 2.1 Tổng quan

```
 Máy Windows (app hiện có)                         iPad (app mới)
┌───────────────────────────────┐   Wi-Fi nhà   ┌──────────────────────────────┐
│ my-librarian.exe              │  HTTP + token │ My Librarian Reader          │
│  ├─ thư viện, phân loại…      │◄─────────────►│  ├─ thư viện trên iPad       │
│  └─ [MỚI] máy chủ đồng bộ     │               │  ├─ trình đọc (dùng lại)     │
│      cổng 47470, chỉ đọc       │               │  └─ đồng bộ: ghép QR, tải,  │
│      hàng chờ "Gửi sang iPad"  │               │     tiếp tục khi đứt mạng   │
└──────────────┬────────────────┘               └──────────────┬───────────────┘
               │ đọc file                                         │
        ổ cứng di động \books                         Documents/ của app (iPad)
```

### 2.2 Mã nguồn

```
crates/core/       thêm feature: "desktop" (mặc định: quét, trash, dedupe, phân loại…) và "reader" (định dạng, reader.rs)
                   + module mới sync_proto (kiểu dữ liệu API đồng bộ, dùng chung hai bên)
crates/app/        thêm máy chủ đồng bộ + màn "iPad" + menu "Gửi sang iPad"
crates/ipad/       [MỚI] app Tauri 2 cho iOS: DB nhỏ của iPad, giao thức book:// / cover://, client đồng bộ
ui/                tách phần dùng chung (reader.js, reader.css, ui-kit.js, vendor/) để cả hai app cùng dùng
ui-ipad/           [MỚI] giao diện thư viện + đồng bộ cho iPad (HTML/CSS/JS thuần như ui/, không cần bước build)
scripts/ipad-*     [MỚI] chuẩn bị thư mục giao diện; ipad-install.sh (chạy trên Mac) build + ký + cài lên iPad qua Wi-Fi
```

- **Lõi tách feature** để bản iOS không kéo theo `trash`, `walkdir`, `rayon`, Task Scheduler… (một số crate này không build được cho iOS).
- **DB trên iPad**: SQLite (cùng `rusqlite`), bảng `books` rút gọn: id, `remote_id` + `library_id` (sách đến từ máy tính nào, cuốn nào), tiêu đề, tác giả, bộ, số vol, định dạng, dung lượng, sha256, đường dẫn trong app, vị trí đọc, %, trạng thái đọc, thời điểm sửa vị trí.
- **Nhận dạng sách giữa hai máy** = `library_id` (UUID của thư viện Windows, sinh một lần) + `book_id` bên Windows, kèm SHA-256 để kiểm tra file. Sách nhập từ app Tệp trên iPad không có `remote_id` (chỉ ở iPad).
- **File sách** nằm trong `Documents/Books/` của app, **hiện trong app Tệp** của iPad (bật `UIFileSharingEnabled` + `LSSupportsOpeningDocumentsInPlace`) – tiện xem/xoá tay, và là đường dự phòng (app Tệp kết nối được thư mục chia sẻ SMB của Windows).

### 2.3 Dùng lại được gì

| Phần | Dùng lại | Việc cần làm |
|---|---|---|
| Trình đọc EPUB/MOBI/AZW3/FB2 (foliate-js) | Gần như nguyên | Kiểm trên WKWebView (Safari) thay vì WebView2 |
| Trình đọc CBZ (Rust phục vụ từng trang) | Nguyên `reader.rs` | Giao thức `book://` của Tauri trên iOS có dạng URL khác – đã có sẵn chỗ trừu tượng `bookBase()` |
| PDF (pdf.js + Range) | Nguyên | Đo RAM với PDF 300–500 MB trên iPad (SI2); dự phòng PDFKit gốc của iOS |
| Lưu vị trí (CFI / `page:n`) | Nguyên định dạng | → đồng bộ hai chiều không cần chuyển đổi |
| Phục vụ file có HTTP Range (`serve_file`) | Nguyên | Máy chủ đồng bộ dùng lại để iPad tải tiếp khi đứt mạng |
| Ảnh bìa (`covers.rs`, cache JPEG) | Nguyên | iPad tải bìa từ máy tính, không tự trích |
| Giao diện thư viện | Một phần (lưới, ô bộ truyện) | Viết lại cho cảm ứng, bỏ thanh bên phân loại |

### 2.4 Phát triển mà không cần Mac thường xuyên

- Giao diện iPad chạy được trên trình duyệt với backend giả (như `scripts/serve-ui.mjs` hiện nay) → **mở bằng Safari trên chính iPad qua Wi-Fi** để thử cảm ứng, lật trang, hiệu năng foliate/pdf.js trên WebKit thật. Phần lớn lỗi giao diện bắt được ở bước này.
- Máy chủ đồng bộ phía Windows và client đồng bộ (Rust) test bằng `cargo test` trên Windows.
- Chỉ bước **build + ký + cài lên iPad** cần Mac của bạn (Xcode). Code để chung một repo; trên Mac chỉ cần `git pull` rồi chạy script.

---

## 3. Thiết kế đồng bộ

### 3.1 Trải nghiệm

1. **Lần đầu:** máy tính → menu **iPad** → bật "Cho phép iPad kết nối" → hiện mã QR. iPad → **Đồng bộ** → "Ghép với máy tính" → quét QR. Xong. (Windows có thể hỏi cho phép qua tường lửa một lần – chọn mạng **Riêng tư**.)
2. **Chọn sách:** trên máy tính chuột phải cuốn/bộ/nhiều cuốn → **Gửi sang iPad**. Màn iPad trên máy tính hiện hàng chờ, tổng dung lượng, dung lượng còn trống của iPad (lấy từ lần đồng bộ trước), gỡ khỏi hàng chờ được.
3. **Tải:** mở app iPad → bấm **Đồng bộ** (hoặc tự chạy khi mở app nếu thấy máy tính). Thanh tiến độ từng cuốn + tổng; tải 2 file song song; giữ màn hình sáng trong lúc tải; đứt mạng / thoát app thì lần sau **tải tiếp từ chỗ dở** (HTTP Range); xong thì kiểm tra SHA-256 rồi mới hiện sách trong thư viện.
4. **Đọc xong:** vuốt/giữ lâu trên bìa → **Xoá khỏi iPad** (chỉ xoá bản trên iPad, giữ vị trí đọc để lần sau gửi lại vẫn mở đúng chỗ). Bộ truyện: xoá các vol đã đọc, giữ vol đang đọc.
5. Vị trí đọc **không** đồng bộ về máy tính (DI12).

### 3.2 Giao thức (phiên bản `v1`, JSON qua HTTP trong mạng nội bộ)

| Lệnh | Tác dụng |
|---|---|
| `GET /api/v1/hello` | Tên máy, `library_id`, phiên bản giao thức, ổ sách có đang cắm không |
| `POST /api/v1/pair` `{code}` | Đổi mã ghép (trong QR hoặc 6 số) lấy **token** riêng cho iPad đó |
| `GET /api/v1/outbox` | Hàng chờ: id, tiêu đề, tác giả, bộ, vol, định dạng, dung lượng, sha256 |
| `GET /api/v1/books/{id}/file` | Tải file nguyên gốc, hỗ trợ `Range` |
| `GET /api/v1/books/{id}/cover` | Ảnh bìa từ cache có sẵn |
| `POST /api/v1/outbox/ack` | iPad báo đã nhận đủ + đúng hash → rời hàng chờ, máy tính đánh dấu "Có trên iPad" |
| `POST /api/v1/status` | iPad báo dung lượng còn trống + danh sách sách đang có (để máy tính hiện "Có trên iPad" và cảnh báo hàng chờ quá lớn) |
| `GET /api/v1/library?q=&series=` | Tìm / duyệt thư viện máy tính từ iPad (P1) |

### 3.3 An toàn

- Máy chủ **tắt mặc định**, chỉ chạy khi bạn bật trong app Windows (và tự tắt khi đóng app). Chỉ nhận kết nối từ địa chỉ mạng nội bộ.
- Mọi lệnh trừ `hello`/`pair` cần token; mã ghép dùng một lần, hết hạn sau 5 phút; gỡ iPad đã ghép được trong Cài đặt.
- Máy chủ **chỉ đọc** với ổ sách: không xoá, không di chuyển, không ghi gì vào `books`. Chỉ ghi DB (hàng chờ, vị trí đọc). Vẫn đúng khi bật chế độ chỉ đọc (E10-02).
- Chỉ phục vụ file thuộc thư viện (theo `book_id`), không nhận đường dẫn tuỳ ý → không lộ file khác trên máy.
- Dùng HTTP thường (không HTTPS) vì chỉ trong mạng nhà + token; ghi rõ trong rủi ro.

### 3.4 Tìm thấy máy tính

- QR chứa: các địa chỉ IP nội bộ của máy + cổng 47470 + mã ghép + `library_id`. iPad nhớ địa chỉ.
- P1: **Bonjour** (`_mylibrarian._tcp`) – máy tính quảng bá bằng mDNS, iPad tìm bằng `NWBrowser` (plugin Swift nhỏ; iOS cần khai báo `NSBonjourServices` + `NSLocalNetworkUsageDescription`, sẽ hỏi quyền "Mạng cục bộ" một lần). Nhờ vậy máy đổi IP vẫn tìm được, không cần quét QR lại.

---

## 4. Milestones

| Mốc | Nội dung | Kết quả xem được |
|---|---|---|
| **I0** | Spikes SI1, SI2, SI3, SI5 | App "hello" của Tauri cài từ Xcode lên iPad; biết trình đọc chạy ổn trên WebKit với file lớn |
| **I1** | Tách lõi + khung app iPad + thư viện cục bộ + nhập từ app Tệp | Cài lên iPad, nhập EPUB/CBZ từ Tệp, thấy lưới bìa |
| **I2** | Trình đọc trên iPad | Đọc đủ EPUB/MOBI/AZW3/FB2/CBZ/PDF/TXT, nhớ vị trí |
| **I3** | Máy chủ đồng bộ + "Gửi sang iPad" trên Windows | Hàng chờ trên máy tính, QR ghép, tải thử bằng trình duyệt/curl |
| **I4** | Client đồng bộ trên iPad | Quét QR → bấm Đồng bộ → sách hiện trên iPad, tải tiếp khi đứt mạng |
| **I5** | Bonjour, duyệt thư viện máy tính từ iPad | Tìm sách trên máy tính ngay từ iPad, bấm là tải |
| **I6** | Cài / ký lại lên iPad bằng một lệnh trên Mac | `scripts/ipad-install.sh` → app mới trên iPad qua Wi-Fi |

Thứ tự: I0 trước vì rủi ro lớn nhất là **build/ký iOS bằng Tauri** chứ không phải code. I1–I2 và I3 làm song song được (I3 chỉ đụng app Windows).

---

## 5. Backlog chi tiết

### I0 – Spikes (làm trước)
| ID | Câu hỏi cần trả lời | Ảnh hưởng | Cỡ | Trạng thái |
|---|---|---|---|---|
| SI1 | Build app Tauri 2 iOS tối thiểu trên Mac của bạn, ký bằng Apple ID miễn phí (Personal Team), cài lên iPad Air 5 qua cáp rồi qua Wi-Fi. Kiểm: hết hạn 7 ngày rồi cài đè thì dữ liệu trong app còn nguyên | Toàn bộ | S | ⛔ Bỏ (không làm spike riêng; kiểm khi dùng thật) |
| SI2 | foliate-js + pdf.js trong WKWebView trên iPad Air 5: EPUB 5.000 trang, MOBI truyện tranh, CBZ 300 trang, **PDF 300–500 MB** – mượt không, có bị iOS đóng vì RAM không? (thử trước bằng Safari trên iPad) | I2, chọn pdf.js hay PDFKit | M | ⛔ Bỏ (không làm spike riêng; kiểm khi dùng thật) |
| SI3 | Giao thức tự định nghĩa (`book://`) của Tauri trên iOS: stream file có Range, phục vụ từng trang CBZ như bản Windows được không? | I2 | S | ⛔ Bỏ (không làm spike riêng; kiểm khi dùng thật) |
| SI4 | ~~`unrar` trên iOS~~ – bỏ (DI8: không hỗ trợ CBR trên iPad) | — | — | ⛔ Bỏ |
| SI6 | Mở file tại chỗ từ app Tệp (security-scoped bookmark) qua Tauri iOS: chọn file ở "Trên iPad"/iCloud/USB, lần sau mở lại không cần chọn lại | I1-06 | S | ⛔ Bỏ (không làm spike riêng; kiểm khi dùng thật) |
| SI5 | Tốc độ Wi-Fi thực tế máy tính → iPad (mục tiêu ≥ 15 MB/s, tức bộ truyện 2 GB ≈ 2–3 phút); tường lửa Windows; quyền "Mạng cục bộ" của iOS | I3, I4 | S | ⛔ Bỏ (không làm spike riêng; kiểm khi dùng thật) |

### I1 – Nền tảng & thư viện trên iPad
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| I1-01 | Tách `crates/core` thành feature `desktop` / `reader` (`unrar`, `trash`, `walkdir`, `rayon` chỉ nằm trong `desktop`); bản Windows không đổi hành vi | `ci.ps1` vẫn xanh; `cargo check -p librarian-core --no-default-features --features reader --target aarch64-apple-ios` qua (trên Mac) | P0 | M | ✅ |
| I1-02 | Tách phần giao diện dùng chung (`reader.js`, `reader.css`, `ui-kit.js`, `vendor/`) cho hai app; script chép vào thư mục giao diện của từng app lúc build | App Windows chạy như cũ | P0 | S | ⬜ |
| I1-03 | Khung app `crates/ipad` (Tauri 2 iOS): tên, icon, xoay ngang/dọc, Split View, chế độ sáng/tối theo iPad | Cài từ Xcode lên iPad, mở được | P0 | M | 🟡 (đã `tauri ios init`, dự án Xcode tạo được; chưa cài lên iPad) |
| I1-04 | DB SQLite trên iPad + migration; thư mục `Documents/Books`, hiện trong app Tệp | Xoá app cài lại không lỗi; thấy thư mục trong Tệp | P0 | S | ⬜ |
| I1-05 | Thư viện: lưới bìa, mục "Đang đọc" ở đầu, **nhóm truyện theo bộ** (ô bộ, "N vol", "Đọc tiếp" – học E3-07), sắp xếp (mới thêm / đọc gần đây / tên), tìm không dấu | Bộ 50 vol hiện đúng thứ tự Vol 2 trước Vol 10 | P0 | M | ⬜ |
| I1-06 | **Đọc file sẵn có trên iPad** (DI7): chọn file/thư mục trong app Tệp → **mở tại chỗ** (không chép, nhớ quyền truy cập + vị trí đọc) hoặc **chép vào app**; "Mở bằng" từ Safari/Mail/AirDrop; kéo thả từ Tệp khi Split View; tự lấy metadata + bìa bằng lõi Rust. File mở tại chỗ bị xoá/di chuyển thì đánh dấu "Không tìm thấy file" | Chọn 20 file một lần; trùng file thì không nhân đôi; tắt app mở lại vẫn đọc được file tại chỗ | P0 | M | ⬜ |
| I1-07 | **Xoá khỏi iPad** (một cuốn, nhiều cuốn, các vol đã đọc của một bộ) – xác nhận kèm dung lượng giải phóng; giữ lại vị trí đọc | Gửi lại sách đã xoá → mở đúng chỗ cũ | P0 | S | ⬜ |
| I1-08 | Hiện dung lượng app đang dùng + dung lượng iPad còn trống (iPad 64 GB); mục "Đã đọc xong" để xoá hàng loạt; gợi ý xoá khi iPad còn < 5 GB | Hiện đúng so với Cài đặt iPad | P0 | S | ⬜ |

### I2 – Trình đọc trên iPad
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| I2-01 | Đưa trình đọc hiện có (E7) vào app iPad: thanh trên/dưới tự ẩn, chạm giữa hiện/ẩn, chạm mép / vuốt lật trang, mục lục, tìm, bảng "Aa" | Hành vi như Apple Books trên iPad | P0 | M | ⬜ |
| I2-02 | Tối ưu cảm ứng: vùng chạm ≥ 44 pt, vuốt lật trang có hoạt ảnh theo ngón tay, chụm để zoom (PDF/truyện), tránh vùng cử chỉ hệ thống (mép dưới, notch); bàn phím rời: ←/→/Space | Không lật nhầm khi kéo thanh trượt | P0 | M | ⬜ |
| I2-03 | EPUB/MOBI/AZW3/FB2 (foliate-js) trên WebKit | Mở được bộ mẫu `target/reader-samples` + vài sách thật của bạn | P0 | S | ⬜ |
| I2-04 | CBZ: trang đơn/đôi (tự trang đôi khi xoay ngang), manga phải-sang-trái, cuộn dọc webtoon, tải trước trang | Truyện 300 trang lật tức thì | P0 | M | ⬜ |
| I2-05 | ~~CBR~~ – bỏ (DI8). Mở CBR/DJVU thì báo "Định dạng chưa hỗ trợ", không crash | — | — | — | ⛔ Bỏ |
| I2-06 | PDF: pdf.js (hoặc PDFKit nếu SI2 cho thấy pdf.js quá nặng), cuộn liên tục / từng trang, zoom, mục lục, chế độ đêm | PDF 300 MB mở < 3 s, không bị iOS đóng app vì hết RAM | P0 | L | ⬜ |
| I2-07 | TXT | Như bản Windows | P2 | S | ⬜ |
| I2-08 | Nhớ vị trí từng sách (cùng định dạng CFI / `page:n` như Windows); hết vol gợi ý vol kế tiếp (nếu vol đó có trên iPad, không thì gợi ý "Gửi từ máy tính") | Thoát app, mở lại đúng chỗ | P0 | S | ⬜ |
| I2-09 | Chế độ đọc giữ màn hình không tắt (tuỳ chọn), khoá xoay | — | P2 | S | ⬜ |

### I3 – Phía Windows: máy chủ đồng bộ & "Gửi sang iPad"
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| I3-01 | Máy chủ HTTP nhỏ trong app Windows (cổng 47470), bật/tắt trong màn **iPad**, chỉ nhận IP nội bộ, token, giao thức v1 (mục 3.2); dùng lại `serve_file` có Range | `cargo test`: tải có Range, từ chối khi sai token / IP ngoài | P0 | M | ✅ |
| I3-02 | Ghép nối: hiện **QR** (IP + cổng + mã + `library_id`) và mã 6 số; danh sách iPad đã ghép, gỡ được | Quét QR bằng camera iPad đọc ra đúng nội dung | P0 | S | ✅ |
| I3-03 | **Hàng chờ "Gửi sang iPad"**: chuột phải cuốn / ô bộ truyện / nhiều cuốn, và nút trong trình đọc; màn iPad hiện hàng chờ, tổng dung lượng so với dung lượng trống của iPad (từ lần kết nối trước), cảnh báo khi vượt, gỡ khỏi hàng chờ | Gửi cả bộ 100 vol bằng một lần bấm; vượt dung lượng thì cảnh báo trước | P0 | M | ✅ |
| I3-04 | Tính SHA-256 đầy đủ cho sách trong hàng chờ (chạy nền; lưu vào DB, dùng luôn cho dedupe) | Hash đúng, không chặn giao diện | P0 | S | ✅ |
| I3-05 | ~~CBR → CBZ khi gửi~~ – bỏ (DI8). "Gửi sang iPad" ẩn với CBR/DJVU; gửi cả bộ có lẫn CBR/DJVU thì bỏ qua các file đó và báo số lượng | — | — | — | ⛔ Bỏ |
| I3-06 | Đánh dấu trên lưới thư viện Windows: "Có trên iPad" (từ `ack` / `status`) | Lọc được "Có trên iPad" | P1 | S | ✅ |
| I3-07 | Tường lửa / mạng Công cộng. Bộ cài chạy currentUser (không Admin) nên **không** thêm luật lúc cài; thay vào đó màn **iPad** kiểm tra (`ipad_net_check`): card có IP nội bộ đang là mạng Công cộng → cảnh báo "Bị chặn" + nút **Sửa** (một lần UAC, `ipad_net_fix`): chuyển sang Riêng tư + luật `My Librarian sync (TCP 47470)` (TCP 47470, LocalSubnet, hồ sơ Riêng tư). Gỡ cài thì xoá luật (UAC nếu luật có). Lý do: Wi-Fi Công cộng chặn SYN từ điện thoại dù app có luật Cho phép (gặp thật với Android, 2026-11-12) | Wi-Fi Công cộng → thấy cảnh báo; bấm Sửa → điện thoại kết nối được | P1 | S | ✅ |
| I3-08 | Quảng bá Bonjour/mDNS `_mylibrarian._tcp` khi máy chủ đang bật | iPad thấy máy tính mà không cần IP | P1 | S | ⬜ |
| I3-09 | Báo rõ khi ổ sách chưa cắm: iPad thấy "Ổ sách trên máy tính chưa kết nối" thay vì lỗi tải | — | P0 | S | ✅ |

### I4 – Phía iPad: đồng bộ
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| I4-01 | Ghép nối bằng camera quét QR (plugin barcode-scanner của Tauri), hoặc gõ IP + mã 6 số | Ghép được trong < 30 s | P0 | S | 🟡 (code xong, chưa thử trên iPad) |
| I4-02 | Màn **Đồng bộ**: trạng thái máy tính (tìm thấy / không / ổ chưa cắm), hàng chờ, nút Tải tất cả / từng cuốn | — | P0 | S | ✅ (lõi + giao diện; chưa thử trên iPad thật) |
| I4-03 | Tải file: 2 luồng song song, ghi vào file tạm, **tải tiếp bằng Range** khi đứt mạng / thoát app, kiểm SHA-256 rồi mới đưa vào thư viện, `ack` về máy tính; giữ màn hình sáng khi đang tải | Tắt Wi-Fi giữa chừng rồi bật lại: tải tiếp, file cuối cùng đúng hash | P0 | M | ✅ (lõi + giao diện; chưa thử trên iPad thật) |
| I4-04 | Kiểm tra dung lượng trống trước khi tải; thiếu thì báo cần thêm bao nhiêu và gợi ý xoá sách đã đọc | Không tải dở vì đầy bộ nhớ | P0 | S | ✅ (lõi + giao diện; chưa thử trên iPad thật) |
| I4-05 | Tự đồng bộ khi mở app nếu thấy máy tính (tắt được trong Cài đặt) | — | P1 | S | ✅ (lõi + giao diện; chưa thử trên iPad thật) |
| I4-06 | Tải tiếp khi app chạy nền / khoá màn hình (background URLSession – cần plugin Swift) | Khoá iPad, tải vẫn chạy | P2 | M | ⬜ |
| I4-07 | Tìm máy tính bằng Bonjour (`NWBrowser`, plugin Swift nhỏ) | Máy tính đổi IP vẫn kết nối được | P1 | S | ⬜ |

### I5 – Duyệt thư viện máy tính từ iPad
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| I5-01 | ~~Vị trí đọc hai chiều~~ – bỏ (2026-11-07, Q4): không cần đọc nhiều máy | — | — | — | ⛔ Bỏ |
| I5-02 | **Duyệt / tìm thư viện máy tính ngay trên iPad**: tìm không dấu, xem bộ truyện, bấm để tải (không cần thêm vào hàng chờ trên máy tính) | Tìm "mat bi" ra "Mắt Biếc" và tải về được | P1 | M | ⬜ |
| I5-03 | Sách nhập trực tiếp trên iPad (I1-06) không có trên máy tính: chỉ ở iPad, không đồng bộ (ghi rõ trên bìa) | — | P2 | S | ⬜ |

### I6 – Cài đặt lên iPad (Apple ID miễn phí, DI11)
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| I6-01 | Cấu hình ký Personal Team (Bundle ID cố định, vd. `vn.mylibrarian.reader`, không đổi giữa các lần cài để giữ dữ liệu); không đưa thông tin tài khoản vào repo | Cài đè không mất sách | P0 | S | ✅ (ký + cài được lên iPad Air 5, 2026-11-09; chưa thử cài đè giữ dữ liệu) |
| I6-02 | `scripts/ipad-install.sh` trên Mac: build release → ký → cài lên iPad **qua Wi-Fi** (`xcrun devicectl`); dùng cho cả cài bản mới lẫn ký lại mỗi 7 ngày | Một lệnh, iPad không cần cắm cáp | P0 | M | 🟡 (đã cài được qua cáp; tự đăng ký iPad, cài .app thay .ipa; chưa thử qua Wi-Fi) |
| I6-03 | App iPad hiện ngày chữ ký hết hạn ở màn Cài đặt và nhắc trước 1 ngày khi mở app | — | P1 | S | ⬜ |
| I6-04 | Hướng dẫn lần đầu (`docs/IPAD.md`): ghép iPad với Mac, bật Developer Mode, tin cậy nhà phát triển trên iPad, ghép QR với app Windows; tuỳ chọn AltStore/SideStore để tự ký lại | Làm theo được từ đầu đến cuối | P1 | S | 🟡 (viết xong, chưa chạy trên Mac/iPad thật) |
| I6-05 | Kiểm thử: test Rust cho DB iPad + giao thức đồng bộ + máy chủ (chạy trong `ci.ps1` trên Windows); kịch bản kiểm tay trên iPad thật | `ci.ps1` xanh | P0 | M | ⬜ |

**Ước lượng tổng (P0 + P1):** khoảng **4–6 tuần** một người (đã bỏ đồng bộ vị trí đọc và TestFlight). Riêng P0 (đủ dùng: nhập Tệp, đọc, gửi từ máy tính qua QR, tải tiếp khi đứt, quản lý dung lượng): khoảng **3–4 tuần**.

---

## 6. Rủi ro chính
| Rủi ro | Giảm thiểu |
|---|---|
| Chữ ký Apple ID miễn phí hết hạn sau 7 ngày → app không mở được cho tới khi cài lại | I6-02 ký lại bằng một lệnh qua Wi-Fi (dữ liệu giữ nguyên); I6-03 nhắc trước; tuỳ chọn AltStore/SideStore hoặc mua tài khoản Developer sau này |
| Không cẩn thận xoá app trên iPad (thay vì cài đè) → mất sách và vị trí đọc trên iPad | Sách gốc vẫn ở ổ cứng, gửi lại được; hướng dẫn I6-04 ghi rõ "cài đè, đừng xoá app" |
| iPad 64 GB đầy nhanh với truyện tranh | I1-08, I3-03, I4-04: hiện dung lượng ở cả hai đầu, cảnh báo trước khi gửi/tải, xoá hàng loạt sách đã đọc |
| WKWebView giới hạn RAM chặt hơn WebView2 → PDF/CBZ lớn làm app bị đóng | iPad Air 5 có 8 GB RAM nên rủi ro thấp; SI2 vẫn đo sớm; CBZ phục vụ từng trang; PDF dùng Range; dự phòng PDFKit gốc |
| Tauri iOS còn mới, plugin ít (camera, Bonjour, tải nền) | Dùng plugin chính thức (barcode-scanner); phần còn thiếu viết plugin Swift nhỏ; SI1 kiểm sớm |
| Tường lửa Windows chặn; mạng Wi-Fi đặt là "Công cộng"; router bật cách ly máy khách (Wi-Fi khách) | I3-07 thêm luật tường lửa; màn iPad trên máy tính tự kiểm và hướng dẫn chuyển mạng sang Riêng tư; Q5 |
| iOS tạm dừng app khi chuyển sang app khác / khoá màn hình → tải dở | Giữ màn hình sáng + tải tiếp bằng Range (P0); tải nền (I4-06, P2) |
| HTTP không mã hoá trong mạng nhà | Chỉ IP nội bộ, token riêng, máy chủ tắt mặc định, chỉ đọc |
| Thay đổi lõi (tách feature) làm hỏng bản Windows | I1-01 làm trước, `ci.ps1` (test chạy trong mỗi lần build) bắt lỗi |

## 7. Ngoài phạm vi
- Phân loại, dedupe, quét, di chuyển, Thùng rác trên iPad.
- Đồng bộ qua đám mây (iCloud, Google Drive…) hoặc qua Internet ngoài mạng nhà.
- Đưa lên App Store / TestFlight (cần tài khoản Developer – có thể làm sau, code không đổi).
- Đồng bộ vị trí đọc, đọc tiếp trên nhiều máy (Q4).
- CBR, DJVU trên iPad; sách có DRM.
- Tô sáng / ghi chú (theo E7-07, sau v1 cho cả hai bản).
- iPhone được tối ưu riêng; Android.

## 8. Phương án khác đã cân nhắc (không chọn)
| Phương án | Vì sao không chọn |
|---|---|
| **Web app (PWA) mở bằng Safari, "Thêm vào màn hình chính"** – không cần Mac, không tốn 99 USD | iOS giới hạn và có thể dọn bộ nhớ của web app; bộ truyện vài GB không chắc giữ được. Tải từ máy tính qua HTTP trong mạng nhà bị Safari chặn khi web app chạy HTTPS. Đọc file lớn kém ổn định hơn app thật. Giữ làm **phương án dự phòng nếu Q1/Q2 là "không"**. |
| iPad làm máy chủ, máy tính đẩy file sang (kiểu "chia sẻ qua Wi-Fi" của VLC) | iPad phải mở app và sáng màn hình suốt lúc chép; app Windows khó biết iPad đang ở đâu. Máy tính làm máy chủ thì iPad chủ động kéo, tải tiếp dễ hơn. |
| Chỉ dùng app Tệp của iPad kết nối thư mục chia sẻ (SMB) của Windows | Không cần code, nhưng phải bật chia sẻ thư mục trên Windows, chép tay từng file, không có hàng chờ, không đồng bộ vị trí. Vẫn dùng được như đường dự phòng vì thư mục sách của app hiện trong Tệp. |
| Viết app iPad bằng Swift (SwiftUI + PDFKit + thư viện EPUB gốc) | Phải viết lại trình đọc từ đầu, không dùng lại được gì của bản Windows. |
| Đăng ký Apple Developer để dùng TestFlight | Bạn chưa có tài khoản (Q2); cài từ Xcode miễn phí đủ dùng cho một iPad. Chuyển sang sau vẫn được. |

## 9. Bước tiếp theo đề xuất
1. ~~Bạn duyệt DI1–DI12~~ – đã duyệt 2026-11-07.
2. Làm **SI2 ngay trên Windows + Safari iPad**: dựng giao diện đọc chạy trên trình duyệt, mở từ iPad qua Wi-Fi để thử hiệu năng thật.
3. Song song: **SI1** trên Mac của bạn (cần cài Xcode, Rust, `tauri-cli`; mình viết script và hướng dẫn, bạn chạy trên Mac) và **I3** (máy chủ đồng bộ phía Windows – không phụ thuộc iOS).
