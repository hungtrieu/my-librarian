//! Thể loại bị chặn: chặn một thư mục (kể cả thư mục con tự chia) → sách trong đó vào Thùng rác, thư mục biến mất;
//! sách mới cùng thể loại tự phân loại → vào thẳng Thùng rác, thư mục không được tạo lại; thư mục bị chặn mà xuất hiện
//! lại thì lần quét sau dọn tiếp.

use librarian_core::trash::Trasher;
use librarian_core::{blocked, deleted, report, testdata, Library};
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

fn epub(path: &Path, title: &str, subjects: &[&str], body: &str) {
    testdata::write_epub(
        path,
        &testdata::EpubSpec { title, authors: &["Tác giả"], subjects, isbn: None, language: "vi", body },
    )
    .unwrap();
}

/// Calibre: thư mục tác phẩm có `metadata.opf` mang tag thể loại.
fn calibre(inbox: &Path, title: &str, tag: &str) {
    let d = inbox.join("Calibre Library").join(title);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(format!("{title}.epub")), format!("noi dung {title}")).unwrap();
    std::fs::write(
        d.join("metadata.opf"),
        format!("<package><metadata><dc:title>{title}</dc:title><dc:subject>{tag}</dc:subject></metadata></package>"),
    )
    .unwrap();
}

struct Env {
    _tmp: tempfile::TempDir,
    books: PathBuf,
    lib: Library,
    trash: DirTrash,
}

const PARENT: &str = "Ngôn tình - Đam mỹ";

fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("LIBRARIAN_HOME", tmp.path().join("home")); // nhật ký phân loại ghi vào đây
    let books = tmp.path().join("books");
    // Người dùng tự chia thể loại thành hai thư mục con.
    epub(&books.join(PARENT).join("Đam mỹ").join("Thiên quan tứ phúc.epub"), "Thiên quan tứ phúc", &[], "dm1");
    epub(&books.join(PARENT).join("Đam mỹ").join("Ma đạo tổ sư.epub"), "Ma đạo tổ sư", &[], "dm2");
    epub(&books.join(PARENT).join("Ngôn tình").join("Bên nhau trọn đời.epub"), "Bên nhau trọn đời", &[], "nt1");
    epub(&books.join("Văn học").join("Mắt Biếc.epub"), "Mắt Biếc", &[], "vh1");
    let trash = DirTrash { dir: tmp.path().join("trash"), n: Cell::new(0) };
    std::fs::create_dir_all(&trash.dir).unwrap();
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&books).unwrap();
    // Hai thư mục con tạo bằng app (thư mục con lạ thì bị làm phẳng khi quét).
    lib.config.subcategories = vec![format!("{PARENT}/Đam mỹ"), format!("{PARENT}/Ngôn tình")];
    let e = Env { _tmp: tmp, books, lib, trash };
    analyze(&e);
    e
}

fn analyze(e: &Env) -> report::ReportStats {
    report::run_analysis(&e.lib.conn, &e.lib.config, "app", None, Some(&e.trash)).unwrap()
}

