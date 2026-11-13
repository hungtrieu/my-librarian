//! Kiểm thử tích hợp lõi (E10-05): quét, tìm, trùng, nghi trùng, xoá vào thùng rác, phân loại,
//! di chuyển, hoàn tác, ổ rút ra, đổi tên ngoài app, quét ngầm chỉ đọc, khoá chống chạy chồng.

use librarian_core::classify::{self, Bucket};
use librarian_core::dedupe::{self, Decision};
use librarian_core::moves::{self, MoveRequest};
use librarian_core::scan::{self, ScanOptions};
use librarian_core::trash::{self, Trasher};
use librarian_core::{db, hash, lock, report, search, testdata, Config, Error, Library};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Thùng rác giả: chuyển file vào một thư mục, hoặc mô phỏng lỗi.
struct FakeTrash {
    dir: PathBuf,
    mode: RefCell<Mode>,
    calls: RefCell<usize>,
}
#[derive(Clone, Copy)]
enum Mode {
    Ok,
    /// Lỗi, file vẫn còn (bị khoá).
    FailKeep,
    /// File biến mất nhưng API báo lỗi (không chắc đã vào thùng rác).
    FailVanish,
}
impl FakeTrash {
    fn new(dir: PathBuf) -> Self {
        std::fs::create_dir_all(&dir).unwrap();
        FakeTrash { dir, mode: RefCell::new(Mode::Ok), calls: RefCell::new(0) }
    }
}
impl Trasher for FakeTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        *self.calls.borrow_mut() += 1;
        match *self.mode.borrow() {
            Mode::Ok => {
                let dst =
                    self.dir.join(format!("{}-{}", self.calls.borrow(), path.file_name().unwrap().to_string_lossy()));
                std::fs::rename(path, dst).map_err(|e| e.to_string())
            }
            Mode::FailKeep => Err("file đang được mở".into()),
            Mode::FailVanish => {
                std::fs::remove_file(path).unwrap();
                Err("lỗi lạ".into())
            }
        }
    }
}

struct Env {
    _tmp: tempfile::TempDir,
    lib: Library,
    sample: testdata::Sample,
    trash: FakeTrash,
}

fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let sample = testdata::generate_sample(tmp.path()).unwrap();
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&sample.books).unwrap();
    let trash = FakeTrash::new(tmp.path().join("trash"));
    Env { _tmp: tmp, lib, sample, trash }
}

fn scan(e: &Env) -> scan::ScanSummary {
    scan::scan(&e.lib.conn, &e.lib.config, &ScanOptions::default()).unwrap()
}

fn analyze(e: &Env) {
    scan(e);
    hash::ensure_hashes(&e.lib.conn, 2, None).unwrap();
}

/// Ảnh chụp cây thư mục: đường dẫn tương đối → (dung lượng, mtime).
fn tree(root: &Path) -> BTreeMap<String, (u64, i64)> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            let m = e.metadata().unwrap();
            (
                e.path().strip_prefix(root).unwrap().to_string_lossy().into_owned(),
                (m.len(), db::mtime_ms(m.modified().unwrap())),
            )
        })
        .collect()
}

fn titles(v: &[db::Book]) -> Vec<String> {
    v.iter().map(|b| b.title.clone()).collect()
}

#[test]
fn scan_counts_metadata_and_skips() {
    let e = setup();
    let s = scan(&e);
    assert_eq!(s.files_seen, e.sample.book_files, "lỗi: {:?}", s.errors);
    assert_eq!(s.added, e.sample.book_files);
    assert!(s.errors.is_empty(), "{:?}", s.errors);
    let c = &e.lib.conn;
    let b = scan::book_by_path(c, &e.sample.books.join("Văn học").join("Mắt Biếc.epub")).unwrap().unwrap();
    assert_eq!(b.authors, "Nguyễn Nhật Ánh");
    assert_eq!(b.isbn.as_deref(), Some("9786041000018"));
    assert_eq!(b.category.as_deref(), Some("Văn học"));
    let m = scan::book_by_path(c, &e.sample.books.join("Công nghệ").join("The Pragmatic Programmer.azw3"))
        .unwrap()
        .unwrap();
    assert_eq!(m.title, "The Pragmatic Programmer");
    assert_eq!(m.authors, "Andrew Hunt");
    assert_eq!(m.subjects, "Programming");
    let p = scan::book_by_path(c, &e.sample.books.join("Văn học").join("Truyện Kiều.pdf")).unwrap().unwrap();
    assert_eq!((p.title.as_str(), p.authors.as_str()), ("Truyện Kiều", "Nguyễn Du"));
    let f = scan::book_by_path(c, &e.sample.books.join("Văn học").join("Số Đỏ.fb2")).unwrap().unwrap();
    assert_eq!(f.authors, "Vũ Trọng Phụng");
    let seven = scan::book_by_path(c, &e.sample.books.join("truyen tranh").join("7 seeds").join("7 Seeds 2.mobi"))
        .unwrap()
        .unwrap();
    assert_eq!((seven.series.as_deref(), seven.volume), (Some("7 seeds"), Some(2)));
    let d = scan::book_by_path(c, &e.sample.books.join("truyen tranh").join("Doraemon").join("Doraemon vol 10.cbz"))
        .unwrap()
        .unwrap();
    assert_eq!((d.volume, d.page_count), (Some(10), Some(3)));
    // Đường dẫn dài và PDF không metadata → fallback tên file.
    assert!(e.sample.long_path.to_string_lossy().chars().count() > 260);
    assert!(scan::book_by_path(c, &e.sample.long_path).unwrap().is_some());
    let mb = scan::book_by_path(c, &e.sample.inbox.join("Mat Biec.pdf")).unwrap().unwrap();
    assert_eq!(mb.title, "Mat Biec");
    assert!(mb.in_inbox);
    // Thư mục ẩn / $RECYCLE.BIN / file không phải sách không được quét.
    let all: Vec<String> = db::books_where(c, "1", []).unwrap().into_iter().map(|b| b.path).collect();
    assert!(all.iter().all(|p| !p.contains(".hidden") && !p.contains("$RECYCLE.BIN") && !p.ends_with(".docx")));
}

