//! Sửa tiêu đề trong app Windows: đổi tên file, ghi vào nội dung (EPUB/FB2), giữ qua lần quét sau,
//! và nghi trùng dùng tiêu đề mới.

use librarian_core::dedupe::{self, Decision};
use librarian_core::formats::{self, Format};
use librarian_core::retitle::{self, RetitleOptions};
use librarian_core::scan::{self, ScanOptions};
use librarian_core::trash::Trasher;
use librarian_core::{db, testdata, Library};
use std::cell::Cell;
use std::path::{Path, PathBuf};

struct DirTrash {
    dir: PathBuf,
    n: Cell<usize>,
}
impl Trasher for DirTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        self.n.set(self.n.get() + 1);
        let dst = self.dir.join(format!("{}-{}", self.n.get(), path.file_name().unwrap().to_string_lossy()));
        std::fs::rename(path, dst).map_err(|e| e.to_string())
    }
}

fn epub(path: &Path, title: &str, body: &str) {
    testdata::write_epub(
        path,
        &testdata::EpubSpec {
            title, authors: &["Nguyễn Nhật Ánh"], subjects: &[], isbn: None, language: "vi", body
        },
    )
    .unwrap();
}

fn id_of(lib: &Library, file: &str) -> i64 {
    lib.conn.query_row("SELECT id FROM books WHERE file_name = ?1", [file], |r| r.get(0)).unwrap()
}

#[test]
fn retitle_renames_writes_content_and_feeds_dedupe() {
    let tmp = tempfile::tempdir().unwrap();
    let books = tmp.path().join("books");
    epub(&books.join("Văn học").join("abc_123.epub"), "Tên tải về sai", "nội dung bản một");
    epub(&books.join("Khác").join("Mắt Biếc.epub"), "Mắt Biếc", "nội dung bản hai, khác hẳn");
    testdata::write_fb2(&books.join("Khác").join("x.fb2"), "Cũ", "Tô", "Hoài", "prose").unwrap();
    testdata::write_pdf(&books.join("Khác").join("scan.pdf"), Some("Old"), None, "abc").unwrap();
    let trash = DirTrash { dir: tmp.path().join("trash"), n: Cell::new(0) };
    std::fs::create_dir_all(&trash.dir).unwrap();
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&books).unwrap();
    scan::scan(&lib.conn, &lib.config, &ScanOptions::default()).unwrap();
    assert!(dedupe::suspect_groups(&lib.conn).unwrap().is_empty(), "tên khác nhau – chưa nghi trùng");

    // EPUB: đổi tên file + ghi vào OPF; bản gốc vào Thùng rác.
    let a = id_of(&lib, "abc_123.epub");
    let opts = RetitleOptions { rename_file: true, write_content: true };
    let r = retitle::retitle_books(&lib.conn, &lib.config, &trash, &[a], "  Mắt   Biếc ", opts).unwrap();
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    let d = &r.done[0];
    assert!(d.renamed && d.content_written, "{d:?}");
    let new_path = books.join("Văn học").join("Mắt Biếc.epub");
    assert_eq!(PathBuf::from(&d.new_path), new_path);
    assert!(new_path.is_file() && !books.join("Văn học").join("abc_123.epub").exists());
    assert_eq!(formats::extract(&new_path, Format::Epub).unwrap().title.as_deref(), Some("Mắt Biếc"));
    assert_eq!(trash.n.get(), 1, "bản gốc vào Thùng rác");
    let b = db::get_book(&lib.conn, a).unwrap().unwrap();
    assert_eq!((b.title.as_str(), b.file_name.as_str()), ("Mắt Biếc", "Mắt Biếc.epub"));
    assert_eq!(b.size, std::fs::metadata(&new_path).unwrap().len() as i64);
    let leftovers: Vec<_> = std::fs::read_dir(books.join("Văn học")).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leftovers.len(), 1, "không còn file tạm: {leftovers:?}");

    // Nghi trùng dùng tiêu đề mới ngay.
    let groups = dedupe::suspect_groups(&lib.conn).unwrap();
    assert_eq!(groups.len(), 1, "{groups:?}");
    let other = id_of(&lib, "Mắt Biếc.epub");
    dedupe::decide(&lib.conn, &[a, other], Decision::NotDuplicate).unwrap();
    assert!(dedupe::suspect_groups(&lib.conn).unwrap().is_empty());

    // Sửa lại (nội dung đổi → băm đổi): quyết định "không trùng" vẫn được nhớ.
    let r = retitle::retitle_books(&lib.conn, &lib.config, &trash, &[a], "Mắt biếc", opts).unwrap();
    assert!(r.done[0].content_written, "{:?}", r.done[0]);
    assert!(dedupe::suspect_groups(&lib.conn).unwrap().is_empty(), "quyết định đã chuyển sang dấu vân tay mới");

    // Quét lại: tiêu đề giữ nguyên.
    scan::scan(&lib.conn, &lib.config, &ScanOptions::default()).unwrap();
    assert_eq!(db::get_book(&lib.conn, a).unwrap().unwrap().title, "Mắt biếc");

    // FB2: ghi <book-title>.
    let f = id_of(&lib, "x.fb2");
    let r = retitle::retitle_books(&lib.conn, &lib.config, &trash, &[f], "Dế Mèn Phiêu Lưu Ký", opts).unwrap();
    assert!(r.done[0].content_written, "{:?}", r.done[0]);
    let fp = books.join("Khác").join("Dế Mèn Phiêu Lưu Ký.fb2");
    assert_eq!(formats::extract(&fp, Format::Fb2).unwrap().title.as_deref(), Some("Dế Mèn Phiêu Lưu Ký"));

    // PDF: chưa ghi được vào nội dung → chỉ DB + tên file, có ghi chú; quét lại khi file đổi vẫn giữ tiêu đề sửa.
    let p = id_of(&lib, "scan.pdf");
    let r = retitle::retitle_books(&lib.conn, &lib.config, &trash, &[p], "Sách: bản quét?", opts).unwrap();
    let d = &r.done[0];
    assert!(d.renamed && !d.content_written && d.note.is_some(), "{d:?}");
    let pp = books.join("Khác").join("Sách - bản quét.pdf");
    assert!(pp.is_file());
    let fh = std::fs::OpenOptions::new().write(true).open(&pp).unwrap();
    fh.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
    drop(fh);
    scan::scan(&lib.conn, &lib.config, &ScanOptions::default()).unwrap();
    let b = db::get_book(&lib.conn, p).unwrap().unwrap();
    assert_eq!(b.title, "Sách: bản quét?", "quét lại không ghi đè tiêu đề người dùng sửa");

    // Chỉ sửa trong thư viện (không đụng file).
    let none = RetitleOptions { rename_file: false, write_content: false };
    let r = retitle::retitle_books(&lib.conn, &lib.config, &trash, &[p], "Chỉ DB", none).unwrap();
    assert!(!r.done[0].renamed && !r.done[0].content_written);
    assert!(pp.is_file());
    assert_eq!(db::get_book(&lib.conn, p).unwrap().unwrap().title, "Chỉ DB");

    // Chỉ đổi hoa / thường.
    let r = retitle::retitle_books(
        &lib.conn,
        &lib.config,
        &trash,
        &[p],
        "SÁCH - BẢN QUÉT",
        RetitleOptions { rename_file: true, write_content: false },
    )
    .unwrap();
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    assert!(r.done[0].new_path.ends_with("SÁCH - BẢN QUÉT.pdf"), "{:?}", r.done[0]);
}
