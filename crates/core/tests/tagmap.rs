//! Ánh xạ tag → thể loại: gán tag (mọi cách viết) vào thư mục → sách mang tag được chuyển; đổi đích từ thư mục cha
//! sang thư mục con và ngược lại → sách chuyển theo, thư mục bộ đi cùng sách; xung đột không chuyển; hoàn tác được;
//! sách mới (hộp thư đến, hoặc chép thẳng vào thư mục thể loại) tự theo ánh xạ.

use librarian_core::tagmap::{self, Change, PlanOptions};
use librarian_core::trash::Trasher;
use librarian_core::{moves, report, testdata, Library};
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

struct Env {
    _tmp: tempfile::TempDir,
    books: PathBuf,
    lib: Library,
}

fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("LIBRARIAN_HOME", tmp.path().join("home"));
    let books = tmp.path().join("books");
    epub(&books.join("Văn học").join("A.epub"), "A", &["Tiểu thuyết"]);
    epub(&books.join("Khác").join("B").join("B-01").join("B.epub"), "B", &["tieu-thuyet"]);
    epub(&books.join("Trinh thám").join("C.epub"), "C", &["Novel"]);
    epub(&books.join("Văn học").join("Harry Potter").join("HP.epub"), "HP", &["novel", "Fantasy"]);
    epub(&books.join("Kinh dị").join("D.epub"), "D", &["Horror"]);
    epub(&books.join("Kinh dị").join("E.epub"), "E", &["novel", "Horror"]);
    std::fs::create_dir_all(books.join("Tiểu thuyết").join("Nước ngoài")).unwrap();
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&books).unwrap();
    // Hành vi cũ: thư mục bộ (`Harry Potter`) đi theo sách. Chế độ làm phẳng: xem tests/subcats.rs.
    lib.config.flatten_subfolders = false;
    let e = Env { _tmp: tmp, books, lib };
    analyze(&e);
    e
}

fn analyze(e: &Env) {
    report::run_analysis(&e.lib.conn, &e.lib.config, "app", None, Some(&NoTrash)).unwrap();
}

/// Đổi ánh xạ rồi chuyển sách như app làm.
fn change(e: &mut Env, changes: &[(&str, Option<&str>)]) -> moves::MoveReport {
    let changes: Vec<Change> =
        changes.iter().map(|(t, d)| Change { tag: t.to_string(), target: d.map(str::to_string) }).collect();
    let olds = tagmap::apply_changes(&mut e.lib.config, &changes).unwrap();
    let opts = PlanOptions {
        tags: Some(changes.iter().map(|c| c.tag.clone()).collect()),
        old_targets: olds,
        ..Default::default()
    };
    let plan = tagmap::plan(&e.lib.conn, &e.lib.config, &opts).unwrap();
    tagmap::apply(&e.lib.conn, &e.lib.config, &plan).unwrap()
}