#[test]
fn incremental_rescan_and_search() {
    let e = setup();
    scan(&e);
    let s2 = scan(&e);
    assert_eq!((s2.added, s2.updated, s2.unchanged), (0, 0, e.sample.book_files));
    let c = &e.lib.conn;
    let r = search::search(c, "mat bi", 10).unwrap();
    assert!(titles(&r).iter().any(|t| t == "Mắt Biếc"), "{:?}", titles(&r));
    assert!(titles(&search::search(c, "MẮT BIẾC", 10).unwrap()).contains(&"Mắt Biếc".to_string()));
    let r = search::search(c, "nguyen nhat", 10).unwrap();
    assert!(r.len() >= 3, "tìm theo tác giả");
    let r = search::search(c, "dora", 20).unwrap();
    assert!(r.len() >= 4, "tìm theo tiền tố tên bộ/tên file");
    assert!(search::search(c, "khong co sach nay", 10).unwrap().is_empty());
    // Sửa file → quét lại chỉ xử lý file đó.
    let p = e.sample.books.join("Văn học").join("Số Đỏ.fb2");
    testdata::write_fb2(&p, "Số Đỏ (bản sửa)", "Vũ Trọng", "Phụng", "x").unwrap();
    let s3 = scan(&e);
    assert_eq!(s3.updated, 1);
}

#[test]
fn series_grouping_natural_order() {
    let e = setup();
    scan(&e);
    let series = search::list_series(&e.lib.conn).unwrap();
    let dora = series.iter().find(|s| s.series == "Doraemon").unwrap();
    assert_eq!(dora.volumes, 4);
    let vols = search::series_volumes(&e.lib.conn, Some("truyen tranh"), "Doraemon").unwrap();
    let names: Vec<_> = vols.iter().map(|b| b.file_name.as_str()).collect();
    assert_eq!(names, vec!["Doraemon vol 1.cbz", "Doraemon vol 2.cbz", "Doraemon vol 3.cbr", "Doraemon vol 10.cbz"]);
}

#[test]
fn exact_duplicates_cleaned_to_trash_keeper_kept() {
    let e = setup();
    analyze(&e);
    let c = &e.lib.conn;
    let groups = dedupe::exact_groups(c).unwrap();
    assert_eq!(groups.len(), 1, "{:?}", groups.iter().map(|g| &g.keeper.path).collect::<Vec<_>>());
    let g = &groups[0];
    assert!(g.keeper.path.ends_with("Văn học\\Mắt Biếc.epub") || g.keeper.path.ends_with("Văn học/Mắt Biếc.epub"));
    assert_eq!(g.keep_reason, "đã nằm trong thư mục thể loại của thư viện");
    assert_eq!(g.others.len(), 1);
    let rep = dedupe::clean_exact(c, &e.lib.config, &e.trash, &groups, &HashMap::new()).unwrap();
    assert_eq!((rep.trashed, rep.failures.len()), (1, 0));
    assert!(!e.sample.inbox.join("Mắt Biếc (1).epub").exists());
    assert!(e.sample.books.join("Văn học").join("Mắt Biếc.epub").exists());
    assert!(dedupe::exact_groups(c).unwrap().is_empty());
    // Nhật ký có ghi lại.
    let batches = moves::list_batches(c, 10).unwrap();
    assert_eq!((batches[0].kind.as_str(), batches[0].done), ("dedupe", 1));
}

#[test]
fn keeper_changed_blocks_deletion() {
    let e = setup();
    analyze(&e);
    let c = &e.lib.conn;
    let groups = dedupe::exact_groups(c).unwrap();
    // Bản giữ bị sửa sau khi băm.
    std::fs::write(&groups[0].keeper.path, b"bi sua").unwrap();
    let rep = dedupe::clean_exact(c, &e.lib.config, &e.trash, &groups, &HashMap::new()).unwrap();
    assert_eq!(rep.trashed, 0);
    assert_eq!(rep.failures.len(), 1);
    assert!(e.sample.inbox.join("Mắt Biếc (1).epub").exists(), "bản thừa phải còn nguyên");
}

#[test]
fn trash_errors_locked_continue_vanished_abort() {
    let e = setup();
    scan(&e);
    let c = &e.lib.conn;
    let ids: Vec<i64> =
        db::books_where(c, "in_inbox = 1 ORDER BY id", []).unwrap().iter().map(|b| b.id).take(3).collect();
    *e.trash.mode.borrow_mut() = Mode::FailKeep;
    let rep = trash::trash_books(c, &e.lib.config, &e.trash, &ids).unwrap();
    assert_eq!((rep.trashed, rep.failures.len(), rep.aborted.is_none()), (0, 3, true), "khoá file: xử lý tiếp cả lô");
    *e.trash.mode.borrow_mut() = Mode::FailVanish;
    let rep = trash::trash_books(c, &e.lib.config, &e.trash, &ids).unwrap();
    assert!(rep.aborted.is_some());
    assert_eq!(rep.failures.len(), 1, "dừng ngay ở file đầu tiên");
    for id in &ids[1..] {
        assert!(Path::new(&db::get_book(c, *id).unwrap().unwrap().path).exists());
    }
}

