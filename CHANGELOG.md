# Nhật ký thay đổi

Các thay đổi đáng chú ý của My Librarian, mới nhất ở trên.

## 2026-11-18

### Thêm mới

- **Sửa tiêu đề sách:** tiêu đề mới lưu vào DB nên tìm kiếm, gom nhóm và nghi trùng dùng ngay; lần quét sau giữ tiêu đề của bạn. Tuỳ chọn đổi tên file và ghi tiêu đề vào nội dung EPUB/FB2 (ghi file tạm, đọc lại xác minh, bản gốc vào Thùng rác).
- **Nhớ sách đã xoá (DB v6):** sách xoá tay hoặc bỏ khi dọn nghi trùng được ghi lại (kích thước, hash, metadata), kể cả khi đã dọn Thùng rác. Sau mỗi lần quét, bản tải lại trùng nội dung tự vào Thùng rác; bản gần giống hiện trong *Nghi trùng* với thẻ "đã xoá trước đây". Khôi phục từ Thùng rác thì app quên mục đó. Công tắc trong Cài đặt (mặc định bật).
- **Chia nhỏ thư mục `Khác`:** sách "chưa chắc" xếp theo chữ cái đầu của tên file (`Khác/A`…`Khác/Z`, `Khác/0-9`, `Khác/#`), trong mỗi chữ là các ngăn tối đa 500 file (`T-01`, `T-02`…). `Khác` phẳng có sẵn được tự chia.
- **Chặn vĩnh viễn một thể loại / thư mục con:** chuột phải thể loại ở thanh bên → *Chặn…*. Sách trong đó vào Thùng rác, sách tự phân loại vào đó sau này cũng vậy, thư mục không được tạo lại. Bỏ chặn trong Cài đặt.
- **Tag → thể loại:** màn hình liệt kê mọi tag trong thư viện (các biến thể như "Tiểu thuyết" / "tieu-thuyet" gộp chung), gán mỗi tag vào một thư mục thể loại (kể cả thư mục con). Lưu thì xem trước sách nào chuyển đi đâu rồi chuyển thành một lô hoàn tác được; phân loại hộp thư đến dùng bảng này trước các luật có sẵn.
- **Thư viện trên nhiều ổ cứng:** thư viện có thể gồm nhiều thư mục trên các ổ khác nhau, thể loại cùng tên được gộp làm một. Màn **Ổ lưu sách**: dung lượng trống từng ổ, thêm/bớt ổ, và công cụ *Chuyển bớt* chuyển cả thư mục từ ổ đầy sang ổ còn chỗ (chạy nền, có tiến trình, dừng, hoàn tác).
- **Thông tin & tag của sách:** xem metadata (tag, tác giả, bộ, ngôn ngữ, ISBN, số trang, đường dẫn, SHA-256…); bấm tag để mở trong *Tag → thể loại*.
- **Sửa thông tin sách:** *Thông tin & tag…* → *Sửa thông tin…*: tiêu đề, tác giả, tag, bộ + số tập, ngôn ngữ, ISBN; EPUB và FB2 ghi được vào file.
- **Đưa sách ra khỏi `Khác`:** sau khi chuyển tay một cuốn, app tìm sách giống vậy trong `Khác` (theo tác giả – kể cả tên tác giả trong tên file –, bộ / tên sách, tag, cụm tên file chung) và đề nghị chuyển luôn, tuỳ chọn nhớ tag → thư mục cho sách mới.
- **Quản lý thư mục thể loại:** thêm, đổi tên, xoá thư mục thể loại và thư mục con từ menu chuột phải ở thanh bên, nút "+" cạnh *Bộ sưu tập* hoặc Cài đặt. Đổi tên áp dụng cho thư mục thật trên mọi ổ; xoá thì chuyển sách ra trước (hoàn tác được). Thư mục con do app tạo hiện lồng dưới thể loại và nhận kéo thả.
- **Chuyển vùng chọn / cả bộ vào thư mục thể loại:** lệnh *Chuyển … vào thư mục thể loại…* trong thư viện (bộ sách: mọi tập).
- **Ngưỡng tự dọn nghi trùng** đặt được trong Cài đặt (50–100 %, mặc định 85 %).

### Thay đổi

- Không bao giờ tự chuyển sách sang ổ khác: phân loại và chuyển tay chỉ đổi tên trong cùng ổ; chỉ *Chuyển bớt* mới chép giữa các ổ. Bản chép giữa hai ổ ghi ra file tạm ẩn, đọc lại so SHA-256, đổi tên vào chỗ rồi mới xoá bản gốc; lần chép/quét sau tự dọn file tạm dở dang.
- Hoàn tác lô có chép giữa các ổ chạy nền, có tiến trình, dừng và tiếp tục. Màn *Hoạt động* hoàn tác được lô *Tag → thể loại* và *Chuyển bớt*.
- Duyệt nghi trùng nhanh hơn: xoá chạy nối tiếp ở nền, nhóm kế tiếp hiện ngay; không còn toast che nút, chỉ báo lỗi. Nhóm có sách đã xoá trước đây cũng được tự dọn (giữ bản đề xuất).
- Chuyển hàng loạt sách nhanh hơn.
- Các file mã nguồn dùng cùng kiểu xuống dòng với bản gốc (không đổi nội dung).

### Gỡ bỏ

- **Quét ngầm:** bỏ `librarian-bg.exe`, giờ quét và công tắc "tự dọn trùng khi quét ngầm" – app đã quét và phân tích mỗi lần mở hoặc bấm *Quét*.
- Git hook `pre-push` và `scripts/install-hooks.ps1`; kiểm tra chạy qua `scripts/ci.ps1`.

## 2026-11-14

- iPad / iPhone / Android: hiện ảnh bìa thật trong lưới và danh sách, trích ngay trên máy (EPUB, MOBI/AZW, CBZ/CBR, FB2).
- README: thêm ảnh giới thiệu app Windows và điện thoại.

## 2026-11-13

- Gom các định dạng của cùng một tác phẩm; tìm nghi trùng nhanh hơn.
- Đổi tên app thành My Librarian.
- Phát hành công khai lần đầu.
