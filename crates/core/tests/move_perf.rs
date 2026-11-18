//! Đo thời gian chuyển sách giữa các thể loại (chạy tay: `cargo test --release -p librarian-core --test move_perf -- --ignored --nocapture`).

use librarian_core::moves::{self, MoveRequest};
use librarian_core::trash::Trasher;
use librarian_core::{bench, report, Library};
use std::path::Path;
use std::time::Instant;

struct NoTrash;
impl Trasher for NoTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        let _ = std::fs::remove_dir_all(path);
        let _ = std::fs::remove_file(path);
        Ok(())
    }
}

#[test]
#[ignore]
fn move_many_books_between_categories() {
    let n: usize = std::env::var("BOOKS").ok().and_then(|s| s.parse().ok()).unwrap_or(6000);
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("LIBRARIAN_HOME", tmp.path().join("home"));
    let books = tmp.path().join("books");
    bench::generate(&books, n).unwrap();
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&books).unwrap();
    lib.config.min_free_gb = 0;
    lib.config.auto_triage = false;
    let t = Instant::now();
    report::run_analysis(&lib.conn, &lib.config, "app", None, Some(&NoTrash)).unwrap();
    eprintln!("analysis {n} sách: {:?}", t.elapsed());
    let ids: Vec<(i64, String)> = {
        let mut st =
            lib.conn.prepare("SELECT id, category FROM books WHERE status='present' AND category = 'Văn học'").unwrap();
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().flatten().collect();
        v
    };
    let target = books.join("Lịch sử");
    for k in [20usize, 2000] {
        let reqs: Vec<MoveRequest> =
            ids.iter().take(k).map(|(id, _)| MoveRequest { book_id: *id, target_dir: target.clone() }).collect();
        let t = Instant::now();
        let r = moves::apply_moves(&lib.conn, &lib.config, &reqs, "manual").unwrap();
        let mv = t.elapsed();
        let t = Instant::now();
        report::refresh_counts(&lib.conn, &lib.config).unwrap();
        let rc = t.elapsed();
        eprintln!("chuyển {} quyển: apply_moves {mv:?}, refresh_counts {rc:?}", r.moved.len());
        let t = Instant::now();
        librarian_core::dedupe::exact_groups(&lib.conn).unwrap();
        eprintln!("  exact_groups {:?}", t.elapsed());
        let t = Instant::now();
        librarian_core::dedupe::suspect_groups(&lib.conn).unwrap();
        eprintln!("  suspect_groups {:?}", t.elapsed());
        let t = Instant::now();
        librarian_core::classify::propose(&lib.conn, &lib.config).unwrap();
        eprintln!("  propose {:?}", t.elapsed());
        let t = Instant::now();
        moves::undo_batch(&lib.conn, &lib.config, r.batch_id).unwrap();
        eprintln!("  hoàn tác: {:?}", t.elapsed());
    }
}
