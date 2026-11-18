//! Thư mục con do app quản lý và làm phẳng thư mục con chép theo từ nguồn (subcats.rs).

use librarian_core::trash::Trasher;
use librarian_core::{classify, folders, moves, report, subcats, testdata, Library};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// Thùng rác giả: ghi lại rồi xoá thật.
struct NoTrash(RefCell<Vec<PathBuf>>);
impl Trasher for NoTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        self.0.borrow_mut().push(path.to_path_buf());
        let _ = std::fs::remove_dir_all(path);
        let _ = std::fs::remove_file(path);
        Ok(())
    }
}

fn epub(path: &Path, title: &str) {
    testdata::write_epub(
        path,
        &testdata::EpubSpec { title, authors: &["Tác giả"], subjects: &[], isbn: None, language: "vi", body: title },
    )
    .unwrap();
}

struct Env {
    _tmp: tempfile::TempDir,
    books: PathBuf,
    lib: Library,
    trash: NoTrash,
}

fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("LIBRARIAN_HOME", tmp.path().join("home"));
    let books = tmp.path().join("books");
    let vh = books.join("Văn học");
    epub(&vh.join("Áo trắng.epub"), "A");
    epub(&vh.join("NNA").join("Bến xe.epub"), "B");
    epub(&vh.join("NNA").join("Bộ 1").join("01.epub"), "Một");
    std::fs::write(vh.join("NNA").join("cover.jpg"), b"jpg").unwrap();
    std::fs::write(vh.join("NNA").join("Ghi chép.docx"), b"docx").unwrap();
    epub(&vh.join("Nước ngoài").join("Cánh đồng.epub"), "C");
    epub(&vh.join("Nước ngoài").join("Murakami").join("Rừng Na Uy.epub"), "D");
    // Trùng tên với file ở thư mục thể loại.
    epub(&vh.join("Khác nữa").join("Áo trắng.epub"), "A2");
    epub(&books.join("Khác").join("K").join("K-01").join("K.epub"), "K");
    epub(&books.join("Chưa phân loại").join("Mới").join("M.epub"), "M");
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&books).unwrap();
    lib.config.min_free_gb = 0;
    lib.config.auto_triage = true;
    lib.config.subcategories = vec!["Văn học/Nước ngoài".into()];
    Env { _tmp: tmp, books, lib, trash: NoTrash(RefCell::new(Vec::new())) }
}

fn analyze(e: &Env) -> report::ReportStats {
    report::run_analysis(&e.lib.conn, &e.lib.config, "app", None, Some(&e.trash)).unwrap()
}

fn path_of(e: &Env, title: &str) -> PathBuf {
    let p: String = e
        .lib
        .conn
        .query_row("SELECT path FROM books WHERE title = ?1 AND status = 'present'", [title], |r| r.get(0))
        .unwrap();
    let p = PathBuf::from(p);
    assert!(p.is_file(), "{} phải có trên đĩa", p.display());
    p
}

