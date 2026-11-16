//! Quản lý thư mục thể loại trong app (sửa thẳng trên mọi ổ) và "chuyển luôn sách cùng tag trong Khác".

use librarian_core::trash::Trasher;
use librarian_core::{classify, folders, moves, report, tagmap, testdata, Library};
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
    d: PathBuf,
    g: PathBuf,
    lib: Library,
}

fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("LIBRARIAN_HOME", tmp.path().join("home"));
    let (d, g) = (tmp.path().join("D").join("books"), tmp.path().join("G").join("sach"));
    epub(&d.join("Văn học").join("A.epub"), "A", &[]);
    epub(&d.join("Văn học").join("NNA").join("B.epub"), "B", &[]);
    epub(&g.join("Van hoc").join("C.epub"), "C", &[]);
    epub(&d.join("Khác").join("K").join("K-01").join("K1.epub"), "K1", &["Novel; Classics"]);
    epub(&d.join("Khác").join("K").join("K-01").join("K2.epub"), "K2", &["novel"]);
    epub(&g.join("Khác").join("K").join("K-01").join("K3.epub"), "K3", &["Classics"]);
    epub(&d.join("Khác").join("K").join("K-01").join("K4.epub"), "K4", &["Horror"]);
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&d).unwrap();
    lib.add_library_root(&g).unwrap();
    lib.config.min_free_gb = 0;
    lib.config.tag_map =
        vec![librarian_core::config::TagRule { tag: "horror".into(), target: "Văn học/NNA".into() }];
    let e = Env { _tmp: tmp, d, g, lib };
    report::run_analysis(&e.lib.conn, &e.lib.config, "app", None, Some(&NoTrash)).unwrap();
    e
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

fn id_of(e: &Env, file: &str) -> i64 {
    e.lib.conn.query_row("SELECT id FROM books WHERE file_name = ?1", [file], |r| r.get(0)).unwrap()
}

#[test]
fn rename_create_and_delete_folders_on_every_drive() {
    let mut e = setup();
    // Đổi tên thể loại: cả hai ổ (G viết không dấu), đường dẫn sách và ánh xạ tag đổi theo.
    let r = folders::rename(&e.lib.conn, &mut e.lib.config, "Văn học", "Văn chương").unwrap();
    assert_eq!(r.dirs.len(), 2, "{r:?}");
    assert_eq!(r.books, 4, "A, B, C và K4 (Horror – ánh xạ vào Văn học/NNA khi quét)");
    assert_eq!(path_of(&e, "A.epub"), e.d.join("Văn chương").join("A.epub"));
    assert_eq!(path_of(&e, "B.epub"), e.d.join("Văn chương").join("NNA").join("B.epub"));
    assert_eq!(path_of(&e, "K4.epub"), e.d.join("Văn chương").join("NNA").join("K4.epub"));
    assert_eq!(path_of(&e, "C.epub"), e.g.join("Văn chương").join("C.epub"));
    assert_eq!(e.lib.config.tag_map[0].target, "Văn chương/NNA");
    let cats = classify::list_categories(&e.lib.config).unwrap();
    assert_eq!(cats.iter().find(|c| c.name == "Văn chương").unwrap().paths.len(), 2);
    let cat: Option<String> =
        e.lib.conn.query_row("SELECT category FROM books WHERE file_name = 'C.epub'", [], |r| r.get(0)).unwrap();
    assert_eq!(cat.as_deref(), Some("Văn chương"));
    // Không đổi được thành tên đã có, không đụng được Khác / hộp thư đến.
    std::fs::create_dir_all(e.d.join("Trinh thám")).unwrap();
    assert!(folders::rename(&e.lib.conn, &mut e.lib.config, "Văn chương", "trinh tham").is_err());
    assert!(folders::rename(&e.lib.conn, &mut e.lib.config, "Khác", "Linh tinh").is_err());
    // Đổi tên thư mục con.
    folders::rename(&e.lib.conn, &mut e.lib.config, "Văn chương/NNA", "Nguyễn Nhật Ánh").unwrap();
    assert_eq!(path_of(&e, "B.epub"), e.d.join("Văn chương").join("Nguyễn Nhật Ánh").join("B.epub"));
    assert_eq!(e.lib.config.tag_map[0].target, "Văn chương/Nguyễn Nhật Ánh");

    // Thêm thể loại / thư mục con.
    assert_eq!(folders::create(&e.lib.config, None, "Thơ").unwrap(), "Thơ");
    assert!(e.d.join("Thơ").is_dir());
    assert_eq!(folders::create(&e.lib.config, Some("van chuong"), "Mới").unwrap(), "Văn chương/Mới");
    assert!(e.d.join("Văn chương").join("Mới").is_dir());
    assert!(folders::create(&e.lib.config, None, "tho").is_err(), "trùng tên");
    assert!(folders::create(&e.lib.config, Some("Khác"), "x").is_err());

    // Xoá thư mục con: sách lên thư mục cha.
    let r = folders::delete(&e.lib.conn, &e.lib.config, &NoTrash, "Văn chương/Nguyễn Nhật Ánh", None, None).unwrap();
    assert_eq!(r.moves.moved.len(), 2);
    assert_eq!(path_of(&e, "B.epub"), e.d.join("Văn chương").join("B.epub"));
    assert!(!e.d.join("Văn chương").join("Nguyễn Nhật Ánh").exists());

    // Xoá thể loại: sách vào Khác trên chính ổ của nó, cả hai thư mục biến mất; hoàn tác trả sách về.
    folders::forget(&mut e.lib.config, "Văn chương").unwrap();
    assert!(e.lib.config.tag_map.is_empty(), "ánh xạ vào thư mục đã xoá được bỏ");
    let r = folders::delete(&e.lib.conn, &e.lib.config, &NoTrash, "Văn chương", None, None).unwrap();
    assert_eq!(r.removed.len(), 2, "{r:?}");
    assert!(path_of(&e, "C.epub").starts_with(e.g.join("Khác")), "không chép sang ổ khác");
    assert!(path_of(&e, "A.epub").starts_with(e.d.join("Khác")));
    moves::undo_batch(&e.lib.conn, &e.lib.config, r.moves.batch_id).unwrap();
    assert_eq!(path_of(&e, "C.epub"), e.g.join("Văn chương").join("C.epub"));
}

