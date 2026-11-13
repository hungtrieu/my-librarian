//! Hai kết nối cùng ghi một DB (WAL): luồng quét và một luồng nền khác (vd. cập nhật số liệu báo cáo lúc mở app,
//! giao diện đánh dấu đã đọc…) không được làm lần quét lỗi "database is locked".

use librarian_core::db;
use librarian_core::scan::{self, ScanOptions};
use librarian_core::{testdata, Config};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Giao dịch của lõi đọc trước rồi mới ghi. Ở WAL, nếu kết nối khác ghi xen vào giữa thì giao dịch DEFERRED không
/// nâng lên ghi được (SQLITE_BUSY ngay, không chờ busy_timeout). Giao dịch ghi của lõi phải giữ chỗ ghi từ đầu.
#[test]
fn write_tx_survives_a_concurrent_writer_between_read_and_write() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("library.db");
    let a = db::open(&path).unwrap();
    let b = db::open(&path).unwrap();

    let tx = db::write_tx(&a).unwrap();
    let n: i64 = tx.query_row("SELECT COUNT(*) FROM roots", [], |r| r.get(0)).unwrap();
    // Kết nối kia ghi trong lúc giao dịch đang mở: phải chờ (busy_timeout), không được xen vào.
    let writer = std::thread::spawn(move || db::ensure_root(&b, "/other", "source").map(|_| ()));
    std::thread::sleep(std::time::Duration::from_millis(200));
    db::ensure_root(&tx, "/lib", "library").unwrap();
    tx.commit().unwrap();
    writer.join().unwrap().unwrap();
    let after: i64 = a.query_row("SELECT COUNT(*) FROM roots", [], |r| r.get(0)).unwrap();
    assert_eq!(after, n + 2);
}

/// Như lỗi thật: quét thư viện trong khi một kết nối khác liên tục ghi ngắn (như `refresh_counts` băm nhanh từng
/// cuốn nghi trùng ngay lúc mở app).
#[test]
fn scan_succeeds_while_another_connection_keeps_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let books = tmp.path().join("books");
    std::fs::create_dir_all(&books).unwrap();
    for i in 0..400 {
        let p = books.join(format!("Tac gia {} - Sach {i}.pdf", i % 17));
        testdata::write_pdf(&p, Some(&format!("Sach {i}")), Some("Tac gia"), &"x".repeat(i)).unwrap();
    }
    let cfg = Config { library_root: Some(books.clone()), ..Default::default() };
    let dbp = home.join("library.db");
    let conn = db::open(&dbp).unwrap();
    db::ensure_root(&conn, &books.to_string_lossy(), "library").unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let other = {
        let (stop, dbp) = (stop.clone(), dbp.clone());
        std::thread::spawn(move || {
            let c = db::open(&dbp).unwrap();
            let mut i = 0u64;
            while !stop.load(Ordering::Relaxed) {
                c.execute("UPDATE books SET updated_at = updated_at WHERE id = (SELECT MIN(id) FROM books)", [])
                    .unwrap();
                db::ensure_root(&c, &format!("/elsewhere/{}", i % 3), "source").unwrap();
                i += 1;
            }
        })
    };
    for _ in 0..3 {
        let s = scan::scan(&conn, &cfg, &ScanOptions::default()).expect("quét không được lỗi vì DB bị khoá");
        assert!(s.files_seen > 0);
    }
    stop.store(true, Ordering::Relaxed);
    other.join().unwrap();
}