#[test]
fn scan_flattens_unmanaged_subfolders_and_can_be_undone() {
    let mut e = setup();
    e.lib.config.auto_triage = true;
    let st = analyze(&e);
    let vh = e.books.join("Văn học");
    assert_eq!(st.auto_flattened, Some(5), "{st:?}");
    assert_eq!(path_of(&e, "A"), vh.join("Áo trắng.epub"));
    assert_eq!(path_of(&e, "B"), vh.join("Bến xe.epub"));
    assert_eq!(path_of(&e, "Một"), vh.join("NNA - Bộ 1 - 01.epub"), "tên chung chung: thêm tên thư mục");
    assert_eq!(path_of(&e, "A2"), vh.join("Áo trắng (1).epub"), "trùng tên thêm (1)");
    assert_eq!(path_of(&e, "C"), vh.join("Nước ngoài").join("Cánh đồng.epub"), "thư mục con do app quản lý giữ nguyên");
    assert_eq!(path_of(&e, "D"), vh.join("Nước ngoài").join("Rừng Na Uy.epub"), "thư mục lạ bên trong thư mục được quản lý");
    assert!(vh.join("Ghi chép.docx").is_file(), "file sách app chưa đọc được cũng được đưa ra");
    assert!(!vh.join("NNA").exists() && !vh.join("Khác nữa").exists(), "thư mục lạ hết sách vào Thùng rác");
    assert!(!vh.join("Nước ngoài").join("Murakami").exists());
    assert!(e.books.join("Khác").join("K").join("K-01").join("K.epub").is_file(), "Khác giữ cấu trúc ngăn");
    // Bộ của sách không còn là thư mục cũ; thư mục con được quản lý không phải "bộ".
    let series: Option<String> =
        e.lib.conn.query_row("SELECT series FROM books WHERE title = 'C'", [], |r| r.get(0)).unwrap();
    assert_eq!(series, None);

    // Quét lại: không còn gì để làm.
    let st = analyze(&e);
    assert_eq!(st.auto_flattened, Some(0));

    // Hoàn tác lô làm phẳng: sách về chỗ cũ (thư mục được tạo lại).
    let batch: i64 = e
        .lib
        .conn
        .query_row("SELECT id FROM batches WHERE kind = ?1 ORDER BY id LIMIT 1", [subcats::FLATTEN_KIND], |r| r.get(0))
        .unwrap();
    let u = moves::undo_batch(&e.lib.conn, &e.lib.config, batch).unwrap();
    assert_eq!(u.restored, 4, "{:?}", u.failures); // file .docx (app chưa đọc) chỉ được chuyển, không vào lô
    assert_eq!(path_of(&e, "B"), vh.join("NNA").join("Bến xe.epub"));
    assert_eq!(path_of(&e, "Một"), vh.join("NNA").join("Bộ 1").join("01.epub"));

    // Tắt làm phẳng: thư mục con lạ được giữ.
    e.lib.config.flatten_subfolders = false;
    let st = analyze(&e);
    assert_eq!(st.auto_flattened, None);
    assert_eq!(path_of(&e, "B"), vh.join("NNA").join("Bến xe.epub"));
}

#[test]
fn creating_a_subfolder_registers_or_adopts_it() {
    let mut e = setup();
    e.lib.config.flatten_subfolders = false;
    analyze(&e);
    // Tạo mới.
    let rel = folders::create(&mut e.lib.config, Some("van hoc"), "Thơ").unwrap();
    assert_eq!(rel, "Văn học/Thơ");
    assert!(subcats::is_registered(&e.lib.config, "Văn học/Thơ"));
    // Thư mục có sẵn (chép theo từ nguồn) cùng tên: app nhận luôn, không báo lỗi.
    let rel = folders::create(&mut e.lib.config, Some("Văn học"), "nna").unwrap();
    assert_eq!(rel, "Văn học/NNA");
    assert!(folders::create(&mut e.lib.config, Some("Văn học"), "NNA").is_err(), "đã quản lý rồi thì báo trùng");
    // Danh sách cho thanh bên, kèm số sách.
    let list = subcats::list(&e.lib.conn, &e.lib.config).unwrap();
    let rels: Vec<(&str, i64)> = list.iter().map(|s| (s.rel.as_str(), s.books)).collect();
    assert_eq!(rels, vec![("Văn học/NNA", 2), ("Văn học/Nước ngoài", 2), ("Văn học/Thơ", 0)]);
    // Chỉ thư mục con do app quản lý là đích phân loại.
    e.lib.config.flatten_subfolders = true;
    let cats = classify::list_categories(&e.lib.config).unwrap();
    let vh = cats.iter().find(|c| c.key == "van hoc").unwrap();
    let mut subs: Vec<&str> = vh.series_dirs.iter().map(|s| s.name.as_str()).collect();
    subs.sort();
    assert_eq!(subs, vec!["NNA", "Nước ngoài", "Thơ"]);
    // Quét: NNA được giữ, Bộ 1 bên trong thì không.
    analyze(&e);
    let vhd = e.books.join("Văn học");
    assert_eq!(path_of(&e, "B"), vhd.join("NNA").join("Bến xe.epub"));
    assert_eq!(path_of(&e, "Một"), vhd.join("NNA").join("Bộ 1 - 01.epub"));
    // Đổi tên / xoá cập nhật danh sách.
    folders::rename(&e.lib.conn, &mut e.lib.config, "Văn học/NNA", "Nguyễn Nhật Ánh").unwrap();
    assert!(subcats::is_registered(&e.lib.config, "Văn học/Nguyễn Nhật Ánh"));
    assert!(!subcats::is_registered(&e.lib.config, "Văn học/NNA"));
    folders::forget(&mut e.lib.config, "Văn học/Thơ").unwrap();
    assert!(!subcats::is_registered(&e.lib.config, "Văn học/Thơ"));
}