#[test]
fn moving_a_book_out_of_other_offers_the_rest_with_the_same_tags() {
    let e = setup();
    let from = path_of(&e, "K1.epub").to_string_lossy().into_owned();
    let req = moves::MoveRequest { book_id: id_of(&e, "K1.epub"), target_dir: e.d.join("Văn học") };
    let r = moves::apply_moves(&e.lib.conn, &e.lib.config, &[req], "manual").unwrap();
    assert_eq!(r.moved.len(), 1);
    let sug = tagmap::other_suggestions(&e.lib.conn, &e.lib.config, &[(id_of(&e, "K1.epub"), from)]).unwrap();
    assert_eq!(sug.len(), 1);
    assert_eq!(sug[0].target, "Văn học");
    let tags: Vec<(&str, usize)> = sug[0].tags.iter().map(|t| (t.key.as_str(), t.books)).collect();
    assert_eq!(tags, vec![("classics", 1), ("novel", 1)]);
    assert_eq!(sug[0].books, 2);
    // Chỉ chọn "novel": K2 chuyển (ổ D), K3 (Classics, ổ G) và K4 (Horror) ở lại.
    let plan = tagmap::plan_from_other(&e.lib.conn, &e.lib.config, &sug[0].target, &["Novel".into()]).unwrap();
    assert_eq!(plan.moves.len(), 1);
    tagmap::apply(&e.lib.conn, &e.lib.config, &plan).unwrap();
    assert_eq!(path_of(&e, "K2.epub"), e.d.join("Văn học").join("K2.epub"));
    assert!(path_of(&e, "K3.epub").starts_with(e.g.join("Khác")));
    // Classics: K3 nằm ở ổ G → vào "Van hoc" có sẵn trên G, không chép sang D.
    let plan = tagmap::plan_from_other(&e.lib.conn, &e.lib.config, "Văn học", &["classics".into()]).unwrap();
    tagmap::apply(&e.lib.conn, &e.lib.config, &plan).unwrap();
    assert_eq!(path_of(&e, "K3.epub"), e.g.join("Van hoc").join("K3.epub"));
    // Sách chuyển giữa hai thể loại thường (không phải từ Khác): không gợi ý gì.
    let from = path_of(&e, "A.epub").to_string_lossy().into_owned();
    assert!(tagmap::other_suggestions(&e.lib.conn, &e.lib.config, &[(id_of(&e, "A.epub"), from)]).unwrap().is_empty());
}
