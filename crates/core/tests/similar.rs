//! Đưa sách không có metadata ra khỏi `Khác`: sách tương tự (tác giả trong tên file, bộ, cụm tên) và màn
//! "Phân loại Khác" (similar.rs).

use librarian_core::trash::Trasher;
use librarian_core::{moves, report, similar, tagmap, testdata, Library};
use std::path::{Path, PathBuf};

struct NoTrash;
impl Trasher for NoTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        let _ = std::fs::remove_dir_all(path);
        let _ = std::fs::remove_file(path);
        Ok(())
    }
}

fn epub(path: &Path, title: &str, author: &str) {
    testdata::write_epub(
        path,
        &testdata::EpubSpec { title, authors: &[author], subjects: &[], isbn: None, language: "vi", body: title },
    )
    .unwrap();
}

/// Sách không có metadata: file .txt, tiêu đề lấy từ tên file.
fn txt(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("noi dung {}", path.display())).unwrap();
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
    epub(&books.join("Văn học").join("Mắt biếc.epub"), "Mắt biếc", "Nguyễn Nhật Ánh");
    epub(&books.join("Văn học").join("Tôi thấy hoa vàng.epub"), "Tôi thấy hoa vàng trên cỏ xanh", "Nguyễn Nhật Ánh");
    epub(
        &books.join("Kỹ năng").join("Tuổi trẻ đáng giá bao nhiêu.epub"),
        "Tuổi trẻ đáng giá bao nhiêu",
        "Rosie Nguyễn",
    );
    epub(&books.join("Kỹ năng").join("Nhà giả kim.epub"), "Kỹ năng sống – nhà giả kim", "Paulo Coelho");
    epub(&books.join("Truyện tranh").join("Doraemon 1.epub"), "Doraemon 1", "Fujiko F. Fujio");
    let other = books.join("Khác");
    txt(&other.join("Kính vạn hoa - Nguyễn Nhật Ánh.txt"));
    txt(&other.join("Cho tôi xin một vé đi tuổi thơ - Nguyen Nhat Anh.txt"));
    txt(&other.join("Ngồi khóc trên cây - Nguyễn Nhật Ánh.txt"));
    txt(&other.join("Conan - Tập 01.txt"));
    txt(&other.join("Conan - Tập 02.txt"));
    txt(&other.join("Conan 45.txt"));
    txt(&other.join("Đắc nhân tâm - Dale Carnegie - First News.txt"));
    txt(&other.join("Quẳng gánh lo đi - First News.txt"));
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&books).unwrap();
    lib.config.min_free_gb = 0;
    lib.config.auto_triage = false; // giữ nguyên `Khác` phẳng của test
    report::run_analysis(&lib.conn, &lib.config, "app", None, Some(&NoTrash)).unwrap();
    Env { _tmp: tmp, books, lib }
}

fn id_of(e: &Env, file: &str) -> i64 {
    e.lib.conn.query_row("SELECT id FROM books WHERE file_name = ?1", [file], |r| r.get(0)).unwrap()
}

fn dir_of(e: &Env, file: &str) -> String {
    let p: String = e.lib.conn.query_row("SELECT path FROM books WHERE file_name = ?1", [file], |r| r.get(0)).unwrap();
    assert!(Path::new(&p).is_file());
    Path::new(&p).parent().unwrap().file_name().unwrap().to_string_lossy().into_owned()
}

/// Chuyển tay một cuốn từ Khác vào `dir` rồi lấy gợi ý.
fn move_and_suggest(e: &Env, file: &str, dir: &str) -> Vec<similar::Suggestion> {
    let id = id_of(e, file);
    let req = moves::MoveRequest { book_id: id, target_dir: e.books.join(dir) };
    let r = moves::apply_moves(&e.lib.conn, &e.lib.config, &[req], "manual").unwrap();
    assert_eq!(r.moved.len(), 1, "{:?}", r.failures);
    let moved: Vec<(i64, String)> = r.moved.iter().map(|m| (m.book_id, m.from.clone())).collect();
    similar::suggestions(&e.lib.conn, &e.lib.config, &moved).unwrap()
}

#[test]
fn similar_books_without_metadata_follow_a_manual_move() {
    let e = setup();
    // Tác giả đoán từ tên file (khớp tác giả đã biết trong thư viện, kể cả không dấu).
    let s = move_and_suggest(&e, "Kính vạn hoa - Nguyễn Nhật Ánh.txt", "Văn học");
    assert_eq!(s.len(), 1);
    let a = &s[0].criteria[0];
    assert_eq!((a.criterion.kind.as_str(), a.books), ("author", 2), "{s:?}");
    let plan =
        similar::plan_similar(&e.lib.conn, &e.lib.config, &s[0].target, std::slice::from_ref(&a.criterion)).unwrap();
    assert_eq!(plan.moves.len(), 2);
    let r = tagmap::apply_kind(&e.lib.conn, &e.lib.config, &plan, similar::OTHER_KIND, None, None).unwrap();
    assert_eq!(r.moved.len(), 2);
    assert_eq!(dir_of(&e, "Cho tôi xin một vé đi tuổi thơ - Nguyen Nhat Anh.txt"), "Văn học");

    // Bộ suy từ tên file (bỏ số tập).
    let s = move_and_suggest(&e, "Conan - Tập 01.txt", "Truyện tranh");
    let c = s[0].criteria.iter().find(|c| c.criterion.kind == "series").expect("có dấu hiệu bộ");
    assert_eq!(c.books, 2, "{s:?}");

    // Cụm tên file chung (nhà phát hành) – chưa biết tác giả.
    let s = move_and_suggest(&e, "Đắc nhân tâm - Dale Carnegie - First News.txt", "Kỹ năng");
    let p = s[0].criteria.iter().find(|c| c.criterion.key == "first news").expect("cụm “First News”");
    assert_eq!((p.criterion.kind.as_str(), p.books), ("part", 1));
}

#[test]
fn model_guesses_categories_for_books_in_other() {
    let e = setup();
    let v = similar::other_view(&e.lib.conn, &e.lib.config, 45).unwrap();
    assert_eq!(v.total, 8);
    assert!(v.groups.iter().all(|g| g.target != "Khác"), "không đoán Khác");
    let vh = v.groups.iter().find(|g| g.target == "Văn học").expect("đoán Văn học theo tác giả trong tên file");
    assert_eq!(vh.books, 3, "{v:?}");
    let plan = similar::other_plan(&e.lib.conn, &e.lib.config, &["van hoc".into()], 45, &[]).unwrap();
    assert_eq!(plan.moves.len(), 3);
    let excl = id_of(&e, "Kính vạn hoa - Nguyễn Nhật Ánh.txt");
    let plan = similar::other_plan(&e.lib.conn, &e.lib.config, &["Văn học".into()], 45, &[excl]).unwrap();
    assert_eq!(plan.moves.len(), 2, "bỏ tick một cuốn");
    let r = tagmap::apply_kind(&e.lib.conn, &e.lib.config, &plan, similar::OTHER_KIND, None, None).unwrap();
    assert_eq!(r.moved.len(), 2);
    assert_eq!(dir_of(&e, "Ngồi khóc trên cây - Nguyễn Nhật Ánh.txt"), "Văn học");
    assert_eq!(dir_of(&e, "Kính vạn hoa - Nguyễn Nhật Ánh.txt"), "Khác");
}
