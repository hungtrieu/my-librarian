//! M7: file hỏng / DRM hiện trong mục "Không đọc được" (E10-04), không làm hỏng lần quét.

use librarian_core::browse::{self, Item, Query};
use librarian_core::scan::{self, ScanOptions};
use librarian_core::{testdata, Library};

#[test]
fn broken_files_are_listed_not_fatal() {
    let tmp = tempfile::tempdir().unwrap();
    let sample = testdata::generate_sample(tmp.path()).unwrap();
    let broken = sample.books.join("Văn học").join("Hỏng.epub");
    std::fs::write(&broken, b"day khong phai epub").unwrap();
    std::fs::write(sample.books.join("Văn học").join("Rỗng.pdf"), b"").unwrap();
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&sample.books).unwrap();
    let s = scan::scan(&lib.conn, &lib.config, &ScanOptions::default()).unwrap();
    assert_eq!(s.files_seen, sample.book_files + 2);

    let page = browse::query(&lib.conn, &Query { problems: true, limit: 100, ..Default::default() }).unwrap();
    let names: Vec<(String, Option<String>)> = page
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Book(b) => Some((b.file_name.clone(), b.error.clone())),
            Item::Series(_) => None,
        })
        .collect();
    assert!(names.iter().any(|(n, e)| n == "Hỏng.epub" && e.is_some()), "{names:?}");
    assert!(names.iter().all(|(_, e)| e.is_some()), "chỉ sách có lỗi/DRM: {names:?}");
    let f = browse::facets(&lib.conn).unwrap();
    assert_eq!(f.problems as usize, names.len());
    // Sách hỏng vẫn tìm thấy trong thư viện như bình thường.
    let all = browse::query(&lib.conn, &Query { q: Some("hong".into()), ..Default::default() }).unwrap();
    assert_eq!(all.total, 1);
}
