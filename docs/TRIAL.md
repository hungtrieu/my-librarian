# Thử trên kho thật (M7)

Mục tiêu: chạy 47 Librarian trên ổ sách ~300 GB của bạn mà **không có rủi ro mất file**, đối chiếu kết quả
rồi mới bật thao tác thật. App không bao giờ ghi gì vào ổ sách ngoài các lần xoá/di chuyển bạn bấm duyệt.

## 0. Cài đặt

1. Chạy `scripts\ci.ps1` (bản đầy đủ) hoặc lấy bộ cài mới nhất trong `dist\<phiên bản>-<thời gian>-<commit>\`.
2. Cài bằng `47 Librarian_<phiên bản>_x64-setup.exe` (NSIS, cài cho riêng tài khoản, không cần quyền admin)
   hoặc file `.msi`. Bộ cài kèm `librarian.exe` (CLI) và `librarian-bg.exe` (quét ngầm) trong thư mục cài.
3. Dữ liệu app nằm ở `%APPDATA%\47-librarian` (DB, ảnh bìa, cache, log, `backups\`).
   Gỡ cài đặt bằng NSIS và tick “Delete the application data” sẽ xoá thư mục này; sách không bị đụng tới.

## 1. Chế độ chỉ đọc (bắt buộc ở lần đầu)

- Mở app → **Cài đặt → Chế độ chỉ đọc: bật** → Lưu. Hoặc CLI: `librarian config --read-only true`.
- Ở chế độ này mọi lệnh xoá/di chuyển đều bị từ chối (“Chế độ chỉ đọc đang bật”), kể cả tác vụ ngầm.

## 2. Quét và đối chiếu

| Bước | Làm gì | Đối chiếu |
|---|---|---|
| Chọn thư mục | Cài đặt → Thư mục thư viện → chọn `…\books` | Thanh bên hiện đúng các thư mục thể loại |
| Quét | Bấm “Quét thư viện” (có thể huỷ, lần sau quét tiếp) | Số sách ≈ số file sách trên ổ (Explorer → Properties) |
| Không đọc được | Mục “Không đọc được” ở thanh bên | Mở thử vài file trong đó bằng app khác: có thật sự hỏng/DRM? |
| Trùng chắc chắn | Màn “Trùng chắc chắn” | Chọn ngẫu nhiên 5 nhóm, so dung lượng + mở thử: giống hệt? Bản giữ hợp lý? |
| Nghi trùng | Màn “Nghi trùng” | Có báo nhầm các tập truyện khác nhau (“Doraemon 1” vs “Doraemon 2”) không? |
| Phân loại | Màn “Phân loại” + Cài đặt → “Đo” độ chính xác | Tỉ lệ đúng ở mức “Tin cậy cao” nên ≥ 90% |
| Đọc | Mở 1 cuốn mỗi định dạng (EPUB, MOBI/AZW3, FB2, CBZ, CBR, PDF lớn) | Mở được, nhớ vị trí khi mở lại; PDF 300 MB mở dưới vài giây |
| Rút ổ | Rút ổ ngoài rồi bấm Làm mới (F5) | “bỏ qua – ổ chưa kết nối”, **không** sách nào bị đánh dấu mất |

Ghi lại số liệu thật (thời gian quét lần đầu/quét lại, số nhóm trùng) vào `docs/PERF.md` mục “Kho thật”.

## 3. Bật thao tác thật

1. Tắt chế độ chỉ đọc.
2. Dọn **một** nhóm trùng chắc chắn → mở Thùng rác Windows → **Restore** thử file vừa xoá (xác nhận D15/S1).
3. Áp dụng phân loại cho vài cuốn → màn “Hoạt động” → **Hoàn tác** → kiểm tra file về đúng chỗ cũ.
4. Sau đó mới dọn hàng loạt.

## 4. Khi có sự cố

- Log: `%APPDATA%\47-librarian\logs\`.
- DB được sao lưu mỗi ngày (giữ 7 bản) ở `%APPDATA%\47-librarian\backups\`. Nếu `library.db` hỏng, app tự cất
  file hỏng thành `library.db.corrupt-<thời gian>` và khôi phục từ bản sao lưu mới nhất (có thông báo khi mở app).
- Nhật ký mọi lần xoá/di chuyển: màn “Hoạt động” hoặc `librarian activity`.