#[test]
fn read_only_blocks_destructive_ops() {
    let mut e = setup();
    analyze(&e);
    e.lib.config.read_only = true;
    let c = &e.lib.conn;
    let groups = dedupe::exact_groups(c).unwrap();
    assert!(matches!(dedupe::clean_exact(c, &e.lib.config, &e.trash, &groups, &HashMap::new()), Err(Error::ReadOnly)));
    let any = db::books_where(c, "in_inbox = 1 LIMIT 1", []).unwrap()[0].id;
    assert!(matches!(trash::trash_books(c, &e.lib.config, &e.trash, &[any]), Err(Error::ReadOnly)));
    let req = MoveRequest { book_id: any, target_dir: e.sample.books.join("Văn học") };
    assert!(matches!(moves::apply_moves(c, &e.lib.config, &[req], "classify"), Err(Error::ReadOnly)));
}

#[test]
fn suspects_found_reviewed_and_remembered() {
    let e = setup();
    analyze(&e);
    let c = &e.lib.conn;
    let groups = dedupe::suspect_groups(c).unwrap();
    // Một tác phẩm nhiều định dạng (Mat Biec.pdf + Mắt Biếc.epub) không phải nghi trùng.
    assert!(
        !groups.iter().any(|g| g.members.iter().any(|b| b.file_name == "Mat Biec.pdf")),
        "khác định dạng thì không tính là trùng"
    );
    let mb = groups
        .iter()
        .find(|g| g.members.iter().any(|b| b.file_name == "Truyen Kieu - ban in.pdf"))
        .expect("Truyen Kieu - ban in.pdf nghi trùng với Truyện Kiều.pdf");
    assert!(mb.members.iter().any(|b| b.file_name == "Truyện Kiều.pdf"));
    assert!(mb.members.iter().all(|b| b.format == "pdf"));
    // Không gộp các tập khác nhau.
    for g in &groups {
        let vols: std::collections::HashSet<_> =
            g.members.iter().filter(|b| b.series.is_some()).map(|b| b.volume).collect();
        assert!(vols.len() <= 1, "gộp nhầm tập: {:?}", g.members.iter().map(|b| &b.file_name).collect::<Vec<_>>());
    }
    let ids: Vec<i64> = mb.members.iter().map(|b| b.id).collect();
    dedupe::decide(c, &ids, Decision::NotDuplicate).unwrap();
    let still = |c| {
        dedupe::suspect_groups(c)
            .unwrap()
            .iter()
            .any(|g| g.members.iter().any(|b| b.file_name == "Truyen Kieu - ban in.pdf"))
    };
    assert!(!still(c));
    // Quét lại + đổi tên file → vẫn nhớ quyết định.
    std::fs::rename(e.sample.inbox.join("Truyen Kieu - ban in.pdf"), e.sample.inbox.join("Truyện Kiều - bản scan.pdf"))
        .unwrap();
    analyze(&e);
    let again = dedupe::suspect_groups(c).unwrap();
    assert!(
        !again.iter().any(|g| g.members.iter().any(|b| b.file_name.contains("bản scan"))),
        "đã duyệt thì không hỏi lại"
    );
}

