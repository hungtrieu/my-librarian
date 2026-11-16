//! Thư viện trên hai ổ (`D:\books` + `G:\sach`): thể loại cùng tên gộp làm một; hộp thư đến của mỗi ổ phân loại
//! tại chỗ (đổi tên, không chép sang ổ kia); chuyển tay vào thể loại ở ổ kia thì sách ở lại ổ của nó; cân bằng ổ
//! chuyển nguyên thư mục sang ổ còn chỗ và hoàn tác được.

use librarian_core::trash::Trasher;
use librarian_core::{classify, dedupe, moves, report, testdata, volumes, Library};
use std::path::{Path, PathBuf};

struct NoTrash;
impl Trasher for NoTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        let _ = std::fs::remove_dir_all(path);
        let _ = std::fs::remove_file(path);
        Ok(())
    }
}

fn epub(path: &Path, title: &str, subjects: &[&str]) {
    testdata::write_epub(
        path,
        &testdata::EpubSpec { title, authors: &["Tác giả"], subjects, isbn: None, language: "vi", body: title },
    )
    .unwrap();
}

/// Calibre trong hộp thư đến: thư mục tác phẩm có `metadata.opf` mang tag thể loại.
fn calibre(inbox: &Path, title: &str, tag: &str) {
    let d = inbox.join("Calibre Library").join(title);
    epub(&d.join(format!("{title}.epub")), title, &[]);
    std::fs::write(
        d.join("metadata.opf"),
        format!("<package><metadata><dc:title>{title}</dc:title><dc:subject>{tag}</dc:subject></metadata></package>"),
    )
    .unwrap();
}

struct Env {
    _tmp: tempfile::TempDir,
    d: PathBuf,
    g: PathBuf,
    lib: Library,
}

fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("LIBRARIAN_HOME", tmp.path().join("home"));
    let (d, g) = (tmp.path().join("D").join("books"), tmp.path().join("G").join("sach"));
    epub(&d.join("Văn học").join("A.epub"), "A", &[]);
    epub(&g.join("Van hoc").join("Nguyễn Nhật Ánh").join("B.epub"), "B", &[]);
    epub(&g.join("Trinh thám").join("C.epub"), "C", &[]);
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&d).unwrap();
    lib.add_library_root(&g).unwrap();
    lib.config.min_free_gb = 0; // ổ test dùng chung một đĩa: không ổ nào "sắp đầy"
    let e = Env { _tmp: tmp, d, g, lib };
    analyze(&e);
    e
}

fn analyze(e: &Env) {
    report::run_analysis(&e.lib.conn, &e.lib.config, "app", None, Some(&NoTrash)).unwrap();
}

fn path_of(e: &Env, file: &str) -> PathBuf {
    let p: String = e
        .lib
        .conn
        .query_row("SELECT path FROM books WHERE file_name = ?1 AND status = 'present'", [file], |r| r.get(0))
        .unwrap();
    let p = PathBuf::from(p);
    assert!(p.is_file(), "{} phải có trên đĩa", p.display());
    p
}

