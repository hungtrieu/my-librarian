# My Librarian

![My Librarian trên Windows và điện thoại](docs/images/intro.png)

Ứng dụng Windows quản lý kho ebook: quét, tìm nhanh không dấu, phát hiện trùng lặp an toàn, phân loại vào các thư mục thể loại có sẵn, quét ngầm hằng ngày. Yêu cầu: [docs/BRD.md](docs/BRD.md) · Backlog & tiến độ: [docs/BACKLOG.md](docs/BACKLOG.md).

Hiện có **lõi Rust (`crates/core`), CLI (`crates/scanner-cli`) và app Tauri 2 (`crates/app` + `ui/`)** với giao diện thư viện kiểu Apple Books (M1–M4) và **trình đọc trong app** (M5): EPUB, MOBI/AZW/AZW3, FB2, CBZ, CBR (zip và RAR), PDF, TXT. DJVU vẫn mở bằng ứng dụng mặc định của Windows. Bộ cài .msi / NSIS do `scripts\ci.ps1` tạo trong `dist\`; trước khi dùng trên kho thật hãy làm theo [docs/TRIAL.md](docs/TRIAL.md). Số đo hiệu năng: [docs/PERF.md](docs/PERF.md) (`librarian bench`). Chi tiết: [docs/BACKLOG.md](docs/BACKLOG.md#10-tiến-độ).

## Cấu trúc

```
crates/core/         thư viện lõi: scan, metadata, SQLite+FTS5, băm, dedupe, phân loại, di chuyển/hoàn tác, thùng rác
crates/scanner-cli/  librarian.exe (CLI) và librarian-bg.exe (tác vụ ngầm, không cửa sổ)
crates/app/          my-librarian.exe – app Tauri 2 (IPC + giao thức cover:// cho ảnh bìa)
crates/ipad/         app iPad + iPhone (Tauri 2, iOS) và điện thoại Android – đọc sách đồng bộ từ app Windows qua Wi-Fi
ui/                  giao diện HTML/CSS/JS thuần (không cần bước build); mock.js = backend giả để xem trên trình duyệt
ui/ipad/             giao diện thư viện iPad (dùng chung trình đọc trong ui/)
scripts/serve-ui.mjs xem trước giao diện trên trình duyệt với dữ liệu giả
docs/                BRD, backlog, BUILD.md (build lại Windows + iPad), IPAD.md (cài app iPad), IPHONE.md (cài app iPhone)
```

**Build bản cài mới sau khi sửa code:** Windows – bấm đúp `build-windows.cmd`; iPad – bấm đúp `build-ipad.command`, iPhone – `build-iphone.command`, Android – `build-android.command` trên Mac. Chỉ build khi code đổi, tự tăng phiên bản và chạy test. Chi tiết: [docs/BUILD.md](docs/BUILD.md).

Thư viện có thể trải trên nhiều ổ – xem [Thư viện trên nhiều ổ cứng](#thư-viện-trên-nhiều-ổ-cứng). Dữ liệu app nằm ở `%APPDATA%\my-librarian\` (`library.db`, `config.json`, `logs\`, `backups\`); đổi bằng `--home` hoặc biến `LIBRARIAN_HOME`. Ngoài các file sách bạn duyệt di chuyển/xoá, app không ghi gì vào ổ sách.

## Build

Cần Rust ≥ 1.89.

- **MSVC (khuyến nghị):** cài *Visual Studio 2022 Build Tools* với workload "Desktop development with C++", rồi:
  ```bash
  cargo test --workspace
  cargo build --release
  ```
- **GNU (không cần quyền admin):** cài MinGW-w64 (vd. `winget install BrechtSanders.WinLibs.POSIX.UCRT`), thêm thư mục `mingw64\bin` vào `PATH`, rồi:
  ```bash
  rustup toolchain install stable-x86_64-pc-windows-gnu
  cargo +stable-x86_64-pc-windows-gnu test --workspace
  ```

Test thật với Thùng rác Windows (xoá một file tạm rồi Restore) được đánh dấu `#[ignore]`:

```bash
cargo test -p librarian-core --test windows_recycle_bin -- --ignored
```

## CI tại máy

Không dùng dịch vụ ngoài; mọi thứ chạy trên máy Windows này (backlog D17, E0-02).

```bash
powershell -ExecutionPolicy Bypass -File scripts/ci.ps1
```

Chạy lần lượt: format → clippy (`-D warnings`) → test → build release → đóng gói .msi/NSIS (nếu đã cài `tauri-cli`). Bản build lưu ở `dist/<phiên bản>-<thời gian>-<commit>/` (giữ 10 bản gần nhất), log ở `target/ci/` (`latest.log` là lần gần nhất). Tự chọn toolchain MSVC nếu có Build Tools, ngược lại GNU. Tham số: `-Quick` (chỉ format + clippy + test), `-NoBundle`, `-Toolchain msvc|gnu`.

- **Chặn push khi hỏng:** `scripts/install-hooks.ps1` bật hook `.githooks/pre-push` (chạy `ci.ps1 -Quick`). Bỏ qua một lần: `git push --no-verify`.
- **Chạy mỗi đêm:** `scripts/register-nightly.ps1 [-At 02:00]` đăng ký Task Scheduler (chạy bù nếu máy tắt đúng giờ); gỡ bằng `-Remove`.
- **Đóng gói** cần `tauri-cli`: `cargo install tauri-cli --version "^2" --locked`.

## Thư viện trên nhiều ổ cứng

Sách nhiều quá một ổ: Cài đặt → **+ Thêm thư mục ở ổ khác…** (vd. thư mục chính `D:\books` + `G:\sach`). Mỗi thư mục có cùng cấu trúc (thể loại ở cấp 1, hộp thư đến riêng); app quét cả hai, và thể loại cùng tên trên các ổ (kể cả khác dấu, vd. `Văn học` / `Van hoc`) hiện làm một.

- **Không bao giờ tự chép sách qua ổ khác.** Phân loại, chuyển tay, ánh xạ tag đều dùng thư mục tương ứng trên chính ổ đang chứa sách (tạo nếu chưa có) – chỉ là đổi tên: tức thì, không chép dữ liệu, không tốn thêm chỗ, không thể làm hỏng file. Ổ còn trống dưới mức **Giữ trống mỗi ổ** (mặc định 20 GB) chỉ được cảnh báo; hãy tải sách mới vào hộp thư đến ở ổ còn chỗ.
- **Ổ lưu sách** (thanh bên): dung lượng từng ổ, ổ sắp đầy được đánh dấu. Chép qua ổ chỉ xảy ra khi bạn bấm **Chuyển bớt sách sang ổ khác**: chuyển nguyên thư mục (bộ / tác giả), thư mục lớn trước, sang ổ còn chỗ – chạy nền, dừng được, hoàn tác được (cũng chạy nền).
- **Chép an toàn:** mỗi file chép vào file tạm ẩn cạnh đích, đọc lại so SHA-256 với bản gốc, đổi tên thành tên thật rồi mới xoá bản gốc. Mất điện / app bị tắt giữa chừng: bản gốc còn nguyên, file tạm được dọn ở lần sau (ghi chú `copy-inflight.txt` trong thư mục dữ liệu app).
- Bản trùng giữa hai ổ hiện ở **Trùng chắc chắn** – dọn đi là lấy lại chỗ. Ổ rời chưa cắm thì bị bỏ qua khi quét, sách trên đó không bị coi là mất.
- Lõi: `crates/core/src/volumes.rs`; cấu hình `extra_libraries`, `min_free_gb` trong `config.json`.

## Gán tag → thể loại

Màn **Tag → thể loại** (thanh bên, mục Dọn dẹp) liệt kê mọi tag (metadata `subject`) của sách trong thư viện kèm số sách và nơi chúng đang nằm. Chọn thư mục thể loại (kể cả thư mục con, vd. `Tiểu thuyết › Nước ngoài`) cho từng tag, hoặc tick nhiều tag rồi gán một lần; `Tiểu thuyết`, `tieu-thuyet`, `TIỂU THUYẾT` là cùng một tag, và nhiều tag (`novel`, `fiction`…) gán được vào cùng thư mục. Bấm Lưu → xem trước sách nào chuyển đi đâu → chuyển (lô `Tag → thể loại` trong Hoạt động, hoàn tác được).

- Đổi đích của một tag thì sách mang tag đó chuyển theo, kể cả từ thư mục con lên thư mục cha và ngược lại. Thư mục bộ / tác giả bên trong (`…/Harry Potter/`) đi cùng sách; thư mục cũ rỗng được dọn.
- Sách mang nhiều tag: các đích cùng nhánh thì chọn đích sâu nhất; khác nhánh thì không chuyển, chỉ báo xung đột.
- Sách mới: tự phân loại hộp thư đến dùng ánh xạ trước các luật có sẵn; sách mới chép thẳng vào thư mục khác trong thư viện cũng được xếp theo ánh xạ sau lần quét.
- Ánh xạ lưu trong `config.json` (`tag_map`). Lõi: `crates/core/src/tagmap.rs`.

## Gộp thư mục thể loại sau khi giải nén

`scripts/merge-categories.py` (Python 3, không cần thư viện ngoài) gộp các thư mục thể loại trùng/lồng nhau ở cấp 1 của kho: `Hồi Ký, Tuỳ Bút` + `Hồi Ký - Tùy Bút` → một thư mục; `Bút Ký - Hồi Ký - Thơ` khi đã có `Bút Ký - Hồi Ký` → hồi ký về thư mục có sẵn, phần còn lại thành `Thơ`. Mỗi tác phẩm (mọi định dạng cùng tên) được xếp theo tên sách, metadata EPUB/MOBI, từ khoá trong phần đầu sách và hình dạng dòng (thơ). Bỏ qua `Calibre Library` và hộp thư đến.

```bash
python -I scripts/merge-categories.py plan "D:\books"
python -I scripts/merge-categories.py apply "D:\books" merge-plan.csv
python -I scripts/merge-categories.py undo merge-undo-<thời gian>.jsonl
```

`plan` không đụng vào ổ sách, chỉ ghi `merge-plan.csv` (mở bằng Excel, sửa cột `den_thu_muc` hoặc xoá `x` để bỏ qua). Thêm `--also "D:\books\dtv books"` để gộp cả thư mục thể loại giải nén bên trong thư mục nguồn. Xong thì quét lại trong app.

## Chạy app

```bash
cargo run -p librarian-app
```

Lần đầu app hỏi thư mục `books` rồi tự quét. Bộ cài Windows: bấm đúp `build-windows.cmd`; app iPad: bấm đúp `build-ipad.command` trên Mac. Xem [docs/BUILD.md](docs/BUILD.md).

Xem trước giao diện không cần Rust (dữ liệu giả):

```bash
node scripts/serve-ui.mjs
```

Giao diện Windows ở http://localhost:5173/, giao diện iPad ở http://localhost:5173/ipad/index.html (thêm `?iphone=1` để xem bản iPhone, `?android=1` để xem bản Android).

## Dùng thử CLI

```bash
librarian gen-sample D:\thu-nghiem
librarian init D:\thu-nghiem\books
librarian scan
librarian search mat bi
librarian analyze
librarian dupes
librarian suspects
librarian classify
librarian evaluate
```

Lệnh thay đổi file chỉ **xem trước**; thêm `--yes` để thực hiện:

| Lệnh | Tác dụng |
|---|---|
| `dupes --yes [--keep HASH=ID]` | Dọn mọi bản trùng chắc chắn (SHA-256) vào Thùng rác |
| `trash <id...> --yes` | Xoá sách đã chọn vào Thùng rác |
| `classify --yes [--include-medium]` | Di chuyển sách từ `Chưa phân loại` vào thư mục đề xuất |
| `triage [--yes]` | Tự phân loại cả hộp thư đến (kể cả thư mục kiểu Calibre): đọc `metadata.opf`, làm phẳng, tạo thư mục thể loại mới, chưa chắc → `Khác` (chia thư mục con theo chữ cái đầu của tên file: `Khác/A/`…`Khác/Z/`, sách bắt đầu bằng số vào `Khác/0-9/`, còn lại `Khác/#/`; trong mỗi chữ là các ngăn tối đa 500 file `T-01`, `T-02`…, để Explorer/Finder mở nhanh; `Khác` phẳng có sẵn cũng được tự chia), rồi đưa mọi file không phải sách (kể cả âm thanh, video) và thư mục thừa vào Thùng rác. Mặc định chạy tự động mỗi lần quét; tắt bằng `config --auto-triage false` |
| `move <id> <thư mục> --yes` | Phân loại tay |
| `undo <lô>` | Hoàn tác một lô di chuyển |
| `decide <id...> --not-dup / --keep-both` | Duyệt nhóm nghi trùng (ghi nhớ kể cả sau khi quét lại) |
| `activity [lô]`, `report`, `series`, `categories`, `backup`, `config` | Xem nhật ký, báo cáo, cấu hình |

Thêm `--read-only` vào bất kỳ lệnh nào để chắc chắn không file nào bị thay đổi (dùng khi thử trên kho thật, E10-02). Thêm `--json` để lấy kết quả dạng JSON.

**Thử trên kho thật:** chạy `librarian config --read-only true` trước, rồi `scan`, `analyze`, `dupes`, `suspects`, `classify`, `evaluate` để đối chiếu kết quả. Chỉ tắt read-only khi kết quả đã ổn.

## Quét ngầm

`librarian-bg.exe [--home <dir>]` chạy quét + băm + tìm trùng + đề xuất phân loại, chỉ ghi DB/log, rồi thoát (mã 0 = xong, 2 = app đang quét, 1 = lỗi). Việc đăng ký lịch trong Task Scheduler (E8-02) chưa làm. Tạm thời có thể đăng ký tay:

```bash
schtasks /Create /SC DAILY /ST 03:00 /TN "My Librarian scan" /TR "\"C:\path\librarian-bg.exe\""
```

## Giấy phép

[MIT](LICENSE). Thư viện kèm theo trong `ui/vendor/foliate-js` giữ giấy phép riêng: foliate-js (MIT), pdf.js (Apache-2.0).