/// Thư mục (tương đối, `/`) của cuốn `file` theo DB, và file có thật ở đó.
fn dir_of(e: &Env, file: &str) -> String {
    let p: String = e
        .lib
        .conn
        .query_row("SELECT path FROM books WHERE file_name = ?1 AND status = 'present'", [file], |r| r.get(0))
        .unwrap();
    assert!(Path::new(&p).is_file(), "{p} phải có trên đĩa");
    let rel = Path::new(&p).parent().unwrap().strip_prefix(&e.books).unwrap();
    rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

#[test]
fn mapping_moves_books_between_parent_and_child_folders() {
    let mut e = setup();
    let tags = tagmap::list_tags(&e.lib.conn, &e.lib.config).unwrap();
    let tt = tags.iter().find(|t| t.key == "tieu thuyet").expect("tag Tiểu thuyết");
    assert_eq!(tt.books, 2, "Tiểu thuyết + tieu-thuyet là một tag");
    assert_eq!(tags.iter().find(|t| t.key == "novel").unwrap().books, 3);

    // Gán ba cách viết vào "Tiểu thuyết".
    let r = change(
        &mut e,
        &[("Tiểu thuyết", Some("Tiểu thuyết")), ("tieu-thuyet", Some("Tiểu thuyết")), ("novel", Some("Tiểu thuyết"))],
    );
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    assert_eq!(e.lib.config.tag_map.len(), 2, "hai cách viết cùng khoá chỉ giữ một dòng");
    assert_eq!(dir_of(&e, "A.epub"), "Tiểu thuyết");
    assert_eq!(dir_of(&e, "B.epub"), "Tiểu thuyết", "từ Khác ra, không mang theo ngăn chữ cái");
    assert_eq!(dir_of(&e, "C.epub"), "Tiểu thuyết");
    assert_eq!(dir_of(&e, "HP.epub"), "Tiểu thuyết/Harry Potter", "thư mục bộ đi theo sách");
    assert_eq!(dir_of(&e, "D.epub"), "Kinh dị", "tag chưa ánh xạ thì để nguyên");
    assert!(!e.books.join("Văn học").join("Harry Potter").exists(), "thư mục bộ cũ rỗng được dọn");
    assert!(e.books.join("Văn học").is_dir(), "thư mục thể loại cấp 1 không bị dọn");

    assert_eq!(dir_of(&e, "E.epub"), "Tiểu thuyết", "lúc này chỉ novel được ánh xạ");

    // Horror → Kinh dị: E mang cả novel + Horror (khác nhánh) nhưng đang nằm ở một trong các đích → không đụng.
    let r = change(&mut e, &[("Horror", Some("Kinh dị"))]);
    assert!(r.moved.is_empty(), "{:?}", r.moved);
    let plan = tagmap::plan(&e.lib.conn, &e.lib.config, &PlanOptions::default()).unwrap();
    assert!(plan.moves.is_empty() && plan.conflicts.is_empty(), "{plan:?}");

    // Cha → con: novel sang "Tiểu thuyết/Nước ngoài".
    let r = change(&mut e, &[("novel", Some("Tiểu thuyết/Nước ngoài"))]);
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    assert_eq!(dir_of(&e, "C.epub"), "Tiểu thuyết/Nước ngoài");
    assert_eq!(dir_of(&e, "HP.epub"), "Tiểu thuyết/Nước ngoài/Harry Potter");
    assert_eq!(dir_of(&e, "A.epub"), "Tiểu thuyết", "tag khác không đổi");
    assert_eq!(dir_of(&e, "E.epub"), "Tiểu thuyết", "xung đột (Tiểu thuyết/Nước ngoài vs Kinh dị) không chuyển");
    let plan = tagmap::plan(&e.lib.conn, &e.lib.config, &PlanOptions::default()).unwrap();
    assert!(plan.moves.is_empty(), "{:?}", plan.moves);
    assert_eq!(plan.conflicts.len(), 1, "E được báo xung đột");
    assert_eq!(plan.conflicts[0].targets, vec!["Kinh dị".to_string(), "Tiểu thuyết/Nước ngoài".to_string()]);

    // Con → cha: novel về lại "Tiểu thuyết".
    let r = change(&mut e, &[("novel", Some("Tiểu thuyết"))]);
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    assert_eq!(dir_of(&e, "C.epub"), "Tiểu thuyết");
    assert_eq!(dir_of(&e, "HP.epub"), "Tiểu thuyết/Harry Potter");
    assert!(!e.books.join("Tiểu thuyết").join("Nước ngoài").join("Harry Potter").exists());

    // Hoàn tác lô vừa rồi: về lại thư mục con.
    moves::undo_batch(&e.lib.conn, &e.lib.config, r.batch_id).unwrap();
    assert_eq!(dir_of(&e, "C.epub"), "Tiểu thuyết/Nước ngoài");
    assert_eq!(dir_of(&e, "HP.epub"), "Tiểu thuyết/Nước ngoài/Harry Potter");

    // Áp dụng lại toàn bộ: "Nước ngoài" không còn là đích của tag nào → coi như thư mục con người dùng tự chia bên
    // trong đúng thể loại, sách trong đó để nguyên.
    let plan = tagmap::plan(&e.lib.conn, &e.lib.config, &PlanOptions::default()).unwrap();
    assert!(plan.moves.is_empty(), "{:?}", plan.moves);
}

#[test]
fn new_books_follow_the_mapping() {
    let mut e = setup();
    change(&mut e, &[("novel", Some("Tiểu thuyết/Nước ngoài"))]);
    // Calibre trong hộp thư đến: tag "NOVEL" – luật có sẵn sẽ xếp vào "Tiểu thuyết", ánh xạ đi trước.
    let d = e.books.join("Chưa phân loại").join("Calibre Library").join("Moby Dick");
    std::fs::create_dir_all(&d).unwrap();
    epub(&d.join("Moby Dick.epub"), "Moby Dick", &[]);
    std::fs::write(
        d.join("metadata.opf"),
        "<package><metadata><dc:title>Moby Dick</dc:title><dc:subject>NOVEL</dc:subject></metadata></package>",
    )
    .unwrap();
    // Chép thẳng vào một thư mục thể loại, tag nằm trong file.
    epub(&e.books.join("Kinh dị").join("F.epub"), "F", &["Novel"]);
    // Sách cũ đúng chỗ khác không bị đụng nếu không có tag ánh xạ.
    analyze(&e);
    assert_eq!(dir_of(&e, "Moby Dick.epub"), "Tiểu thuyết/Nước ngoài");
    assert_eq!(dir_of(&e, "F.epub"), "Tiểu thuyết/Nước ngoài");
    assert_eq!(dir_of(&e, "D.epub"), "Kinh dị");
}