#[test]
fn two_drives_form_one_library() {
    let mut e = setup();
    // Không thêm được thư mục lồng nhau / đã có.
    assert!(e.lib.add_library_root(&e.g).is_err());
    assert!(e.lib.add_library_root(&e.d.join("Văn học")).is_err());
    assert_eq!(dedupe::library_root_ids(&e.lib.conn).unwrap().len(), 2, "cả hai ổ đều là thư viện");

    // Thể loại cùng tên (khác dấu) trên hai ổ là một.
    let cats = classify::list_categories(&e.lib.config).unwrap();
    let vh = cats.iter().find(|c| c.key == "van hoc").unwrap();
    assert_eq!(vh.paths.len(), 2);
    assert!(vh.series_dirs.iter().any(|s| s.name == "Nguyễn Nhật Ánh"), "thư mục con ở ổ G cũng thấy");
    assert_eq!(cats.iter().filter(|c| c.key == "trinh tham").count(), 1);

    // Hộp thư đến của từng ổ: sách ở lại ổ đó.
    calibre(&e.g.join("Chưa phân loại"), "Án mạng G", "Trinh thám");
    calibre(&e.d.join("Chưa phân loại"), "Án mạng D", "Trinh thám");
    analyze(&e);
    assert_eq!(path_of(&e, "Án mạng G.epub").parent().unwrap(), e.g.join("Trinh thám"));
    let on_d = path_of(&e, "Án mạng D.epub");
    assert!(on_d.starts_with(&e.d), "không chép sang ổ G: {}", on_d.display());
    assert_eq!(on_d.parent().unwrap().file_name().unwrap(), "Trinh thám");
    assert_eq!(classify::list_categories(&e.lib.config).unwrap().iter().filter(|c| c.key == "trinh tham").count(), 1);

    // Chuyển tay vào "Văn học › Nguyễn Nhật Ánh" (chỉ có ở G): sách trên D ở lại D, thư mục con được tạo ở D.
    let a = e.lib.conn.query_row("SELECT id FROM books WHERE file_name = 'A.epub'", [], |r| r.get(0)).unwrap();
    let req = moves::MoveRequest { book_id: a, target_dir: e.g.join("Van hoc").join("Nguyễn Nhật Ánh") };
    let r = moves::apply_moves(&e.lib.conn, &e.lib.config, &[req], "manual").unwrap();
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    assert_eq!(path_of(&e, "A.epub").parent().unwrap(), e.d.join("Văn học").join("Nguyễn Nhật Ánh"));

    // Bỏ ổ G khỏi thư viện: không đụng file, ổ đó thôi là thư viện.
    assert!(e.lib.remove_library_root(&e.g).unwrap());
    assert_eq!(dedupe::library_root_ids(&e.lib.conn).unwrap().len(), 1);
    assert!(e.g.join("Trinh thám").join("C.epub").is_file());
}

#[test]
fn balance_moves_whole_folders_to_the_other_drive_and_can_be_undone() {
    let e = setup();
    epub(&e.d.join("Văn học").join("Bộ X").join("X1.epub"), "X1", &[]);
    epub(&e.d.join("Văn học").join("Bộ X").join("X2.epub"), "X2", &[]);
    analyze(&e);
    // Muốn chuyển ít nhất 1 byte khỏi D: thư mục lớn nhất đi trước, cả thư mục đi cùng nhau.
    let plan = volumes::balance_plan(&e.lib.conn, &e.lib.config, Some(&e.d), Some(1)).unwrap();
    assert_eq!(plan.groups.len(), 1, "{plan:?}");
    assert_eq!(plan.groups[0].rel, "Văn học/Bộ X");
    assert_eq!(plan.groups[0].books, 2);
    assert_eq!(plan.groups[0].to_root, e.g.to_string_lossy());
    let r = volumes::apply_balance(&e.lib.conn, &e.lib.config, &plan, None, None).unwrap();
    assert_eq!(r.moved.len(), 2, "{:?}", r.failures);
    // Thư mục thể loại "Van hoc" có sẵn ở G (khác dấu) được dùng, không tạo "Văn học" thứ hai.
    assert_eq!(path_of(&e, "X1.epub").parent().unwrap(), e.g.join("Van hoc").join("Bộ X"));
    assert!(!e.d.join("Văn học").join("Bộ X").exists(), "thư mục rỗng ở ổ cũ được dọn");
    moves::undo_batch(&e.lib.conn, &e.lib.config, r.batch_id).unwrap();
    assert_eq!(path_of(&e, "X2.epub").parent().unwrap(), e.d.join("Văn học").join("Bộ X"));
    // Không có ổ nào thiếu chỗ và không yêu cầu: không đề xuất gì.
    assert!(volumes::balance_plan(&e.lib.conn, &e.lib.config, None, None).unwrap().groups.is_empty());
}