#[test]
fn classify_preview_apply_undo() {
    let e = setup();
    analyze(&e);
    let c = &e.lib.conn;
    let cats = classify::list_categories(&e.lib.config).unwrap();
    let names: Vec<_> = cats.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["Công nghệ", "Tiểu sử - Hồi ký", "Văn học", "truyen tranh"]);
    let props = classify::propose(c, &e.lib.config).unwrap();
    let by = |name: &str| props.iter().find(|p| p.book.file_name == name).unwrap_or_else(|| panic!("{name}"));
    let p = by("7 Seeds 3.mobi");
    assert_eq!(
        (p.bucket, p.target.as_deref()),
        (Bucket::High, Some(e.sample.books.join("truyen tranh").join("7 seeds").as_path()))
    );
    let p = by("Doraemon Tập 45.cbz");
    assert_eq!(p.target.as_deref(), Some(e.sample.books.join("truyen tranh").join("Doraemon").as_path()));
    let p = by("Ngày xưa có một chuyện tình.epub");
    assert_eq!(p.target_label.as_deref(), Some("Văn học"), "{}", p.reason);
    assert_eq!(p.bucket, Bucket::High);
    let p = by("Refactoring.epub");
    assert_eq!(p.target_label.as_deref(), Some("Công nghệ"), "{} {}", p.reason, p.confidence);
    let p = by("One Piece 1.cbz");
    assert_eq!((p.bucket, p.target.is_none()), (Bucket::Manual, true));
    assert!(p.suggestion.as_deref().unwrap().contains("one piece"));
    let p = by("Không rõ.txt");
    assert_eq!(p.bucket, Bucket::Manual);
    let p = by("Mắt Biếc (1).epub");
    assert_eq!(
        (p.bucket, p.target.is_none()),
        (Bucket::Manual, true),
        "bản trùng chắc chắn không bị đề xuất di chuyển"
    );
    // Hộp thư đến không bao giờ là đích.
    assert!(props.iter().all(|p| p.target.as_ref().is_none_or(|t| !t.starts_with(&e.sample.inbox))));

    // Không file nào bị di chuyển trước khi Áp dụng.
    let before = tree(&e.sample.books);
    // Tạo sẵn file trùng tên ở đích → phải thêm (1), không ghi đè.
    std::fs::write(e.sample.books.join("Công nghệ").join("Refactoring.epub"), b"khac").unwrap();
    let before_with_conflict = tree(&e.sample.books);
    assert_eq!(before.len() + 1, before_with_conflict.len());

    let reqs: Vec<MoveRequest> = props
        .iter()
        .filter(|p| p.target.is_some())
        .map(|p| MoveRequest { book_id: p.book.id, target_dir: p.target.clone().unwrap() })
        .collect();
    let rep = moves::apply_moves(c, &e.lib.config, &reqs, "classify").unwrap();
    assert!(rep.failures.is_empty(), "{:?}", rep.failures);
    assert_eq!(rep.moved.len(), reqs.len());
    assert!(e.sample.books.join("truyen tranh").join("7 seeds").join("7 Seeds 3.mobi").exists());
    assert!(e.sample.books.join("Công nghệ").join("Refactoring (1).epub").exists());
    assert_eq!(std::fs::read(e.sample.books.join("Công nghệ").join("Refactoring.epub")).unwrap(), b"khac");
    let moved = db::get_book(c, by("7 Seeds 3.mobi").book.id).unwrap().unwrap();
    assert_eq!(
        (moved.category.as_deref(), moved.series.as_deref(), moved.volume, moved.in_inbox),
        (Some("truyen tranh"), Some("7 seeds"), Some(3), false)
    );
    // Quét lại sau di chuyển: không có gì mới/mất.
    let s = scan(&e);
    assert_eq!((s.added, s.missing), (1, 0), "chỉ file xung đột tạo tay là mới");

    let u = moves::undo_batch(c, &e.lib.config, rep.batch_id).unwrap();
    assert!(u.failures.is_empty(), "{:?}", u.failures);
    assert_eq!(u.restored, reqs.len());
    assert_eq!(tree(&e.sample.books), before_with_conflict, "hoàn tác → cây thư mục y như ban đầu");
    assert!(moves::undo_batch(c, &e.lib.config, rep.batch_id).is_err(), "không hoàn tác hai lần");
    let s = scan(&e);
    assert_eq!((s.added, s.missing), (0, 0));
}

#[test]
fn leave_one_out_evaluation_runs() {
    let e = setup();
    scan(&e);
    let ev = classify::evaluate(&e.lib.conn, &e.lib.config).unwrap();
    assert!(ev.total >= 10);
    assert!(ev.correct > 0);
    assert!(ev.accuracy() > 0.5, "{ev:?}");
}

