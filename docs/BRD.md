# BRD – 47 Librarian (ứng dụng quản lý kho ebook trên Windows)

Trạng thái: **DRAFT**. Chưa có dòng code nào. Các quyết định mới (Tauri, một thư mục `books`, phân loại theo thư mục có sẵn, quét ngầm chỉ lập danh sách, trình đọc kiểu Apple Books) nằm ở [BACKLOG.md](BACKLOG.md) mục 1 và **thay thế** các phần mâu thuẫn trong file này (mục 3.8, 5, 6, 8).

## 1. Bối cảnh & mục tiêu
Người dùng có hàng trăm GB ebook nhiều thể loại, nhiều định dạng, nằm rải rác, trùng lặp nhiều. Cần một ứng dụng Windows để:
1. Đọc được các định dạng phổ biến.
2. Phát hiện và dọn trùng lặp một cách an toàn.
3. Tự sắp xếp file vào thư mục theo thể loại (di chuyển thật).
4. Tìm sách nhanh theo tên.
5. Tự quét trùng hằng ngày, chạy ngầm, không cần mở app.

**Tiêu chí thành công:** kho vài trăm GB / vài chục nghìn file quét lần đầu xong trong thời gian chấp nhận được; quét lại hằng ngày chỉ mất vài phút; tìm kiếm < 200 ms; **không bao giờ mất sách ngoài ý muốn**.

## 2. Phạm vi
### 2.1 Trong phạm vi (v1)
| # | Yêu cầu | Chi tiết |
|---|---|---|
| R1 | Quản lý thư mục nguồn | Thêm/bớt nhiều thư mục gốc (nhiều ổ đĩa, ổ ngoài). Ổ rút ra không bị coi là "sách đã mất". |
| R2 | Định dạng | EPUB, MOBI, AZW/AZW3, CBZ đọc được trong app. PDF, CBR, FB2, DJVU, TXT: quản lý/dedupe/phân loại, mở bằng ứng dụng mặc định của Windows. |
| R3 | Đọc sách | Trình đọc tích hợp cho EPUB/MOBI/AZW3/CBZ. |
| R4 | Trùng chắc chắn | Cùng nội dung từng byte (SHA-256) → tự đưa các bản thừa vào **Thùng rác Windows** (khôi phục được), giữ lại 1 bản tốt nhất. |
| R5 | Nghi trùng | Cùng tên chuẩn hoá (bỏ dấu, bỏ ký hiệu `[ebook]`, `(1)`…) và tác giả không mâu thuẫn, hoặc cùng ISBN, nhưng nội dung khác (vd. epub vs mobi, bản scan khác) → **đưa vào hàng chờ cho người dùng duyệt**: xem cạnh nhau, chọn bản giữ/bản bỏ, hoặc "không trùng". Đã duyệt thì không hỏi lại. |
| R6 | Phân loại & di chuyển | Phân loại mỗi cuốn vào một thư mục thể loại dưới "thư mục thư viện" và **di chuyển file thật**. Có bước xem trước, ghi nhật ký, hoàn tác lô gần nhất. Không bao giờ ghi đè file. |
| R7 | Tìm kiếm | Tìm theo tên sách (cả tác giả, tên file), không phân biệt dấu/hoa thường, gõ tới đâu ra tới đó (tiền tố). |
| R8 | Quét định kỳ ngầm | Windows Task Scheduler chạy hằng ngày (giờ tuỳ chỉnh), không hiện cửa sổ, chạy bù nếu máy tắt đúng giờ. Kết quả: log + thông báo/badge "N nghi trùng chờ duyệt" khi mở app. |

### 2.2 Ngoài phạm vi (v1)
- Sách có DRM (Kindle KFX, Adobe ADEPT) – không đọc/không bẻ khoá.
- Đồng bộ đám mây, đồng bộ thiết bị đọc, chuyển đổi định dạng, chỉnh sửa metadata.
- macOS/Linux (có thể sau).
- Phát hiện trùng theo nội dung "gần giống" (so văn bản/hình ảnh).

