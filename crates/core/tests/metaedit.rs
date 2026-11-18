//! Sửa thông tin sách: ghi vào EPUB, chỉ DB với định dạng khác, giữ qua lần quét sau (metaedit.rs).

use librarian_core::metaedit::{self, MetaEdit};
use librarian_core::trash::Trasher;
use librarian_core::{db, report, testdata, Library};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

struct KeepTrash(PathBuf, RefCell<usize>);
impl Trasher for KeepTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        *self.1.borrow_mut() += 1;
        std::fs::rename(path, self.0.join(format!("{}", self.1.borrow()))).map_err(|e| e.to_string())
    }
}

#[test]
fn edit_writes_epub_and_survives_rescan() {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("LIBRARIAN_HOME", tmp.path().join("home"));
    let books = tmp.path().join("books");
    let ep = books.join("Văn học").join("x.epub");
    testdata::write_epub(&ep, &testdata::EpubSpec { title: "Cũ", authors: &["Ai"], subjects: &[], isbn: None, language: "en", body: "b" }).unwrap();
    let txt = books.join("Văn học").join("y.txt");
    std::fs::write(&txt, "noi dung").unwrap();
    let trash = KeepTrash(tmp.path().join("trash"), RefCell::new(0));
    std::fs::create_dir_all(&trash.0).unwrap();
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&books).unwrap();
    lib.config.auto_triage = false;
    report::run_analysis(&lib.conn, &lib.config, "app", None, Some(&trash)).unwrap();
    let id = |f: &str| -> i64 { lib.conn.query_row("SELECT id FROM books WHERE file_name = ?1", [f], |r| r.get(0)).unwrap() };
    let m = MetaEdit {
        title: "Mắt biếc".into(),
        authors: vec!["Nguyễn Nhật Ánh".into()],
        tags: vec!["Văn học".into(), "Tuổi học trò".into()],
        series: "Truyện dài".into(),
        volume: Some(2),
        language: "vi".into(),
        isbn: String::new(),
    };
    let r = metaedit::edit(&lib.conn, &lib.config, &trash, id("x.epub"), &m, true).unwrap();
    assert!(r.written, "{r:?}");
    assert_eq!(*trash.1.borrow(), 1, "bản gốc vào Thùng rác");
    let md = librarian_core::formats::extract(&ep, librarian_core::formats::Format::Epub).unwrap();
    assert_eq!(md.title.as_deref(), Some("Mắt biếc"));
    assert_eq!(md.subjects, vec!["Văn học", "Tuổi học trò"]);
    // TXT: chỉ trong thư viện.
    let r = metaedit::edit(&lib.conn, &lib.config, &trash, id("y.txt"), &MetaEdit { title: "Ghi chép".into(), authors: vec!["Tôi".into()], ..Default::default() }, true).unwrap();
    assert!(!r.written && r.note.is_some());
    // Quét lại (file đổi nội dung → đọc lại metadata): thông tin đã sửa vẫn giữ.
    std::fs::write(&txt, "noi dung moi hon").unwrap();
    report::run_analysis(&lib.conn, &lib.config, "app", None, Some(&trash)).unwrap();
    let b = db::get_book(&lib.conn, id("y.txt")).unwrap().unwrap();
    assert_eq!((b.title.as_str(), b.authors.as_str()), ("Ghi chép", "Tôi"));
    let b = db::get_book(&lib.conn, id("x.epub")).unwrap().unwrap();
    assert_eq!((b.title.as_str(), b.series.as_deref(), b.volume, b.subjects.as_str()), ("Mắt biếc", Some("Truyện dài"), Some(2), "Văn học; Tuổi học trò"));
}
