//! Kiểm thử trình đọc (M5): vol trước/sau trong bộ, trang CBZ, CBR dạng RAR (giải nén vào cache),
//! bìa + số trang của RAR, vị trí đọc.

use librarian_core::formats::{self, Format};
use librarian_core::reader::{self, ComicSource, Engine};
use librarian_core::scan::{self, ScanOptions};
use librarian_core::{covers, db, testdata, Library};
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rar-comic.cbr")
}

/// Màu đỏ của điểm ảnh đầu tiên – fixture có trang "Page 1/2/10.png" với R = 20/40/200.
fn red(png: &[u8]) -> u8 {
    image::load_from_memory(png).unwrap().to_rgb8().get_pixel(0, 0)[0]
}

#[test]
fn series_neighbours_and_position() {
    let tmp = tempfile::tempdir().unwrap();
    let sample = testdata::generate_sample(tmp.path()).unwrap();
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&sample.books).unwrap();
    scan::scan(&lib.conn, &lib.config, &ScanOptions::default()).unwrap();
    let id_of =
        |name: &str| -> i64 { db::books_where(&lib.conn, "file_name = ?1", [name]).unwrap().first().unwrap().id };
    let v2 = id_of("Doraemon vol 2.cbz");
    let info = reader::reader_info(&lib.conn, v2).unwrap();
    assert_eq!(info.engine, Engine::Comic);
    assert_eq!(info.series_pos, Some((2, 4)));
    assert_eq!(info.prev.unwrap().id, id_of("Doraemon vol 1.cbz"));
    assert_eq!(info.next.unwrap().id, id_of("Doraemon vol 3.cbr"));
    let last = reader::reader_info(&lib.conn, id_of("Doraemon vol 10.cbz")).unwrap();
    assert!(last.next.is_none());

    // Truyện CBZ: trang theo thứ tự, đọc từng trang.
    let b = db::get_book(&lib.conn, v2).unwrap().unwrap();
    let src = reader::open_comic(Path::new(&b.path), b.size, b.mtime, &tmp.path().join("cache")).unwrap();
    assert!(matches!(src, ComicSource::Zip { .. }));
    assert_eq!(src.page_count(), 3);
    let (bytes, mime) = src.page(0).unwrap();
    assert_eq!(mime, "image/png");
    assert!(bytes.starts_with(b"\x89PNG"));
    assert!(src.page(3).is_err());

    // Vị trí đọc: lưu → hiện trong "Đang đọc".
    assert_eq!(reader::save_position(&lib.conn, v2, Some("page:1"), 0.5).unwrap(), "reading");
    let b = db::get_book(&lib.conn, v2).unwrap().unwrap();
    assert_eq!(b.read_status, "reading");
    assert_eq!(reader::reader_info(&lib.conn, v2).unwrap().locator.as_deref(), Some("page:1"));
    // Sách không còn trên đĩa thì không mở.
    lib.conn.execute("UPDATE books SET status = 'missing' WHERE id = ?1", [v2]).unwrap();
    assert!(reader::reader_info(&lib.conn, v2).is_err());
}

#[test]
fn rar_comic_extracts_in_natural_order_and_caches() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("cache");
    let path = fixture();
    // Lần đầu: giải nén dần ở luồng nền, trang đọc được ngay.
    let first = reader::open_comic(&path, 1, 2, &cache).unwrap();
    assert!(matches!(first, ComicSource::Rar(_)), "lần đầu không chờ giải nén hết");
    assert_eq!(first.page_count(), 3, "readme.txt không phải trang");
    let reds: Vec<u8> = (0..3).map(|i| red(&first.page(i).unwrap().0)).collect();
    assert_eq!(reds, vec![20, 40, 200], "Page 1, Page 2, Page 10");
    first.wait_extracted();
    // Giải nén xong: mở lại dùng thư mục cache.
    let src = reader::open_comic(&path, 1, 2, &cache).unwrap();
    let ComicSource::Dir { dir, pages } = &src else { panic!("RAR phải giải nén vào cache") };
    assert_eq!(pages.len(), 3);
    let reds: Vec<u8> = (0..3).map(|i| red(&src.page(i).unwrap().0)).collect();
    assert_eq!(reds, vec![20, 40, 200]);
    // Mở lại: dùng cache, không giải nén lại.
    let marker = dir.join(&pages[0]);
    let before = std::fs::metadata(&marker).unwrap().modified().unwrap();
    let again = reader::open_comic(&path, 1, 2, &cache).unwrap();
    assert_eq!(again.page_count(), 3);
    assert_eq!(std::fs::metadata(&marker).unwrap().modified().unwrap(), before);
    // File đổi (mtime khác) → khoá cache khác.
    let other = reader::open_comic(&path, 1, 3, &cache).unwrap();
    other.wait_extracted();
    let other = reader::open_comic(&path, 1, 3, &cache).unwrap();
    let ComicSource::Dir { dir: dir2, .. } = &other else { panic!() };
    assert_ne!(dir, dir2);
    // Dọn cache: chỉ giữ 1.
    reader::prune_cache(&cache, 1);
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
}

#[test]
fn rar_cover_and_page_count() {
    let path = fixture();
    let jpg = covers::extract_cover(&path, Format::Cbr).expect("bìa RAR");
    assert_eq!(red(&jpg), 20);
    let md = formats::extract(&path, Format::Cbr).unwrap();
    assert_eq!(md.page_count, Some(3));
}

#[test]
fn not_a_comic_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("x.cbr");
    std::fs::write(&p, b"hello world").unwrap();
    assert!(reader::open_comic(&p, 1, 1, tmp.path()).is_err());
}

#[test]
fn open_from_explorer_inside_and_outside_library() {
    let tmp = tempfile::tempdir().unwrap();
    let sample = testdata::generate_sample(tmp.path()).unwrap();
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&sample.books).unwrap();
    scan::scan(&lib.conn, &lib.config, &ScanOptions::default()).unwrap();
    let b = db::books_where(&lib.conn, "file_name = 'Doraemon vol 2.cbz'", []).unwrap().remove(0);
    // Explorer có thể đưa đường dẫn khác hoa thường / dấu gạch.
    let odd = b.path.to_uppercase().replace('\\', "/");
    assert_eq!(reader::find_by_path(&lib.conn, Path::new(&odd)).unwrap(), Some(b.id));

    let outside = tmp.path().join("Tải về").join("Sách lạ.epub");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::copy(sample.books.join("Văn học").join("Mắt Biếc.epub"), &outside).unwrap();
    assert_eq!(reader::find_by_path(&lib.conn, &outside).unwrap(), None);
    let info = reader::external_info(-1, &outside).unwrap();
    assert_eq!(info.engine, Engine::Foliate);
    assert_eq!(info.book.title, "Mắt Biếc");
    assert!(info.next.is_none() && info.locator.is_none());
    assert!(reader::external_info(-2, &tmp.path().join("không có.epub")).is_err());
}