## 3. Quy tắc nghiệp vụ & an toàn (quan trọng nhất)
1. **Xoá = vào Thùng rác Windows**, không xoá vĩnh viễn.
2. Trước khi xoá bản thừa phải kiểm tra lại bản giữ lại vẫn tồn tại, cùng dung lượng & thời gian sửa như lúc băm.
3. **Chỉ SHA-256 giống nhau mới được tự xoá.** Mọi trường hợp dựa vào tên/ISBN đều phải qua người duyệt.
4. Chọn bản giữ: ưu tiên bản đã nằm trong thư mục thư viện → tên không có hậu tố `(1)`/`copy` → đường dẫn nông hơn → file cũ hơn.
5. Di chuyển: không ghi đè; trùng tên thì thêm `(1)`; khác ổ đĩa thì copy → kiểm tra dung lượng → mới xoá nguồn; mỗi lần di chuyển ghi vào nhật ký để hoàn tác.
6. Thư mục gốc không truy cập được → bỏ qua khi quét, **không** đánh dấu sách mất.
7. Tuỳ chọn tắt "tự động xoá bản trùng chắc chắn" (khi tắt: chỉ liệt kê chờ duyệt).
8. Phân loại không tự chạy ngầm – chỉ chạy khi người dùng bấm "Áp dụng" sau khi xem trước. (Xem câu hỏi Q3.)

## 4. Yêu cầu phi chức năng
- **Hiệu năng:** chỉ đọc metadata (không đọc cả file); chỉ băm các file có cùng dung lượng với file khác (tiết kiệm hàng trăm GB I/O); quét lại chỉ xử lý file mới/đổi (so size + mtime). Dữ liệu trong SQLite + FTS5.
- **Chạy ngầm:** tiêu thụ ít tài nguyên (ưu tiên thấp), khoá chống quét chồng với app đang mở.
- **Tiếng Việt:** giao diện tiếng Việt; tìm không dấu; xử lý đúng tên file Unicode, đường dẫn dài trên Windows.
- **Độ tin cậy:** file hỏng/lạ không làm sập quá trình quét (fallback dùng tên file).
- **Dữ liệu cục bộ:** mọi thứ nằm trên máy, không gửi gì ra ngoài.

## 5. Phân loại – cách làm đề xuất
Ebook thường không có thể loại đáng tin trong metadata, nên dùng **luật từ khoá có thể chỉnh sửa**: điểm = từ khoá khớp trong metadata subject (×3) + tiêu đề (×2) + tên file/thư mục cha (×1). CBZ/CBR → "Truyện tranh". Không khớp gì → "Chưa phân loại" (không di chuyển nếu đã nằm trong thư viện). Bộ thể loại mặc định ~10 nhóm (Văn học, Khoa học, Công nghệ, Kinh doanh, Tâm lý, Lịch sử – Triết học, Tôn giáo, Y học – Nấu ăn, Ngoại ngữ, Thiếu nhi…), người dùng sửa được từ khoá/thêm nhóm.
Giới hạn cần nói thẳng: độ chính xác bằng luật từ khoá chỉ ở mức khá; sách tên mơ hồ sẽ rơi vào "Chưa phân loại".

## 6. Lựa chọn kỹ thuật (đề xuất, cần chốt – Q1)
| Phương án | Ưu | Nhược |
|---|---|---|
| **A. Electron + Node (đề xuất)** | Phát triển được trên máy Mac hiện tại; thư viện đọc EPUB/MOBI/AZW3/CBZ sẵn (foliate-js); cùng một file .exe chạy GUI và chế độ ngầm `--scan`; SQLite có sẵn trong Node | File cài ~100–150 MB, tốn RAM hơn |
| B. Tauri (Rust + web UI) | Nhẹ, nhanh | Build Windows cần máy/CI Windows; phải tự ghép phần đọc mobi/azw3 |
| C. .NET (WPF/WinUI) | Native Windows, tích hợp Task Scheduler/Recycle Bin tốt | Không build/chạy được trên Mac; phải tự làm trình đọc mobi/azw3/epub |

Kiểm thử Windows thật (Task Scheduler, Thùng rác, đường dẫn dài) cần một máy Windows – trên Mac mình chỉ kiểm thử được logic lõi.