fn present(e: &Env, file: &str) -> bool {
    e.lib
        .conn
        .query_row("SELECT COUNT(*) FROM books WHERE file_name = ?1 AND status = 'present'", [file], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
        > 0
}

#[test]
fn blocking_a_subfolder_trashes_its_books_and_new_ones_of_that_genre() {
    let mut e = setup();
    let rel = blocked::add(&mut e.lib.config, &format!("{PARENT}/Đam mỹ")).unwrap();
    let r = blocked::enforce(&e.lib.conn, &e.lib.config, &e.trash, Some(&rel), None, None).unwrap();
    assert_eq!(r.trashed, 2, "{r:?}");
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    assert!(!e.books.join(PARENT).join("Đam mỹ").exists(), "thư mục bị chặn vào Thùng rác");
    assert!(e.books.join(PARENT).join("Ngôn tình/Bên nhau trọn đời.epub").is_file(), "thư mục anh em giữ nguyên");
    assert!(!present(&e, "Ma đạo tổ sư.epub"));
    assert!(present(&e, "Bên nhau trọn đời.epub"));
    assert_eq!(deleted::count(&e.lib.conn).unwrap(), 2, "ghi vào sổ sách đã xoá");
    let kind: String =
        e.lib.conn.query_row("SELECT kind FROM batches ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(kind, "block");

    // Sách mới: đam mỹ → Thùng rác (thư mục không được tạo lại), ngôn tình → thư mục con của nó.
    let inbox = e.books.join("Chưa phân loại");
    calibre(&inbox, "Sát phá lang", "Đam mỹ");
    calibre(&inbox, "Truyện tình mới", "Romance; Đam mỹ");
    calibre(&inbox, "Yêu em từ cái nhìn đầu tiên", "Ngôn tình");
    let st = analyze(&e);
    assert_eq!(st.auto_blocked, Some(2), "{st:?}");
    assert!(!e.books.join(PARENT).join("Đam mỹ").exists(), "không tạo lại thư mục bị chặn");
    assert!(e.books.join(PARENT).join("Ngôn tình/Yêu em từ cái nhìn đầu tiên.epub").is_file());
    assert!(!inbox.join("Calibre Library").exists());
    assert_eq!(
        std::fs::read_dir(&e.trash.dir).unwrap().count(),
        2 + 1 + 2 + 1,
        "2 sách + thư mục + 2 sách mới + thư mục Calibre thừa"
    );
    let banner = report::banner(&report::latest(&e.lib.conn).unwrap().unwrap()).unwrap();
    assert!(banner.contains("2 sách thuộc thể loại bị chặn"), "{banner}");
}

#[test]
fn blocked_folder_recreated_by_hand_is_emptied_on_next_scan() {
    let mut e = setup();
    let rel = blocked::add(&mut e.lib.config, PARENT).unwrap();
    let r = blocked::enforce(&e.lib.conn, &e.lib.config, &e.trash, Some(&rel), None, None).unwrap();
    assert_eq!(r.trashed, 3);
    assert!(!e.books.join(PARENT).exists());

    // Tạo lại tay (đổi kiểu chữ) + thêm sách; tự phân loại cùng thể loại cũng không tạo thư mục mới.
    epub(&e.books.join("ngôn tình - đam mỹ").join("Cuốn mới.epub"), "Cuốn mới", &[], "x");
    calibre(&e.books.join("Chưa phân loại"), "Tổng tài bá đạo", "Ngôn tình");
    let st = analyze(&e);
    assert_eq!(st.auto_blocked, Some(2), "{st:?}");
    assert!(!e.books.join("ngôn tình - đam mỹ").exists());
    assert!(!e.books.join(PARENT).exists(), "không tạo thư mục mặc định của luật");
    assert!(e.books.join("Văn học/Mắt Biếc.epub").is_file());

    // Bỏ chặn: lần sau thể loại được tạo lại như thường.
    assert!(blocked::remove(&mut e.lib.config, &rel));
    calibre(&e.books.join("Chưa phân loại"), "Hoa hồng", "Ngôn tình");
    let st = analyze(&e);
    assert_eq!(st.auto_blocked, None);
    assert!(e.books.join(PARENT).join("Hoa hồng.epub").is_file());
}

#[test]
fn folder_tree_lists_subfolders_counts_and_removed_blocked_entries() {
    let mut e = setup();
    let rel = blocked::add(&mut e.lib.config, &format!("{PARENT}/Đam mỹ")).unwrap();
    let t = blocked::folder_tree(&e.lib.conn, &e.lib.config, 3).unwrap();
    let get = |r: &str| t.iter().find(|n| n.rel == r).unwrap_or_else(|| panic!("{r}: {t:?}")).clone();
    assert_eq!(get(PARENT).books, 3);
    assert_eq!(get(&format!("{PARENT}/Đam mỹ")).books, 2);
    assert!(get(&format!("{PARENT}/Đam mỹ")).blocked);
    assert!(!get(&format!("{PARENT}/Ngôn tình")).blocked);
    assert!(t.iter().all(|n| n.rel != "Chưa phân loại"), "không liệt kê hộp thư đến");

    blocked::enforce(&e.lib.conn, &e.lib.config, &e.trash, Some(&rel), None, None).unwrap();
    let t = blocked::folder_tree(&e.lib.conn, &e.lib.config, 3).unwrap();
    let gone = t.iter().find(|n| n.rel == rel).expect("mục bị chặn vẫn hiện để bỏ chặn");
    assert!(gone.blocked && !gone.exists);
}

#[test]
fn enforce_does_nothing_without_blocked_folders_on_disk() {
    let mut e = setup();
    blocked::add(&mut e.lib.config, "Không có").unwrap();
    let r = blocked::enforce(&e.lib.conn, &e.lib.config, &e.trash, None, None, None).unwrap();
    assert_eq!(r.trashed, 0);
    assert_eq!(r.batch_id, 0, "không tạo lô rỗng trong lịch sử mỗi lần quét");
    e.lib.config.read_only = true;
    assert!(blocked::enforce(&e.lib.conn, &e.lib.config, &e.trash, None, None, None).is_err());
}
