# Backlog – 47 Librarian

Trạng thái: **Đang làm – M1–M4 xong (lõi, CLI, giao diện thư viện), M5 phần lớn xong (trình đọc trong app), M7 xong phần làm được trên máy (bộ cài, sao lưu, đo hiệu năng) – còn chạy thử trên kho thật** (xem [mục 10](#10-tiến-độ)). Cập nhật theo các quyết định ngày 2026-11-06; tiến độ M5 và M7 ngày 2026-11-07. Phần này **thay thế** mục 3.8, 6 và 8 của [BRD.md](BRD.md) ở những chỗ mâu thuẫn.

Ước lượng thô (1 dev, chưa tính debug trên Windows): **S** ≤ 1 ngày · **M** 2–3 ngày · **L** 4–7 ngày. Ưu tiên: **P0** bắt buộc cho v1 · **P1** nên có · **P2** để sau.

Cột **Trạng thái**: ✅ Xong · 🟡 Lõi xong (chưa có giao diện) / Một phần · ⬜ Chưa làm · ⛔ Bỏ (không làm).

---

## 1. Quyết định đã chốt

| # | Quyết định |
|---|---|
| D1 | Công nghệ: **Rust + web UI (Tauri 2)**. Phát triển, build và kiểm thử **trực tiếp trên máy Windows 11 của bạn** (cập nhật 2026-11-06; trước đây dự kiến viết trên Mac và build ở CI). |
| D2 | **Một thư mục thư viện duy nhất** `…\books` trên ổ cứng gắn ngoài. Không tạo thư mục gốc nào khác. Di chuyển trong cùng ổ = đổi tên tức thì, không copy hàng trăm GB. |
| D3 | **Phân loại theo thư mục thể loại có sẵn** (vd. `Tiểu sử - Hồi ký`). **Không tạo thư mục mới.** Sách không xếp được thì để nguyên và đưa vào danh sách "cần phân loại tay". |
| D4 | Quét ngầm hằng ngày **chỉ lập danh sách** (nghi trùng, trùng chắc chắn, sách mới chưa phân loại, sách mất). **Không tự xoá / di chuyển gì.** |
| D5 | Thông báo: chỉ hiển thị khi mở app (banner + số trên menu). Không toast. |
| D6 | Trình đọc trong app cho **EPUB, MOBI/AZW3, CBZ, CBR, PDF, FB2, DJVU**; giao diện học theo **Apple Books** (iOS). |
| D7 | Cho phép **xoá thẳng sách vào Thùng rác Windows** từ mọi màn hình (một cuốn hoặc nhiều cuốn). |
| D8 | Giữ bản epub + mobi của cùng một sách là hợp lệ; chọn bản giữ theo quy tắc BRD 3.4. |
| D9 | Quy mô: ~300 GB, SSD ngoài, chủ yếu PDF, CBZ, MOBI truyện tranh; mỗi file tối đa ~200 MB. |
| D10 | **Hộp thư đến = `books\Chưa phân loại`.** Sách mới được copy vào đây; chỉ sách trong thư mục này được đề xuất phân loại. Thư mục này **không** là đích phân loại. |
| D11 | **Danh mục chỉ 1 cấp**: mỗi thư mục con cấp 1 của `books` (trừ `Chưa phân loại`) là một thể loại. |
| D12 | **Truyện tranh xếp theo bộ**: mỗi bộ là một thư mục chứa nhiều vol. Thư viện hiển thị theo **bộ (series)**, không phải từng vol rời. |
| D13 | Đánh dấu / tô sáng / ghi chú: **sau v1**. |
| D14 | Máy thử: **Windows 11** (WebView2 có sẵn, không cần cài thêm). |
| D15 | Bỏ spike Thùng rác (S1): file ≤ 200 MB nên coi như luôn vào được Thùng rác. Giữ một bước kiểm tra rẻ ở E5-02 (xem bên dưới). |
| D17 | **CI tại máy, không dùng GitHub Actions** (2026-11-06): `scripts/ci.ps1` chạy format → clippy (`-D warnings`) → test → build release → đóng gói .msi/NSIS, lưu bản build vào `dist/`. Git hook `pre-push` chạy bản nhanh (format + clippy + test) và chặn push nếu hỏng; tuỳ chọn chạy đầy đủ mỗi đêm bằng Task Scheduler. Không dùng GitHub Actions và không build trên macOS/Linux. |

**Hệ quả của D4 so với yêu cầu đầu:** "tự động cho vào thùng rác bản chắc chắn trùng" giờ là *một nút bấm hàng loạt trong app* ("Dọn N bản trùng chắc chắn"), bạn xem danh sách rồi bấm một lần. Nếu sau này muốn bật tự động hoàn toàn thì chỉ là một công tắc cấu hình (mặc định tắt) – xem story E8-05.

---

## 2. Kiến trúc đề xuất

```
47-librarian/
├─ crates/
│  ├─ core/        # thư viện Rust thuần: scan, metadata, DB, dedupe, classify, trash (không phụ thuộc UI)
│  ├─ scanner-cli/ # binary không cửa sổ cho tác vụ ngầm (dùng chung core)
│  └─ app/         # Tauri 2: command/IPC, custom protocol stream file, tray-less
├─ ui/             # web UI (TypeScript + framework nhẹ) + trình đọc
└─ docs/
```
- **Chia lõi riêng (`core`)** để test được bằng `cargo test` không cần giao diện (vẫn chạy được trên Linux/Mac nếu cần), phần phụ thuộc Windows nằm sau trait (`Trasher`, `Scheduler`).
- **DB:** SQLite (rusqlite, bundled) + FTS5, chế độ WAL để app và tác vụ ngầm cùng đọc/ghi.
- **Lưu dữ liệu app** tại `%APPDATA%\47-librarian\` (DB, thumbnail cache, log, config). Không ghi gì vào ổ sách ngoài chính các file sách bạn duyệt di chuyển/xoá.
- **Stream file cho trình đọc** qua custom protocol của Tauri có hỗ trợ HTTP Range; **không** nạp cả file 500 MB vào RAM của webview.
- **Băm:** lọc theo dung lượng → băm nhanh đầu/giữa/cuối → băm đầy đủ chỉ cho ứng viên. Thuật toán (SHA-256 hay BLAKE3) chốt sau spike S5; cả hai đều đủ chắc chắn để coi là "giống từng byte".
- **Tác vụ ngầm:** Task Scheduler chạy `scanner-cli.exe` (subsystem `windows`, không hiện console).

---

## 3. Milestones

| Mốc | Nội dung | Kết quả xem được |
|---|---|---|
| **M0** | Spikes giảm rủi ro (S1–S6) | Biết chắc phương án cho Thùng rác, đọc file lớn, DJVU/CBR |
| **M1** | Lõi: quét + metadata + DB + tìm kiếm | CLI quét kho mẫu, tìm tên không dấu |
| **M2** | Dedupe + duyệt + xoá vào Thùng rác | Danh sách trùng chắc chắn & nghi trùng |
| **M3** | Phân loại theo thư mục có sẵn | Đề xuất di chuyển, xem trước, hoàn tác |
| **M4** | Giao diện thư viện kiểu Apple Books | Lưới bìa sách, bộ sưu tập = thư mục, tìm nhanh |
| **M5** | Trình đọc (EPUB/MOBI/AZW3/FB2 → CBZ/CBR → PDF → DJVU) | Đọc được mọi định dạng |
| **M6** | Quét ngầm + thông báo trong app | Tác vụ chạy khi app đóng |
| **M7** | Đóng gói, thử trên dữ liệu thật, tối ưu | Bộ cài .msi, thử trên bản sao/chế độ chỉ đọc |

Thứ tự này đưa phần "đáng giá nhất và rủi ro nhất" (dedupe an toàn, Thùng rác) lên trước trình đọc.

---

## 4. Backlog chi tiết

### E0 – Nền tảng dự án (M0/M1)
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| E0-01 | Dựng workspace Cargo (`core`, `scanner-cli`, `app`) + `ui` | `cargo test` và `cargo run -p librarian-app` chạy trên Windows | P0 | S | ✅ Xong |
| E0-02 | **CI tại máy Windows** (D17): `scripts/ci.ps1` (format, clippy `-D warnings`, test, build release, đóng gói .msi/NSIS vào `dist/`, log ở `target/ci`), tự chọn toolchain MSVC/GNU; hook `pre-push` chạy `ci.ps1 -Quick`; `scripts/register-nightly.ps1` đăng ký chạy đầy đủ mỗi đêm | Push bị chặn khi test/lint hỏng; mỗi lần chạy đầy đủ có bộ cài trong `dist/` | P1 | M | ✅ Xong |
| E0-06 | ~~GitHub Actions `windows-latest` chạy lại `scripts/ci.ps1` trên máy sạch~~ – bỏ (2026-11-06): chỉ build trên máy Windows này | — | — | — | ⛔ Bỏ |
| E0-03 | Logging có xoay vòng + cấu hình + migration DB | Log ở `%APPDATA%`; nâng schema không mất dữ liệu | P0 | S | ✅ Xong |
| E0-04 | Bộ dữ liệu mẫu tự sinh (epub, mobi, azw3, cbz, cbr, pdf, fb2, djvu, bản trùng, tên Unicode, đường dẫn dài) | Script sinh bộ mẫu dùng chung cho test lõi | P0 | M | ✅ Xong |
| E0-05 | ~~i18n: toàn bộ chuỗi UI qua file ngôn ngữ~~ – bỏ (2026-11-06): app dùng cá nhân, chỉ tiếng Việt | — | — | — | ⛔ Bỏ |

### E1 – Chỉ mục thư viện (M1)
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| E1-01 | Đặt thư mục thư viện `books` (và cho phép thêm thư mục nguồn phụ nếu cần) | Chọn bằng hộp thoại; lưu cấu hình | P0 | S | ✅ Xong |
| E1-02 | Duyệt thư mục đệ quy, hỗ trợ đường dẫn dài (`\\?\`), tên Unicode, bỏ qua `$RECYCLE.BIN`, `System Volume Information`, thư mục ẩn | Quét bộ mẫu đúng số file; không treo vì quyền truy cập | P0 | M | ✅ Xong |
| E1-03 | Quét tăng dần: so dung lượng + mtime, chỉ xử lý file mới/đổi | Quét lần 2 trên 50k file < vài phút (đo ở S5) | P0 | M | ✅ Xong |
| E1-04 | Phát hiện ổ ngoài chưa cắm → bỏ qua quét, **không** đánh dấu sách mất; hiện "Ổ chưa kết nối" | Rút ổ rồi chạy quét: không sách nào bị đánh dấu mất | P0 | S | ✅ Xong |
| E1-05 | Đánh dấu sách mất (file bị xoá/đổi tên ngoài app) và nhận lại nếu chỉ bị đổi tên/chuyển (khớp hash) | Đổi tên file ngoài app → thư viện giữ nguyên tiến độ đọc | P1 | M | ✅ Xong |
| E1-06 | Tiến độ quét + huỷ giữa chừng, tiếp tục được | Huỷ rồi quét lại không hỏng dữ liệu | P0 | S | ✅ Xong |
| E1-07 | ~~Giám sát thay đổi thời gian thực~~ – bỏ (2026-11-06), thay bằng **nút “⟳ Làm mới”** trên thanh công cụ thư viện (và phím F5): quét tăng dần, chỉ đọc file mới/đã đổi | Bấm Làm mới → sách mới thả vào `books` hiện ra | P2 | S | ✅ Xong (thay thế) |

### E2 – Metadata & ảnh bìa (M1/M4)
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| E2-01 | EPUB: title, author, language, subject, ISBN từ OPF | Khớp trên bộ mẫu; file hỏng không làm sập | P0 | M | ✅ Xong |
| E2-02 | MOBI/AZW/AZW3: tên đầy đủ + EXTH (tác giả, ISBN, subject) | Khớp trên mẫu thật của bạn | P0 | M | ✅ Xong |
| E2-03 | CBZ: ComicInfo.xml (Series, Number, Title); mặc định dùng tên file/thư mục chứa | Truyện nhiều tập tách được số tập | P0 | S | ✅ Xong |
| E2-04 | PDF: Info dict (title, author), số trang; thường rỗng/sai → fallback tên file | Không đọc cả file lớn để lấy metadata | P0 | M | 🟡 Một phần |
| E2-05 | FB2 (XML), CBR, DJVU: lấy từ metadata nếu có, còn lại tên file | Có bản ghi đủ để tìm & dedupe | P1 | M | 🟡 Một phần |
| E2-06 | Chuẩn hoá tên: bỏ dấu, bỏ `[ebook]`, `(1)`, `- Copy`, ký hiệu nguồn tải | Bảng test ≥ 50 cặp tên thật | P0 | M | ✅ Xong |
| E2-07 | Ảnh bìa: EPUB (cover OPF), MOBI (record ảnh bìa), CBZ/CBR (ảnh đầu), FB2 (coverpage) | Thumbnail lưu cache, sinh nền, không chặn UI | P0 | L | ✅ Xong |
| E2-08 | Ảnh bìa PDF/DJVU: render trang 1 lười (lazy) khi cuộn tới, cache lại | Cuộn lưới 2.000 sách vẫn mượt | P1 | M | ⬜ |
| E2-09 | Bìa mặc định đẹp (màu + chữ cái) khi không trích được | Không ô trống nào trong lưới | P0 | S | ✅ Xong |

### E3 – Tìm kiếm & duyệt thư viện (M1/M4)
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| E3-01 | FTS5: tìm theo tên sách, tác giả, tên file, không phân biệt dấu/hoa thường, khớp tiền tố | "mat bi" ra "Mắt Biếc"; < 200 ms trên 50k sách | P0 | M | ✅ Xong |
| E3-02 | Gõ tới đâu ra tới đó (debounce) + highlight phần khớp | Không giật khi gõ | P0 | S | ✅ Xong |
| E3-03 | Lọc theo thể loại (thư mục), định dạng, tác giả, trạng thái (chưa đọc / đang đọc / đã đọc) | Kết hợp nhiều bộ lọc | P1 | M | 🟡 Một phần |
| E3-04 | Sắp xếp: mới thêm, tên, tác giả, đọc gần đây, dung lượng | Giữ lựa chọn khi mở lại app | P1 | S | ✅ Xong |
| E3-05 | Danh sách ảo hoá (virtualized) cho hàng chục nghìn sách | Cuộn 50k mục không lag | P0 | M | ✅ Xong |
| E3-06 | Mở thư mục chứa file trong Explorer; sao chép đường dẫn | Dùng `explorer /select` | P1 | S | ✅ Xong |
| E3-07 | **Nhóm truyện theo bộ**: mỗi thư mục bộ = 1 ô trong lưới (bìa = vol đầu, ghi "N vol"); mở ra xem danh sách vol theo số thứ tự tự nhiên (Vol 2 trước Vol 10); tiến độ "đã đọc k/N vol"; nút "Đọc tiếp" nhảy tới vol đang đọc dở | Bộ 100 vol hiện đúng thứ tự, không thành 100 ô rời | P0 | M | ✅ Xong |

### E4 – Phát hiện trùng lặp (M2)
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| E4-01 | Băm nhiều tầng: cùng dung lượng → băm đầu/giữa/cuối → băm đầy đủ; xử lý song song vừa phải; băm lại khi file đổi | 300 GB: chỉ băm đầy đủ nhóm ứng viên; có số liệu đo ở S5 | P0 | L | ✅ Xong |
| E4-02 | Nhóm **trùng chắc chắn** = hash đầy đủ giống nhau; chọn bản giữ theo quy tắc BRD 3.4; hiển thị lý do chọn | Test: cùng nội dung khác tên/khác thư mục được nhóm đúng | P0 | M | ✅ Xong |
| E4-03 | Màn "Trùng chắc chắn": danh sách nhóm, bản giữ (có thể đổi), dung lượng thu hồi được; nút **Dọn tất cả** và dọn từng nhóm | Bấm một lần dọn hết vào Thùng rác; hiện tổng dung lượng giải phóng | P0 | M | ✅ Xong |
| E4-04 | Phát hiện **nghi trùng**: cùng tên chuẩn hoá + tác giả không mâu thuẫn, hoặc cùng ISBN, nội dung khác | Không gộp nhầm "Doraemon 1" với "Doraemon 2" | P0 | L | ✅ Xong |
| E4-05 | Xử lý riêng truyện tranh: so số tập, so số trang/dung lượng gần nhau; tránh báo trùng giữa các tập | Bộ test truyện nhiều tập: 0 báo nhầm | P0 | M | ✅ Xong |
| E4-06 | Màn duyệt nghi trùng: so sánh cạnh nhau (bìa, định dạng, dung lượng, số trang, thư mục, ngày), chọn bản giữ/bỏ, "Không trùng", "Giữ cả hai" | Đã duyệt thì không hỏi lại (ngay cả sau khi quét lại) | P0 | L | ✅ Xong |
| E4-07 | Phím tắt duyệt nhanh (←/→ chuyển nhóm, 1/2 chọn bản giữ, N không trùng) | Duyệt 100 nhóm không cần chuột | P1 | S | ✅ Xong |
| E4-08 | Mở thử hai bản ngay trong trình đọc để so sánh | Mở được từ màn duyệt | P2 | S | ✅ Xong |

### E5 – Xoá vào Thùng rác Windows (M2) — *yêu cầu D7*
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| E5-01 | Trừu tượng `Trasher`; bản Windows dùng Recycle Bin API (crate `trash`/IFileOperation) | Xoá file thử → xuất hiện trong Thùng rác, **Restore** được | P0 | M | ✅ Xong |
| E5-02 | Kiểm tra rẻ: nếu API báo file **không** vào Thùng rác (hoặc trả lỗi) thì dừng cả lô và báo rõ; không bao giờ thử xoá vĩnh viễn thay thế. Không làm tiền kiểm dung lượng/cấu hình ổ vì file ≤ 200 MB | Giả lập lỗi từ `Trasher`: lô dừng, file còn nguyên | P0 | S | ✅ Xong |
| E5-03 | Xoá từ mọi nơi: thư viện, kết quả tìm, màn duyệt, trình đọc; chọn nhiều (Ctrl/Shift) + phím Delete | Có hộp xác nhận ghi số lượng + tổng dung lượng | P0 | M | ✅ Xong |
| E5-04 | Kiểm tra an toàn trước khi xoá bản trùng: bản giữ còn tồn tại, cùng dung lượng/mtime như lúc băm | Bản giữ bị đổi → không xoá, báo lỗi | P0 | S | ✅ Xong |
| E5-05 | Nhật ký thao tác (đã xoá gì, khi nào, bản giữ nào) + màn "Hoạt động"; nút mở Thùng rác | Truy vết được mọi lần xoá | P0 | S | ✅ Xong |
| E5-06 | Xoá file đang mở/bị khoá → báo lỗi dễ hiểu, không crash, các file khác trong lô vẫn xử lý | Test khoá file bằng tiến trình khác | P1 | S | ✅ Xong |

### E6 – Phân loại theo thư mục có sẵn (M3) — *yêu cầu D3*
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| E6-01 | Danh mục = **thư mục con cấp 1** của `books` (vd. `Tiểu sử - Hồi ký`), trừ `Chưa phân loại`. Chỉ 1 cấp; thư mục sâu hơn (vd. thư mục bộ truyện) thuộc danh mục cấp 1 chứa nó | Hiển thị danh mục + số sách mỗi mục | P0 | S | ✅ Xong |
| E6-02 | Hộp thư đến cố định = `books\Chưa phân loại` (đổi được trong Cài đặt). Chỉ sách ở đây được đề xuất; sách đã ở thư mục thể loại không bị đụng tới. Sách không đủ tin cậy thì nằm lại trong `Chưa phân loại` | Thư mục này không bao giờ xuất hiện như một đích | P0 | S | ✅ Xong |
| E6-10 | Truyện tranh mới trong inbox: khớp theo **tên bộ** (bỏ số vol, theo quy tắc D16) với thư mục bộ đã có (vd. `7 Seeds 12.mobi` → `truyen tranh\7 seeds\`; `Doraemon vol 45.cbz` → `...\Doraemon\`). Khớp không phân biệt dấu/hoa thường. Bộ chưa có thì **không tự tạo thư mục**; đưa vào "Cần phân loại tay" kèm gợi ý. (Tạo thư mục bộ mới là hành động riêng, do bạn bấm.) | Test: tên vol viết khác nhau (`Vol.3`, `Tập 03`, `#3`) vẫn khớp đúng bộ | P0 | M | ✅ Xong |
| E6-03 | Bộ phân loại học từ chính kho của bạn: dựng hồ sơ mỗi thư mục từ tiêu đề/tác giả/chủ đề của sách đã nằm trong đó (TF-IDF + trùng tác giả) + từ khoá lấy từ tên thư mục | Có điểm tin cậy 0–100 và lý do ("3 sách cùng tác giả đang ở mục này") | P0 | L | ✅ Xong |
| E6-04 | Ngưỡng: tin cậy cao → đề xuất sẵn tick; trung bình → đề xuất không tick; thấp → vào "Cần phân loại tay" | Đo độ chính xác trên chính kho của bạn bằng cách giấu nhãn thư mục (leave-one-out) và báo tỷ lệ đúng | P0 | M | ✅ Xong |
| E6-05 | Màn xem trước: bảng `sách → thư mục đích`, đổi đích bằng dropdown/kéo-thả, lọc theo độ tin cậy | Không file nào bị di chuyển trước khi bạn bấm Áp dụng | P0 | L | ✅ Xong |
| E6-06 | Áp dụng: đổi tên/di chuyển trong cùng ổ, không ghi đè (trùng tên thêm `(1)`), ghi nhật ký từng file | Di chuyển 1.000 file < vài giây (cùng ổ) | P0 | M | ✅ Xong |
| E6-07 | **Hoàn tác** theo lô (trả đúng chỗ cũ); báo rõ file nào không hoàn tác được | Test: áp dụng rồi hoàn tác → cây thư mục y như ban đầu | P0 | M | ✅ Xong |
| E6-08 | Phân loại tay: kéo sách vào mục bất kỳ ở thư viện (có thể chọn nhiều); mỗi lần làm tay cũng "dạy" bộ phân loại | Sách đã làm tay được tính vào hồ sơ thư mục | P1 | M | 🟡 Một phần |
| E6-09 | Kho đang có sách đặt sai thư mục? Chế độ "Rà soát": liệt kê sách có điểm tin cậy cao ở mục khác với mục hiện tại (chỉ báo, không tự làm gì) | Danh sách gợi ý tách riêng | P2 | M | ⬜ |

### E7 – Trình đọc kiểu Apple Books (M5) — *yêu cầu D6*
Nguyên tắc UI/UX học theo Apple Books: giao diện tối giản, **ẩn thanh điều khiển, chạm giữa để hiện/ẩn**, chữ là trung tâm, hoạt ảnh mượt, nhớ vị trí đọc. Dùng **foliate-js** làm bộ máy EPUB/MOBI/AZW3/FB2/CBZ (và PDF), cùng **pdf.js** và một thư viện DJVU cho trình duyệt; cần kiểm chứng ở spike S3/S4.

| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| E7-01 | Khung trình đọc: toàn màn hình, thanh trên (quay lại thư viện, mục lục, tìm, cỡ chữ/giao diện, đánh dấu) và thanh dưới (thanh trượt vị trí + %, số trang/thời gian còn lại) **tự ẩn**; hiện lại khi chạm giữa / di chuột | Hành vi tương tự Books iOS | P0 | L | ✅ Xong |
| E7-02 | EPUB/MOBI/AZW3/FB2: dạng trang (paged) và dạng cuộn (scroll); lật trang bằng bấm mép trái/phải, phím ←/→/Space/PageUp/Down, vuốt trên màn hình cảm ứng | Mượt trên sách 5.000 trang | P0 | L | ✅ Xong |
| E7-03 | Giao diện đọc: Trắng / Sepia / Xám / Đêm; cỡ chữ; phông (hệ thống + serif/sans); giãn dòng, lề, căn đều; theo chế độ sáng/tối của Windows | Đổi tức thì không mất vị trí | P0 | M | ✅ Xong |
| E7-04 | Mục lục (TOC) dạng ngăn trượt bên; nhảy tới chương | Đúng với sách thật có nhiều cấp | P0 | S | ✅ Xong |
| E7-05 | Tìm trong sách: kết quả có ngữ cảnh, nhảy tới + tô sáng | Tìm tiếng Việt có/không dấu | P1 | M | 🟡 Một phần |
| E7-06 | **Nhớ vị trí đọc** từng sách (CFI/trang) + % hoàn thành; mục "Đang đọc" ở đầu thư viện; mở lại đúng chỗ | Tắt app mở lại vẫn đúng chỗ | P0 | M | ✅ Xong |
| E7-07 | *(Sau v1)* Đánh dấu trang, tô sáng nhiều màu, ghi chú; danh sách ghi chú theo sách | Lưu vào DB, xuất Markdown được | P2 | L | ⬜ |
| E7-15 | Truyện nhiều vol: hết vol thì gợi ý/chuyển sang vol kế tiếp của bộ (E3-07) | Đọc liền mạch cả bộ | P1 | S | ✅ Xong |
| E7-08 | CBZ/CBR: chế độ trang đơn / hai trang, vừa chiều rộng / vừa trang, **đọc phải-sang-trái (manga)**, cuộn dọc liên tục (webtoon), thanh xem trước trang, tải trước trang kế | Truyện 300 trang chuyển trang tức thì | P0 | L | ✅ Xong |
| E7-09 | CBR: giải nén RAR (cần thư viện RAR; nhiều file .cbr thực chất là zip) – kiểm chứng ở S4 | Mở được CBR mẫu của bạn | P0 | M | ✅ Xong |
| E7-10 | PDF: cuộn liên tục / trang đơn, zoom, mục lục, tìm kiếm, chế độ đêm; **stream theo Range**, không nạp cả file | PDF 300–500 MB mở dưới vài giây, RAM không phình | P0 | L | 🟡 Một phần |
| E7-11 | DJVU: hiển thị trang, zoom, điều hướng (thư viện web) – kiểm chứng ở S4 | Mở được DJVU mẫu; nếu không khả thi thì mở bằng app ngoài (ghi rõ) | P1 | L | 🟡 Thay thế (mở app ngoài) |
| E7-12 | Phím tắt Windows + toàn màn hình (F11), chế độ chống mỏi mắt (nhiệt độ màu), thu phóng giao diện | Có bảng phím tắt trong Cài đặt | P1 | S | 🟡 Một phần |
| E7-13 | Hoạt ảnh lật trang (cuộn/mờ) tắt được | Tôn trọng "giảm chuyển động" của Windows | P2 | M | 🟡 Một phần |
| E7-14 | Xoá sách đang đọc ngay trong trình đọc (E5-03) | Xoá xong quay lại thư viện | P0 | S | ✅ Xong |

### E8 – Quét ngầm hằng ngày (M6) — *yêu cầu D4*
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| E8-01 | `scanner-cli.exe`: quét + dedupe + phân loại đề xuất, **chế độ chỉ đọc** với sách (không xoá, không di chuyển) và chỉ ghi vào DB/log | Chạy xong không file sách nào thay đổi (test so sánh cây thư mục) | P0 | M | ✅ Xong |
| E8-02 | Cài/gỡ lịch trong Task Scheduler từ Cài đặt (tên tác vụ `47 Librarian - Quet ngam` – bộ cài NSIS đã gỡ tác vụ này khi gỡ app): giờ chạy, chạy ẩn, ưu tiên thấp, **chạy bù nếu lỡ giờ**, chỉ chạy khi ổ sách đã cắm | Máy tắt lúc 3h → bật lên chạy bù; không cửa sổ nhấp nháy | P0 | M | ⬜ |
| E8-03 | Khoá chống quét chồng (app đang quét thì tác vụ ngầm thoát êm, và ngược lại); DB WAL cho phép app đọc khi quét | Chạy đồng thời không lỗi, không hỏng DB | P0 | M | ✅ Xong |
| E8-04 | Kết quả mỗi lần quét lưu thành "báo cáo": trùng chắc chắn mới, nghi trùng mới, sách mới cần phân loại, sách mất, lỗi đọc file | Lịch sử báo cáo xem lại được | P0 | M | ✅ Xong |
| E8-05 | Công tắc "tự động dọn trùng chắc chắn khi quét ngầm" – **mặc định TẮT**, có cảnh báo; chỉ khi bạn tự bật | Mặc định không bao giờ tự xoá | P2 | S | ✅ Xong |
| E8-06 | Hiển thị trạng thái lịch: lần chạy gần nhất, kết quả, lần kế tiếp | Hiện trong Cài đặt | P1 | S | ⬜ |
| E8-07 | Nếu ổ ngoài chưa cắm: ghi "bỏ qua – ổ chưa kết nối", không báo lỗi giả | Có trong báo cáo | P0 | S | ✅ Xong |

### E9 – Thông báo trong app & cài đặt (M6) — *yêu cầu D5*
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| E9-01 | Khi mở app: banner "Từ lần quét 03:00 hôm nay: 12 nghi trùng, 4 trùng chắc chắn, 9 sách mới chờ phân loại" kèm nút đi thẳng tới màn xử lý; số badge trên menu | Bấm vào mở đúng màn | P0 | S | ✅ Xong |
| E9-02 | Trang Tổng quan: số sách, dung lượng, số định dạng, trùng chờ duyệt, ngày quét gần nhất | Số liệu khớp DB | P1 | S | ⬜ |
| E9-03 | Cài đặt: thư mục thư viện, loại trừ thư mục, giờ quét, ngưỡng phân loại, giao diện sáng/tối | Lưu & áp dụng ngay | P0 | M | ✅ Xong |
| E9-04 | Màn "Hoạt động" (nhật ký xoá/di chuyển/hoàn tác) | Lọc theo loại, theo ngày | P1 | S | 🟡 Một phần |

### E10 – Đóng gói, chất lượng, hiệu năng (M7)
| ID | Story | Tiêu chí nghiệm thu | Ưu tiên | Cỡ | Trạng thái |
|---|---|---|---|---|---|
| E10-01 | Bộ cài .msi/NSIS, đăng ký liên kết file `.epub/.mobi/.azw3/.cbz/...` (tuỳ chọn) | Cài/gỡ sạch, gỡ có hỏi xoá dữ liệu | P0 | M | ✅ Xong |
| E10-02 | **Chạy thử "dry-run" trên kho thật**: chế độ chỉ đọc toàn app (không xoá/di chuyển) để đối chiếu kết quả trước khi bật thao tác thật | Cờ `--read-only` / công tắc trong Cài đặt | P0 | S | ✅ Xong |
| E10-03 | Đo hiệu năng: quét lần đầu, quét lại, dedupe, tìm kiếm, mở PDF/CBZ lớn, RAM | Báo cáo số liệu; đặt ngưỡng chấp nhận | P0 | M | 🟡 Lõi xong (chưa đo app thật) |
| E10-04 | Xử lý file hỏng/không đọc được/DRM: hiển thị "Không đọc được" thay vì lỗi; liệt kê ở một mục riêng | Không crash khi gặp file lạ | P0 | M | ✅ Xong |
| E10-05 | Bộ kiểm thử tích hợp lõi (bản trùng, nghi trùng, di chuyển, hoàn tác, ổ rút ra, tên Unicode) chạy trong `scripts/ci.ps1` | Xanh trên Windows | P0 | M | ✅ Xong |
| E10-06 | Sao lưu DB tự động (giữ 7 bản) | Khôi phục được khi DB hỏng | P1 | S | ✅ Xong |
| E10-07 | Tự cập nhật | Để sau v1 | P2 | M | ⬜ |

---

## 5. Spikes (M0) – làm trước khi cam kết các story liên quan
| ID | Câu hỏi cần trả lời | Ảnh hưởng |
|---|---|---|
| ~~S1~~ | *Đã bỏ theo D15.* Việc đầu tiên khi có bản chạy được trên Windows: xoá thử vài file trên ổ ngoài rồi bấm Restore trong Thùng rác để tự xác nhận. | E5 |
| S2 | Tauri custom protocol + Range có stream được PDF/CBZ 500 MB vào WebView2 mà không tốn RAM không? | E7-08, E7-10 |
| S3 | foliate-js chạy đúng trong WebView2 với EPUB/MOBI/AZW3/FB2/CBZ thật của bạn (kể cả truyện tranh MOBI)? | E7 toàn bộ |
| S4 | Khả thi đọc **CBR (RAR)** và **DJVU** trong app (thư viện nào, giấy phép, hiệu năng)? Phương án dự phòng: chuyển CBR→CBZ nội bộ; DJVU mở app ngoài | E7-09, E7-11 |
| S5 | Tốc độ băm trên SSD ngoài: SHA-256 vs BLAKE3, số luồng tối ưu, hiệu quả lọc theo đầu/giữa/cuối | E4-01, thời gian quét 300 GB |
| S6 | Task Scheduler: chạy ẩn, chạy bù, điều kiện "ổ đã cắm", không cần người dùng đăng nhập? | E8-02 |

---

## 6. Rủi ro chính
| Rủi ro | Giảm thiểu |
|---|---|
| Xoá vĩnh viễn thay vì vào Thùng rác (file lớn, ổ ngoài) | S1 + E5-02: tiền kiểm và từ chối nếu không chắc |
| Báo trùng nhầm truyện nhiều tập | E4-05, bộ test riêng, nghi trùng luôn qua duyệt |
| Phân loại sai | Chỉ đề xuất, xem trước, hoàn tác theo lô, đo độ chính xác trên kho thật (E6-04) |
| Ổ ngoài bị rút giữa chừng | E1-04, E8-07, thao tác từng file có journal |
| PDF/CBZ rất lớn làm treo/RAM | Stream Range, thumbnail lười, quét chỉ đọc metadata |
| Một số định dạng (DJVU/CBR) không đọc được trong webview | S4 + phương án dự phòng mở bằng app ngoài |
| CI chạy trên máy cá nhân (môi trường không "sạch", chỉ chạy khi máy bật) | `ci.ps1` tự kiểm tra toolchain; hook pre-push; bản build/log lưu lại để đối chiếu |

---

## 7. Definition of Done (mọi story)
1. Có kiểm thử tự động cho phần lõi (hoặc kịch bản kiểm tay ghi rõ nếu phụ thuộc Windows).
2. Mọi thao tác phá huỷ (xoá/di chuyển) có xác nhận hoặc xem trước, có nhật ký, và hoàn tác/khôi phục được.
3. Không treo giao diện khi chạy trên kho 300 GB (tác vụ nặng chạy nền, có tiến độ, huỷ được).
4. Chuỗi UI tiếng Việt, hỗ trợ sáng/tối.

## 8. Câu hỏi còn lại
Đã trả lời hết (xem D10–D14 và D16). Không còn câu hỏi chặn việc.

**D16 – Cấu trúc truyện tranh (xác nhận):** `books\truyen tranh\<bộ>\<tên bộ> <số vol>.<đuôi>`, ví dụ `truyen tranh/7 seeds/7 Seeds 4.mobi`. Hệ quả:
- **Tên bộ = tên thư mục cha** (nguồn đáng tin nhất), không suy ra từ tên file.
- **Số vol = phần số cuối còn lại sau khi bỏ tiền tố tên bộ khỏi tên file.** Quan trọng vì tên bộ có thể chứa số (`7 Seeds`): nếu chỉ lấy "số cuối" thì vẫn đúng với `7 Seeds 4`, nhưng nếu lấy "số đầu" sẽ sai thành 7. Với file không có tiền tố tên bộ thì mới dùng các mẫu `Vol 3`, `Tập 03`, `#3`, `v3`.
- Tên thư mục thể loại có thể **không dấu** (`truyen tranh`) trong khi các thư mục khác có dấu → so khớp tên thể loại luôn bỏ dấu, hiển thị đúng tên thật của thư mục.
- Truyện tranh có thể là MOBI (không chỉ CBZ/PDF) → trình đọc MOBI phải xử lý được MOBI/AZW3 dạng ảnh (fixed-layout); đưa vào S3.

## 9. Bước tiếp theo đề xuất
Duyệt → làm **M0 (spikes S2–S6)**. Mình viết mã thử để bạn chạy trên Windows 11 và gửi kết quả; S6 (Task Scheduler) bắt buộc chạy trên Windows. Trong lúc đó có thể bắt đầu **M1** (lõi quét/metadata/tìm kiếm) song song vì không phụ thuộc kết quả spike.

## 10. Tiến độ

Cập nhật 2026-11-06. Trạng thái từng story nằm ở cột **Trạng thái** trong các bảng mục 4.

**M1–M3 (lõi + CLI):** `crates/core` + `librarian.exe` / `librarian-bg.exe` (quét ngầm chỉ đọc).

**M4 (giao diện thư viện kiểu Apple Books):** `crates/app` (Tauri 2) + `ui/` (HTML/CSS/JS thuần, không cần build).
- Lưới bìa sách ảo hoá, ô bộ truyện chồng bìa ("N vol", "Đọc tiếp"), mục Đang đọc / Bộ truyện / hộp thư đến / từng thể loại.
- Tìm gõ-tới-đâu-ra-tới-đó có tô sáng không dấu; lọc định dạng, trạng thái đọc; sắp xếp (nhớ lựa chọn).
- Ảnh bìa EPUB/MOBI/AZW3/CBZ/CBR(zip)/FB2 sinh lười qua giao thức `cover://`, cache JPEG 360×540; PDF/DJVU/TXT dùng bìa tự tạo.
- Màn Trùng chắc chắn (đổi bản giữ, dọn tất cả/từng nhóm), Nghi trùng (so sánh cạnh nhau, phím ←/→ 1–9 N K), Phân loại (xem trước, đổi đích, áp dụng, hoàn tác), Hoạt động (nhật ký, hoàn tác, lịch sử quét, mở Thùng rác), Cài đặt (chọn thư mục, chỉ đọc, ngưỡng, giao diện sáng/tối, đo độ chính xác).
- Chọn nhiều (Ctrl/Shift), phím Delete → xác nhận số lượng + dung lượng → Thùng rác; menu chuột phải: mở bằng app mặc định, hiện trong thư mục, sao chép đường dẫn, đánh dấu đọc, chuyển thư mục.
- Banner kết quả lần quét gần nhất + số trên menu; nút "Quét thư viện" chạy nền có tiến độ và huỷ được.

**Đã bỏ:** E1-07 (giám sát thời gian thực – thay bằng nút “⟳ Làm mới”/F5), E0-05 (i18n – app cá nhân, chỉ tiếng Việt), E0-06 (GitHub Actions – chỉ build trên máy Windows này).

**CI tại máy (E0-02, D17):** `scripts/ci.ps1` + hook `pre-push` (`git config core.hooksPath .githooks`, bật bằng `scripts/install-hooks.ps1`). Không dùng GitHub Actions. Khi đang phát triển chỉ cần `ci.ps1 -Quick` (format + clippy + test, đã chạy xanh); bản đầy đủ (build release + đóng gói .msi/NSIS) chỉ chạy khi cần bộ cài để dùng thật – **chưa chạy thử bước đóng gói** (sẽ kiểm ở M7).

**M5 (trình đọc trong app, 2026-11-07):** `ui/reader.js` + `ui/reader.css`, lõi `crates/core/src/reader.rs`, giao thức `book://` trong `crates/app`.
- Khung kiểu Apple Books: thanh trên/dưới mờ nền, tự ẩn (chạm giữa / đưa chuột lên mép để hiện), chạm mép trái/phải lật trang, thanh trượt vị trí kèm xem trước trang, mục lục, tìm trong sách, bảng "Aa" (Trắng / Sepia / Xám / Đêm / theo Windows, cỡ chữ, phông, giãn dòng, lề, căn đều, lật trang/cuộn), F11 toàn màn hình, Esc về thư viện, Delete xoá vào Thùng rác (E7-14).
- **EPUB/MOBI/AZW/AZW3/FB2:** foliate-js (MIT, vendor ở `ui/vendor/foliate-js`, ghi phiên bản + một sửa nhỏ ở `VERSION.txt`). Tìm có/không dấu ("giot nuoc" ra "giọt nước"), kết quả có ngữ cảnh, nhảy tới và tô sáng.
- **CBZ/CBR:** Rust phục vụ từng trang (`book.localhost/<id>/page/<n>`), không nạp cả file vào webview. CBR dạng RAR giải nén **một lần** vào `%APPDATA%\47-librarian\cache\rar` (giữ 4 cuốn gần nhất) bằng crate `unrar`; bìa và số trang của CBR-RAR cũng đã lấy được. Trang đơn / trang đôi (bìa đứng riêng) / tự động, vừa trang / vừa chiều rộng, phải-sang-trái (manga), cuộn dọc liên tục (webtoon), tải trước 3 trang.
- **PDF:** pdf.js 5.5 (đi kèm foliate-js) qua `PDFDataRangeTransport`: chỉ xin từng khúc 1 MB theo HTTP Range (S2). Cuộn liên tục (vẽ lười, giữ ~10 canvas) hoặc từng trang, zoom (Ctrl +/−/0, Ctrl+lăn chuột), mục lục từ outline, tìm không dấu, chế độ đêm/sepia bằng bộ lọc màu.
- **TXT:** cột chữ cuộn, tự nhận UTF-8 / Windows-1258. **DJVU:** chưa có thư viện web phù hợp → mở bằng ứng dụng mặc định và báo rõ (phương án dự phòng của S4).
- **Nhớ vị trí (E7-06):** CFI (foliate) hoặc `page:<n>`; ≥ 99,5% → "Đã đọc"; sách đã đọc mở lại vẫn giữ "Đã đọc". Đóng trình đọc: lưới thư viện cập nhật % mà giữ nguyên chỗ đang cuộn.
- **Truyện nhiều vol (E7-15):** hết vol hiện thẻ "Đã đọc hết" + nút "Đọc tiếp: vol sau"; menu ⋯ có vol trước/sau.
- **Toolchain GNU:** `crates/core/build.rs` cho `-lpthread` của `unrar_sys` dùng bản tĩnh (tránh trùng ký hiệu với std) → .exe không cần DLL của MinGW. MSVC không bị ảnh hưởng.
- **Xem thử trên trình duyệt:** `node scripts/serve-ui.mjs` phục vụ file mẫu cho trình đọc từ `target/reader-samples` (`sample.epub|pdf|mobi|azw3|fb2|txt`, thư mục `comic/`).

**Kiểm thử:** 49 test tự động (24 unit lõi, 18 tích hợp, 5 tích hợp trình đọc, 1 tích hợp "không đọc được" dùng file RAR mẫu `crates/core/tests/fixtures/rar-comic.cbr`, 1 test HTTP Range ở app) + 1 test Thùng rác Windows thật (`--ignored`). clippy `-D warnings` sạch. Trình đọc đã thử trên trình duyệt với file mẫu: mở đủ EPUB/MOBI/AZW3/FB2/CBZ/PDF/TXT, mục lục, tìm, lưu/khôi phục vị trí, trang đôi, manga, chuyển vol, thẻ hết sách. **Chưa thử trên app Tauri thật (WebView2)** và chưa kiểm bằng mắt các phần cần vẽ: hoạt ảnh lật trang, canvas PDF, cuộn dọc webtoon (khung xem trước bị ẩn khi thử).
**M7 (đóng gói, chất lượng, hiệu năng – 2026-11-07):**
- **Bộ cài (E10-01):** `scripts\ci.ps1` tạo `.msi` (WiX) và `-setup.exe` (NSIS, giao diện tiếng Việt, cài cho riêng tài khoản) trong `dist\`. Bộ cài kèm `librarian.exe`, `librarian-bg.exe` (cấu hình riêng cho lúc đóng gói: `crates/app/tauri.bundle.conf.json`, để build thường không đòi file release). Đăng ký "Mở bằng 47 Librarian" cho `.epub .mobi .azw .azw3 .fb2 .cbz .cbr .pdf .djvu`; bấm đúp file khi app đang mở thì chuyển vào cửa sổ đang chạy (single-instance); file nằm ngoài thư viện vẫn đọc được, không ghi vào DB. Gỡ bằng NSIS: tick "Delete the application data" thì xoá `%APPDATA%\47-librarian` (hook `crates/app/installer-hooks.nsh`), đồng thời gỡ tác vụ quét ngầm nếu có; sách không bị đụng tới. Bộ cài MSI không có lựa chọn xoá dữ liệu. Chuỗi trong MSI phải không dấu (WiX dùng bảng mã 1252).
- **Không đọc được (E10-04):** mục riêng ở thanh bên liệt kê sách có lỗi đọc lúc quét hoặc có DRM, kèm lý do; trình đọc gặp file hỏng thì báo lỗi kèm nút mở bằng app khác.
- **Sao lưu DB (E10-06):** app và tác vụ ngầm sao lưu mỗi ngày (`VACUUM INTO`, giữ 7 bản ở `backups\`). Khi mở mà `library.db` hỏng: cất file hỏng thành `library.db.corrupt-<thời gian>`, khôi phục từ bản sao lưu mới nhất còn dùng được (không có thì tạo DB mới) và báo một lần trong app.
- **Hiệu năng (E10-03):** `librarian bench --books 50000` đo trên kho tổng hợp; kết quả và ngưỡng ở [PERF.md](PERF.md). 50k sách: quét lại 0,6 s, tìm 3,6 ms, lưới sắp theo tên 237 ms (đã tối ưu từ 670 ms). Chưa đo kho thật / RAM khi mở PDF lớn.
- **Thử trên kho thật:** quy trình ở [TRIAL.md](TRIAL.md) (chế độ chỉ đọc → đối chiếu → bật thao tác thật). **Chưa làm – cần bạn chạy trên ổ sách.**

**Kiểm thử (đến M4):** 36 test tự động (18 unit, 18 tích hợp) + 1 test Thùng rác Windows thật (`--ignored`). Giao diện đã thử trên app thật (Windows 11, WebView2) với thư viện mẫu và trên trình duyệt với dữ liệu giả (`node scripts/serve-ui.mjs`).

**Chưa làm:** phần còn lại của M5 (DJVU trong app – E7-11, tô sáng kết quả tìm trong PDF, chế độ chống mỏi mắt + bảng phím tắt – E7-12, công tắc tắt hoạt ảnh riêng – E7-13; đo RAM/thời gian mở PDF 300–500 MB – S2/E10-03; ghi chú/tô sáng để sau v1 – E7-07), đăng ký Task Scheduler (E8-02, E8-06), trang Tổng quan (E9-02), bìa PDF/DJVU (E2-08), kéo-thả phân loại (E6-08), lọc theo tác giả trên giao diện (đã có ở lõi), chạy thử trên kho thật + đo RAM app thật (M7, xem TRIAL.md), spikes S3, S5, S6 (S4 đã có câu trả lời: CBR → `unrar`, DJVU → app ngoài).

**Giới hạn đã biết:** PDF dùng xref stream nén hoặc Info nằm trong object stream thì không lấy được tiêu đề (dùng tên file). CBR dạng RAR chỉ lấy bìa + số trang (không đọc ComicInfo). Build bằng toolchain GNU có cảnh báo linker "multiple non-default manifests" (không gặp với MSVC). Một nhóm nghi trùng có thể chứa cả bản trùng chắc chắn của cùng cuốn; nên dọn ở màn Trùng chắc chắn trước.