## 7. Kế hoạch triển khai
| Giai đoạn | Nội dung | Kết quả nghiệm thu |
|---|---|---|
| **P0 – Chốt yêu cầu** | Trả lời mục 8 | BRD được duyệt |
| **P1 – Lõi dữ liệu** | Quét thư mục, trích metadata (EPUB/MOBI/AZW3/CBZ), SQLite + FTS, tìm kiếm | Quét bộ mẫu đúng metadata; tìm "mat bi" ra "Mắt Biếc" |
| **P2 – Dedupe** | SHA-256 theo nhóm cùng dung lượng, tự bỏ trùng chắc chắn vào Thùng rác, sinh danh sách nghi trùng | Bộ test: bản y hệt bị bỏ đúng 1, bản gốc giữ; nghi trùng được liệt kê |
| **P3 – Phân loại** | Luật, xem trước, di chuyển, nhật ký, hoàn tác | Di chuyển + hoàn tác trả đúng vị trí; không ghi đè |
| **P4 – Giao diện** | Thư viện + tìm kiếm, màn duyệt trùng, màn phân loại, cài đặt | Dùng được end-to-end trên bộ mẫu |
| **P5 – Trình đọc** | Mở EPUB/MOBI/AZW3/CBZ trong app; các định dạng khác mở bằng app mặc định | Mở được file mẫu mọi định dạng |
| **P6 – Chạy ngầm** | Chế độ `--scan` không cửa sổ, Task Scheduler hằng ngày, khoá chống chạy chồng, log | Tác vụ chạy đúng giờ khi app đóng |
| **P7 – Đóng gói & thử thực tế** | Bộ cài Windows, thử trên bản **copy** của kho thật, đo hiệu năng | Quét kho thật; không mất sách |

Mình đã làm thử một bản nguyên mẫu lõi (P1–P3, kiểm thử tự động qua) để kiểm chứng tính khả thi, nhưng đã xoá theo yêu cầu; sẽ viết lại khi bạn chốt.

## 8. Câu hỏi cần bạn chốt
- **Q1. Công nghệ:** chọn A (Electron, đề xuất), B hay C? Bạn có sẵn máy Windows để thử không?
- **Q2. "Phân loại theo folder":** hiểu đúng là *phân loại theo thể loại rồi dồn vào các folder thể loại trong một thư mục thư viện đích* chứ không phải giữ nguyên cấu trúc folder hiện tại? Nếu bạn đã có sẵn cây folder thể loại của riêng mình, mình sẽ dùng nó làm danh mục thay cho bộ mặc định.
- **Q3. Chạy ngầm hằng ngày** gồm: quét + tự bỏ trùng chắc chắn + cập nhật danh sách nghi trùng. Có **đồng ý để nó tự xoá (vào Thùng rác)** các bản trùng chắc chắn khi bạn không mở app? Còn phân loại/di chuyển file mình đề xuất **không** tự chạy ngầm, cần bạn xem trước. Đồng ý chứ?
- **Q4. Thư mục thư viện đích:** là một thư mục mới trống, và nằm cùng ổ với phần lớn sách (để di chuyển tức thì thay vì copy hàng trăm GB)?
- **Q5. Định dạng:** "mobile, aw3" mình hiểu là **MOBI, AZW3**. Ngoài ra bạn có nhiều PDF/CBR/DJVU không? PDF trong app hay mở bằng phần mềm ngoài là đủ?
- **Q6. Quy mô:** khoảng bao nhiêu file, nằm trên mấy ổ (HDD hay SSD, có ổ ngoài/mạng không)? Ảnh hưởng thời gian quét lần đầu.
- **Q7. Quy tắc giữ bản:** quy tắc ở mục 3.4 ổn chưa? Với cùng một sách nhưng khác định dạng (epub + mobi), bạn muốn giữ cả hai, hay ưu tiên EPUB?
- **Q8. Thông báo:** sau lần quét ngầm, chỉ cần ghi log và hiện số nghi trùng khi mở app, hay muốn có toast của Windows?
