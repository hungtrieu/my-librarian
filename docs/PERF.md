# Hiệu năng (E10-03)

Đo bằng `librarian bench --books 50000` (lõi, bản release). Lệnh sinh một **kho tổng hợp** trong thư mục tạm
(~55% EPUB, 20% PDF, 20% CBZ theo bộ, 5% bản sao), không đụng kho thật, đo rồi xoá (`--keep` để giữ lại,
`--json` để lấy số liệu máy đọc được). Mã thoát 3 nếu có bước vượt ngưỡng.

Ngưỡng tính cho kho 50.000 sách, các bước tỉ lệ theo số sách được quy đổi tuyến tính.

## Kết quả – 2026-11-07, Windows 11, toolchain GNU, ổ trong (SSD)

Kho: 49.596 sách, 61 MB file (file mẫu rất nhỏ), DB 35 MB.

| Bước | Thời gian | Ngưỡng | Ghi chú |
|---|---:|---:|---|
| Quét lần đầu | 66,6 s | — | 745 file/giây (đọc metadata từng file) |
| Quét lại, không có gì đổi | 0,64 s | ≤ 120 s | chỉ so dung lượng + mtime (E1-03) |
| Băm (dung lượng → đầu/giữa/cuối → đầy đủ) | 3,3 s | — | 4.924 file phải băm đầy đủ |
| Trùng chắc chắn | 66 ms | ≤ 5 s | 2.400 nhóm |
| Nghi trùng | 236 ms | ≤ 10 s | 18 nhóm |
| Tìm kiếm – trung bình / chậm nhất | 3,6 / 11,5 ms | ≤ 200 ms | 8 truy vấn × 3 lần (E3-01) |
| Lưới thư viện, gộp bộ, sắp theo tên | 237 ms | ≤ 500 ms | 41.126 ô; trước tối ưu 670 ms |
| Lưới thư viện khi gõ tìm | 42 ms | ≤ 200 ms | 5.832 ô |
| Đề xuất phân loại hộp thư đến | 363 ms | ≤ 15 s | 2.542 sách |

**Tối ưu đã làm:** sắp theo tên/tác giả trước đây bỏ dấu (`fold`) cả hai chuỗi ở *mỗi* lần so sánh
(~41k × log 41k lần). Giờ bỏ dấu một lần cho mỗi mục rồi mới sắp (`natural_cmp_folded`): 670 → 237 ms.

## Chưa đo được bằng lệnh này

- **Kho thật 300 GB trên SSD ngoài:** quét lần đầu bị chặn bởi tốc độ đọc metadata trên ổ ngoài; băm đầy đủ chỉ
  áp dụng cho nhóm cùng dung lượng nên phụ thuộc số bản trùng thật. Ghi số liệu vào mục dưới khi chạy thử
  (xem [TRIAL.md](TRIAL.md)).
- **Mở PDF/CBZ 300–500 MB và RAM của WebView2** (S2): PDF tải từng khúc 1 MB theo Range, truyện tải từng trang,
  nên về thiết kế RAM không phụ thuộc kích thước file; cần đo trên app thật bằng Task Manager.

## Kho thật

_(Điền sau khi chạy thử: số sách, dung lượng, thời gian quét lần đầu / quét lại / băm, số nhóm trùng.)_