#[test]
fn unplugged_drive_marks_nothing_missing() {
    let e = setup();
    scan(&e);
    let moved_away = e.sample.books.with_file_name("books-unplugged");
    std::fs::rename(&e.sample.books, &moved_away).unwrap();
    let s = scan(&e);
    assert_eq!(s.skipped_roots.len(), 1);
    assert_eq!(s.missing, 0);
    let n: i64 = e.lib.conn.query_row("SELECT COUNT(*) FROM books WHERE status = 'missing'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    let st = report::run_analysis(&e.lib.conn, &e.lib.config, "background", None, None).unwrap();
    let banner = report::banner(&report::latest(&e.lib.conn).unwrap().unwrap()).unwrap();
    assert!(banner.contains("ổ chưa kết nối"), "{banner}");
    assert_eq!(st.missing_books, 0);
    std::fs::rename(&moved_away, &e.sample.books).unwrap();
}

#[test]
fn external_rename_keeps_record() {
    let e = setup();
    scan(&e);
    let c = &e.lib.conn;
    let old = e.sample.books.join("Văn học").join("Tôi thấy hoa vàng trên cỏ xanh.epub");
    let b = scan::book_by_path(c, &old).unwrap().unwrap();
    c.execute("UPDATE books SET read_status = 'reading', progress = 0.42 WHERE id = ?1", [b.id]).unwrap();
    let new = e.sample.books.join("Tiểu sử - Hồi ký").join("Hoa vàng cỏ xanh.epub");
    std::fs::rename(&old, &new).unwrap();
    let s = scan(&e);
    assert_eq!((s.recovered, s.missing), (1, 0));
    let nb = scan::book_by_path(c, &new).unwrap().unwrap();
    assert_eq!(nb.id, b.id, "giữ nguyên bản ghi");
    assert_eq!(nb.progress, Some(0.42), "giữ tiến độ đọc");
    assert_eq!(nb.category.as_deref(), Some("Tiểu sử - Hồi ký"));
}

#[test]
fn background_analysis_triages_inbox_by_default() {
    let e = setup();
    assert!(e.lib.config.auto_triage, "mặc định bật");
    let fake = FakeTrash::new(e._tmp.path().join("bg-trash"));
    let st = report::run_analysis(&e.lib.conn, &e.lib.config, "background", None, Some(&fake)).unwrap();
    assert!(st.auto_triaged.unwrap_or(0) > 0, "sách trong hộp thư đến được tự xếp");
    let inbox = e.sample.books.join("Chưa phân loại");
    let left: Vec<_> = std::fs::read_dir(&inbox)
        .unwrap()
        .flatten()
        .filter(|f| !matches!(f.path().extension().and_then(|x| x.to_str()), Some("cbz" | "cbr")))
        .collect();
    assert!(left.is_empty(), "chỉ còn truyện tranh: {left:?}");
    assert!(e.sample.books.join("Khác").is_dir(), "sách chưa chắc vào Khác");
}

#[test]
fn background_analysis_is_read_only() {
    let e = setup();
    let before = tree(&e.sample.books);
    let fake = FakeTrash::new(e._tmp.path().join("bg-trash"));
    // Hợp đồng "chỉ đọc" áp dụng khi tắt tự phân loại (mặc định BẬT, xem test bên dưới).
    let cfg = Config { auto_triage: false, ..e.lib.config.clone() };
    let st = report::run_analysis(&e.lib.conn, &cfg, "background", None, Some(&fake)).unwrap();
    assert_eq!(tree(&e.sample.books), before, "quét ngầm không thay đổi file sách nào");
    assert_eq!(*fake.calls.borrow(), 0, "E8-05 mặc định tắt");
    assert_eq!((st.exact_groups, st.exact_files), (1, 1));
    assert!(st.suspect_groups >= 1);
    assert!(st.inbox_books >= 7);
    let banner = report::banner(&report::latest(&e.lib.conn).unwrap().unwrap()).unwrap();
    assert!(banner.contains("trùng chắc chắn") && banner.contains("chờ phân loại"), "{banner}");
}

#[test]
fn counts_drop_after_cleaning_and_reviewing() {
    // Badge "Trùng chắc chắn" / "Nghi trùng" và banner đọc số từ báo cáo gần nhất: dọn/duyệt xong phải giảm ngay,
    // không đợi lần quét sau.
    let e = setup();
    let (c, cfg) = (&e.lib.conn, &e.lib.config);
    let st = report::run_analysis(c, cfg, "manual", None, None).unwrap();
    assert_eq!(st.exact_files, 1);
    let stats = |c| report::latest(c).unwrap().unwrap().stats.unwrap();

    let groups = dedupe::exact_groups(c).unwrap();
    dedupe::clean_exact(c, cfg, &e.trash, &groups, &HashMap::new()).unwrap();
    report::refresh_counts(c, cfg).unwrap();
    let after_clean = stats(c);
    assert_eq!((after_clean.exact_groups, after_clean.exact_files, after_clean.reclaimable_bytes), (0, 0, 0));
    let banner = report::banner(&report::latest(c).unwrap().unwrap()).unwrap();
    assert!(!banner.contains("trùng chắc chắn"), "{banner}");

    let suspects = dedupe::suspect_groups(c).unwrap();
    assert!(!suspects.is_empty() && after_clean.suspect_groups == suspects.len());
    let ids: Vec<i64> = suspects[0].members.iter().map(|b| b.id).collect();
    dedupe::decide(c, &ids, Decision::KeepBoth).unwrap();
    report::refresh_counts(c, cfg).unwrap();
    assert_eq!(stats(c).suspect_groups, suspects.len() - 1, "duyệt một nhóm nghi trùng thì số giảm 1");

    // Giữ một bản, xoá các bản còn lại của nhóm nghi trùng kế tiếp → cũng giảm.
    let rest = dedupe::suspect_groups(c).unwrap();
    if let Some(g) = rest.first() {
        let drop: Vec<i64> = g.members.iter().skip(1).map(|b| b.id).collect();
        trash::trash_books(c, cfg, &e.trash, &drop).unwrap();
        report::refresh_counts(c, cfg).unwrap();
        assert!(stats(c).suspect_groups < rest.len());
    }
}

#[test]
fn stale_counts_fixed_without_new_action() {
    // Đã dọn ở bản cũ (báo cáo còn số cũ), nay chỉ mở app / mở màn: số phải về đúng mà không cần thao tác gì.
    let e = setup();
    let (c, cfg) = (&e.lib.conn, &e.lib.config);
    report::run_analysis(c, cfg, "manual", None, None).unwrap();
    let groups = dedupe::exact_groups(c).unwrap();
    dedupe::clean_exact(c, cfg, &e.trash, &groups, &HashMap::new()).unwrap();
    let stats = |c| report::latest(c).unwrap().unwrap().stats.unwrap();
    assert_eq!(stats(c).exact_files, 1, "chưa cập nhật = tình trạng của bản cũ");

    // Mở màn "Trùng chắc chắn".
    assert!(report::sync_exact(c, &dedupe::exact_groups(c).unwrap()).unwrap());
    assert_eq!(stats(c).exact_files, 0);
    assert!(!report::sync_exact(c, &[]).unwrap(), "không đổi thì không ghi");
    // Mở màn "Nghi trùng".
    let n = dedupe::suspect_groups(c).unwrap().len();
    report::sync_suspects(c, n + 5).unwrap();
    assert!(report::sync_suspects(c, n).unwrap());
    assert_eq!(stats(c).suspect_groups, n);
}

#[test]
fn auto_clean_only_when_enabled() {
    let mut e = setup();
    e.lib.config.auto_clean_exact_in_background = true;
    let fake = FakeTrash::new(e._tmp.path().join("bg-trash"));
    let st = report::run_analysis(&e.lib.conn, &e.lib.config, "background", None, Some(&fake)).unwrap();
    assert_eq!(st.auto_cleaned, Some(1));
    assert!(!e.sample.inbox.join("Mắt Biếc (1).epub").exists());
}

#[test]
fn scan_lock_is_exclusive() {
    let tmp = tempfile::tempdir().unwrap();
    let a = lock::ScanLock::try_acquire(tmp.path()).unwrap();
    assert!(matches!(lock::ScanLock::try_acquire(tmp.path()), Err(Error::Locked)));
    drop(a);
    assert!(lock::ScanLock::try_acquire(tmp.path()).is_ok());
}

#[test]
fn config_roundtrip_defaults() {
    let tmp = tempfile::tempdir().unwrap();
    let c = Config::load(tmp.path()).unwrap();
    assert!(!c.auto_clean_exact_in_background && !c.read_only);
    assert_eq!(c.inbox_name, "Chưa phân loại");
    let mut c2 = c.clone();
    c2.classify_high = 80;
    c2.save(tmp.path()).unwrap();
    assert_eq!(Config::load(tmp.path()).unwrap().classify_high, 80);
}

#[test]
fn covers_extracted_and_cached() {
    use librarian_core::covers::CoverCache;
    use librarian_core::formats::Format;
    let e = setup();
    scan(&e);
    let cache = CoverCache::new(&e.lib.home);
    let check = |rel: &[&str], expect: bool| {
        let p = rel.iter().fold(e.sample.books.clone(), |p, s| p.join(s));
        let b = scan::book_by_path(&e.lib.conn, &p).unwrap().unwrap();
        let fmt = Format::parse(&b.format).unwrap();
        let got = cache.get(b.id, &p, fmt, b.size, b.mtime).unwrap();
        assert_eq!(got.is_some(), expect, "{rel:?}");
        if let Some(j) = got {
            assert!(j.starts_with(&[0xFF, 0xD8]), "thumbnail là JPEG");
            // Lần 2 đọc từ cache.
            assert_eq!(cache.get(b.id, &p, fmt, b.size, b.mtime).unwrap().unwrap(), j);
        }
    };
    check(&["Văn học", "Mắt Biếc.epub"], true);
    check(&["Công nghệ", "The Pragmatic Programmer.azw3"], true);
    check(&["truyen tranh", "Doraemon", "Doraemon vol 2.cbz"], true);
    check(&["truyen tranh", "Doraemon", "Doraemon vol 3.cbr"], true);
    check(&["Văn học", "Truyện Kiều.pdf"], false);
    check(&["Văn học", "Truyện Kiều.pdf"], false);
}

#[test]
fn browse_filters_sorts_and_groups_series() {
    use librarian_core::browse::{self, Item, Query, Sort};
    let e = setup();
    scan(&e);
    let c = &e.lib.conn;
    let all = browse::query(c, &Query { group_series: true, limit: 500, ..Default::default() }).unwrap();
    let series: Vec<_> =
        all.items.iter().filter_map(|i| if let Item::Series(s) = i { Some(s) } else { None }).collect();
    let dora = series.iter().find(|s| s.series == "Doraemon").expect("Doraemon là một ô");
    assert_eq!(dora.volumes, 4);
    assert!(all.items.iter().all(|i| !matches!(i, Item::Book(b) if b.series.as_deref() == Some("Doraemon"))));
    let flat = browse::query(c, &Query { limit: 500, ..Default::default() }).unwrap();
    assert_eq!(flat.total, e.sample.book_files);
    assert!(all.total < flat.total);
    // Lọc theo thể loại (bỏ dấu) + định dạng.
    let vh = browse::query(
        c,
        &Query { category: Some("van hoc".into()), format: Some("epub".into()), limit: 50, ..Default::default() },
    )
    .unwrap();
    assert!(
        vh.total >= 2
            && vh
                .items
                .iter()
                .all(|i| matches!(i, Item::Book(b) if b.format == "epub" && b.category.as_deref() == Some("Văn học")))
    );
    // Sắp xếp theo tên, phân trang.
    let t = browse::query(c, &Query { sort: Sort::Title, limit: 3, offset: 1, ..Default::default() }).unwrap();
    assert_eq!((t.items.len(), t.offset), (3, 1));
    // Tìm + lọc tác giả.
    let r = browse::query(
        c,
        &Query { q: Some("mat".into()), author: Some("Nguyễn Nhật Ánh".into()), limit: 50, ..Default::default() },
    )
    .unwrap();
    assert!(r.total >= 1 && r.items.iter().all(|i| matches!(i, Item::Book(b) if b.authors.contains("Nhật Ánh"))));
    // Chi tiết bộ theo thứ tự tự nhiên.
    let s = browse::query(c, &Query { series: Some("Doraemon".into()), limit: 50, ..Default::default() }).unwrap();
    let names: Vec<_> =
        s.items.iter().map(|i| if let Item::Book(b) = i { b.file_name.clone() } else { String::new() }).collect();
    assert_eq!(names, vec!["Doraemon vol 1.cbz", "Doraemon vol 2.cbz", "Doraemon vol 3.cbr", "Doraemon vol 10.cbz"]);
    // Trạng thái đọc + "Đọc tiếp".
    let ids: Vec<i64> = s.items.iter().filter_map(|i| if let Item::Book(b) = i { Some(b.id) } else { None }).collect();
    browse::set_read_status(c, &ids[..2], "read").unwrap();
    let all = browse::query(c, &Query { group_series: true, limit: 500, ..Default::default() }).unwrap();
    let dora = all
        .items
        .iter()
        .find_map(|i| if let Item::Series(s) = i { (s.series == "Doraemon").then_some(s.clone()) } else { None })
        .unwrap();
    assert_eq!((dora.read, dora.continue_book_id), (2, Some(ids[2])));
    let f = browse::facets(c).unwrap();
    assert_eq!(f.total as usize, e.sample.book_files);
}

/// Phân loại hộp thư đến bị dừng giữa chừng: phần đã chuyển giữ nguyên và DB cập nhật ngay (không thành sách
/// "mất"), lần sau chỉ chạy phần còn lại; tiến trình báo đúng "x / tổng".
#[test]
fn triage_resumes_after_cancel_and_updates_db() {
    use librarian_core::triage::{self, TriageOptions};
    use std::sync::atomic::{AtomicBool, Ordering};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("books");
    let inbox = root.join("Chưa phân loại");
    for i in 0..60 {
        let d = inbox.join(format!("Tác giả {i}/Vụ án số {i}"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("Vụ án số {i}.epub")), format!("nội dung {i}")).unwrap();
        std::fs::write(d.join("metadata.opf"), "<dc:subject>Trinh thám</dc:subject>").unwrap();
    }
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&root).unwrap();
    lib.config.auto_triage = false;
    scan::scan(&lib.conn, &lib.config, &ScanOptions::default()).unwrap();
    let trash = FakeTrash::new(tmp.path().join("trash"));

    let cancel = AtomicBool::new(false);
    let seen = RefCell::new(Vec::new());
    let on = |s: librarian_core::Step| {
        if s.phase != "triage" {
            return; // bỏ qua bước đọc hộp thư đến (`triage_read`)
        }
        seen.borrow_mut().push((s.done, s.total));
        if s.done >= 25 {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let opts =
        TriageOptions { cancel: Some(&cancel), progress: Some(&on), conn: Some(&lib.conn), ..Default::default() };
    let r1 = triage::run(&lib.config, &trash, &opts).unwrap();
    assert!(r1.cancelled);
    assert!(r1.units >= 25 && r1.units < 60, "{}", r1.units);
    assert!(seen.borrow().contains(&(0, 60)));
    // Đã chuyển xong → DB trỏ chỗ mới, vẫn "present", không còn trong hộp thư đến.
    let (moved, in_inbox): (i64, i64) = lib
        .conn
        .query_row(
            "SELECT SUM(category = 'Trinh thám - Hình sự'), SUM(in_inbox) FROM books WHERE status = 'present'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(moved as usize, r1.units);
    assert_eq!(in_inbox as usize, 60 - r1.units);

    // Lần sau: chỉ còn phần chưa làm.
    seen.borrow_mut().clear();
    let cancel2 = AtomicBool::new(false);
    let on2 = |s: librarian_core::Step| {
        if s.phase == "triage" {
            seen.borrow_mut().push((s.done, s.total));
        }
    };
    let opts2 =
        TriageOptions { cancel: Some(&cancel2), progress: Some(&on2), conn: Some(&lib.conn), ..Default::default() };
    let r2 = triage::run(&lib.config, &trash, &opts2).unwrap();
    assert!(!r2.cancelled);
    assert_eq!(r2.units, 60 - r1.units);
    assert_eq!(seen.borrow().last(), Some(&(60 - r1.units, 60 - r1.units)));
    let s = scan::scan(&lib.conn, &lib.config, &ScanOptions::default()).unwrap();
    assert_eq!((s.added, s.missing, s.unchanged), (0, 0, 60), "quét lại không phải đọc lại sách nào");
}

/// Băm bị huỷ giữa chừng: phần đã băm được lưu, lần sau chỉ băm phần còn lại.
#[test]
fn hashing_is_saved_per_batch_and_resumes() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("books");
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..200 {
        // cùng dung lượng
        std::fs::write(root.join(format!("Sách {i}.txt")), format!("{i:08}")).unwrap();
    }
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&root).unwrap();
    scan::scan(&lib.conn, &lib.config, &ScanOptions::default()).unwrap();
    let cancel = AtomicBool::new(false);
    let on = |s: librarian_core::Step| {
        if s.phase == "hash_quick" && s.done > 0 {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let h1 = hash::ensure_hashes_with(&lib.conn, 1, Some(&cancel), Some(&on)).unwrap();
    assert!(h1.cancelled);
    assert!(h1.quick_hashed > 0 && h1.quick_hashed < 200, "{}", h1.quick_hashed);
    let saved: i64 =
        lib.conn.query_row("SELECT COUNT(*) FROM books WHERE quick_hash IS NOT NULL", [], |r| r.get(0)).unwrap();
    assert_eq!(saved as usize, h1.quick_hashed);
    let totals = RefCell::new(Vec::new());
    let on2 = |s: librarian_core::Step| totals.borrow_mut().push((s.phase, s.total));
    let h2 = hash::ensure_hashes_with(&lib.conn, 1, None, Some(&on2)).unwrap();
    assert_eq!(h2.quick_hashed, 200 - h1.quick_hashed);
    assert!(totals.borrow().contains(&("hash_quick", 200 - h1.quick_hashed)));
}

#[test]
fn suspect_hashing_reports_progress_and_resumes_after_cancel() {
    use std::sync::atomic::AtomicBool;
    let e = setup();
    analyze(&e);
    let c = &e.lib.conn;
    let steps = RefCell::new(Vec::<librarian_core::Step>::new());
    let on = |s: librarian_core::Step| steps.borrow_mut().push(s);
    let phase_steps = |steps: &RefCell<Vec<librarian_core::Step>>| -> Vec<(usize, usize)> {
        steps.borrow().iter().filter(|s| s.phase == dedupe::SUSPECT_HASH_PHASE).map(|s| (s.done, s.total)).collect()
    };

    // Huỷ ngay: không ra kết quả, nhưng đã báo bước con kèm tổng x / y.
    let cancel = AtomicBool::new(true);
    assert!(dedupe::suspect_groups_with(c, 2, Some(&cancel), Some(&on)).unwrap().is_none());
    let first = phase_steps(&steps);
    assert!(first.first().is_some_and(|&(d, t)| d == 0 && t >= 2), "{first:?}");

    // Chạy tiếp: băm đủ, tiến trình đi tới total.
    steps.borrow_mut().clear();
    let groups = dedupe::suspect_groups_with(c, 2, None, Some(&on)).unwrap().unwrap();
    assert!(!groups.is_empty());
    let run = phase_steps(&steps);
    assert!(run.last().is_some_and(|&(d, t)| d == t && t >= 2), "{run:?}");

    // Lần sau: không còn gì để băm lại.
    steps.borrow_mut().clear();
    let again = dedupe::suspect_groups_with(c, 2, None, Some(&on)).unwrap().unwrap();
    assert_eq!(again.len(), groups.len());
    assert!(phase_steps(&steps).iter().all(|&(_, t)| t == 0), "{:?}", phase_steps(&steps));
}

#[test]
fn second_full_run_redoes_no_finished_work() {
    // Các bước đã xong (phân loại, quét, băm nhanh, băm đầy đủ, băm bổ sung nghi trùng) được lưu lại:
    // chạy lần hai không làm lại việc nào.
    let e = setup();
    let (c, cfg) = (&e.lib.conn, &e.lib.config);
    let steps = RefCell::new(Vec::<librarian_core::Step>::new());
    let on = |s: librarian_core::Step| steps.borrow_mut().push(s);
    let first = report::run_analysis_with(c, cfg, "manual", None, Some(&e.trash), Some(&on)).unwrap();
    assert!(!first.scan.cancelled);
    assert!(steps.borrow().iter().any(|s| s.phase == "triage" && s.total > 0), "lần đầu có việc phân loại");

    steps.borrow_mut().clear();
    let second = report::run_analysis_with(c, cfg, "manual", None, Some(&e.trash), Some(&on)).unwrap();
    assert_eq!((second.scan.added, second.scan.updated), (0, 0), "quét lại chỉ thấy file không đổi");
    for phase in ["triage", "hash_quick", "hash_full", dedupe::SUSPECT_HASH_PHASE] {
        let max = steps.borrow().iter().filter(|s| s.phase == phase).map(|s| s.total).max().unwrap_or(0);
        assert_eq!(max, 0, "bước {phase} làm lại {max} mục");
    }
    assert_eq!(second.suspect_groups, first.suspect_groups);
}

#[test]
fn browse_groups_one_work_in_many_formats() {
    use librarian_core::browse::{self, Item, Query};
    let e = setup();
    let vh = e.sample.books.join("Văn học");
    testdata::write_mobi(&vh.join("Mắt Biếc.mobi"), "Mắt Biếc", "Nguyễn Nhật Ánh", None, None, "ban mobi").unwrap();
    testdata::write_pdf(&vh.join("Mắt Biếc.pdf"), Some("Mắt Biếc"), Some("Nguyễn Nhật Ánh"), "ban pdf").unwrap();
    scan(&e);
    let c = &e.lib.conn;
    let all = browse::query(c, &Query { group_series: true, limit: 500, ..Default::default() }).unwrap();
    let works: Vec<_> = all
        .items
        .iter()
        .filter_map(|i| if let Item::Series(s) = i { s.work.as_ref().map(|_| s) } else { None })
        .collect();
    let mb = works.iter().find(|s| s.series == "Mắt Biếc").expect("Mắt Biếc 3 định dạng là một ô");
    assert_eq!(mb.formats, vec!["epub", "mobi", "pdf"]);
    assert_eq!(mb.volumes, 3);
    assert_eq!(mb.category.as_deref(), Some("Văn học"));
    // Không còn ô sách lẻ trùng bìa trong Văn học; bản ở hộp thư đến (thư mục khác) vẫn là ô riêng.
    let loose: Vec<_> = all
        .items
        .iter()
        .filter_map(|i| if let Item::Book(b) = i { (b.norm_title == "mat biec").then_some(b) } else { None })
        .collect();
    assert!(loose.iter().all(|b| b.in_inbox), "{:?}", loose.iter().map(|b| &b.rel_path).collect::<Vec<_>>());
    // Mở ô: bản chính là EPUB (đọc được trong app).
    let main = browse::query(c, &Query { work: mb.work.clone(), limit: 50, ..Default::default() }).unwrap();
    let fmts: Vec<_> =
        main.items.iter().map(|i| if let Item::Book(b) = i { b.format.clone() } else { String::new() }).collect();
    assert_eq!(fmts, vec!["epub", "mobi", "pdf"]);
    assert!(matches!(&main.items[0], Item::Book(b) if b.id == mb.cover_book_id));
    // Mục "Bộ truyện" chỉ có bộ thật; lọc một định dạng thì không gộp.
    let only =
        browse::query(c, &Query { group_series: true, only_series: true, limit: 500, ..Default::default() }).unwrap();
    assert!(only.items.iter().all(|i| matches!(i, Item::Series(s) if s.work.is_none())));
    let pdf = browse::query(
        c,
        &Query {
            group_series: true,
            format: Some("pdf".into()),
            category: Some("Văn học".into()),
            limit: 50,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(pdf.items.iter().any(|i| matches!(i, Item::Book(b) if b.file_name == "Mắt Biếc.pdf")));
}
