//! Chặn lỗi "quét đứng yên không báo gì" (xem `librarian_core::progress`):
//! - mọi bước của một lần quét đầy đủ phải báo tiến trình với tên bước đã biết;
//! - vòng lặp chậm (mỗi mục chậm nhưng vẫn chạy) không bị coi là treo – tức là có nhịp theo từng mục;
//! - một mục đứng yên quá giới hạn thì bộ giám sát phát hiện, chỉ đúng bước + mục, và quét dừng lại.
//!
//! File test riêng = tiến trình riêng: đặt `LIBRARIAN_HOME` không ảnh hưởng test khác, và không có test
//! song song nào gọi `pulse()` làm nhiễu bộ đếm.

use librarian_core::progress::{phase_label, Watchdog};
use librarian_core::trash::Trasher;
use librarian_core::{report, Library, Step};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

/// Thùng rác chậm như ổ USB: mỗi lần mất `delay`; lần thứ `hang_at` (nếu có) đứng yên `hang`.
struct SlowTrash {
    dir: PathBuf,
    delay: Duration,
    hang_at: Option<usize>,
    hang: Duration,
    calls: RefCell<usize>,
}
impl Trasher for SlowTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        let n = {
            let mut c = self.calls.borrow_mut();
            *c += 1;
            *c
        };
        std::thread::sleep(if self.hang_at == Some(n) { self.hang } else { self.delay });
        let dst = self.dir.join(format!("{n}-{}", path.file_name().unwrap().to_string_lossy()));
        std::fs::rename(path, dst).map_err(|e| e.to_string())
    }
}

fn write(p: &Path, body: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Thư viện có đủ việc cho mọi bước: hộp thư đến kiểu Calibre (đọc → phân loại → dọn thư mục rỗng),
/// sách trùng (băm nhanh → băm đầy đủ), rồi phân tích.
fn library(tmp: &Path) -> Library {
    let root = tmp.join("books");
    std::fs::create_dir_all(root.join("Thơ")).unwrap();
    let inbox = root.join("Chưa phân loại").join("Calibre Library");
    for i in 0..12 {
        let d = inbox.join(format!("Tác giả {i}/Tập thơ {i} ({i})"));
        write(&d.join(format!("Tập thơ {i} - Tác giả {i}.epub")), format!("epub {i}").as_bytes());
        write(&d.join("cover.jpg"), b"img");
        write(
            &d.join("metadata.opf"),
            format!(
                "<package><metadata><dc:title>Tập thơ {i}</dc:title><dc:subject>Thơ</dc:subject></metadata></package>"
            )
            .as_bytes(),
        );
    }
    // Thư mục rác riêng lẻ (không sách) → mỗi cái là một mục dọn.
    for k in 0..6 {
        write(&root.join(format!("Chưa phân loại/Rác {k}/anh.jpg")), b"img");
    }
    write(&root.join("Thơ/Bản A.txt"), b"cung noi dung");
    write(&root.join("Thơ/Bản B.txt"), b"cung noi dung");
    let mut lib = Library::open(&tmp.join("home")).unwrap();
    lib.set_library_root(&root).unwrap();
    lib.config.auto_triage = true;
    lib
}

#[test]
fn every_phase_is_reported_and_stalls_are_caught() {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("LIBRARIAN_HOME", tmp.path().join("home")); // nhật ký phân loại ghi vào đây

    // 1) Chạy đủ các bước, mỗi lần vào Thùng rác chậm 40 ms (cả bước dọn ~0,5 s > giới hạn 300 ms):
    //    không được báo treo, và mọi bước đều có tên.
    let lib = library(tmp.path());
    let trash_dir = tmp.path().join("trash");
    std::fs::create_dir_all(&trash_dir).unwrap();
    let slow = SlowTrash {
        dir: trash_dir.clone(),
        delay: Duration::from_millis(40),
        hang_at: None,
        hang: Duration::ZERO,
        calls: RefCell::new(0),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let wd = Watchdog::start(cancel.clone(), Duration::from_millis(300), |_| {});
    let phases = RefCell::new(BTreeSet::new());
    let on = |s: Step| {
        phases.borrow_mut().insert(s.phase);
        wd.observe(s);
    };
    // auto_clean = None: thùng rác chậm chỉ dùng cho bước phân loại (tham số `trasher` của triage)
    let triage = librarian_core::triage::run(
        &lib.config,
        &slow,
        &librarian_core::triage::TriageOptions {
            journal: Some(&tmp.path().join("home/triage-journal.jsonl")),
            cancel: Some(&cancel),
            progress: Some(&on),
            conn: Some(&lib.conn),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(triage.dirs_trashed > 0, "phải có thư mục rỗng để dọn: {triage:?}");
    let st = report::run_analysis_with(&lib.conn, &lib.config, "app", Some(&cancel), None, Some(&on)).unwrap();
    assert!(wd.finish().is_none(), "bước chậm nhưng vẫn chạy không được coi là treo");
    assert!(!st.scan.cancelled);
    let seen = phases.into_inner();
    for p in ["count", "scan", "triage", "triage_read", "triage_clean", "hash_quick", "hash_full", "analyze"] {
        assert!(seen.contains(p), "bước {p} không báo tiến trình; đã thấy {seen:?}");
    }
    for p in &seen {
        assert_ne!(phase_label(p), "Bước không rõ", "bước {p} chưa có tên trong progress::phase_label");
    }

    // 2) Một mục đứng yên quá giới hạn → phát hiện treo đúng bước, chỉ ra mục, và quét dừng lại.
    let tmp2 = tempfile::tempdir().unwrap();
    std::env::set_var("LIBRARIAN_HOME", tmp2.path().join("home"));
    let lib2 = library(tmp2.path());
    let trash2 = tmp2.path().join("trash");
    std::fs::create_dir_all(&trash2).unwrap();
    let hanging = SlowTrash {
        dir: trash2,
        delay: Duration::from_millis(5),
        hang_at: Some(1),
        hang: Duration::from_millis(1500),
        calls: RefCell::new(0),
    };
    let cancel2 = Arc::new(AtomicBool::new(false));
    let wd2 = Watchdog::start(cancel2.clone(), Duration::from_millis(400), |_| {});
    let on2 = |s: Step| wd2.observe(s);
    let r = librarian_core::triage::run(
        &lib2.config,
        &hanging,
        &librarian_core::triage::TriageOptions {
            cancel: Some(&cancel2),
            progress: Some(&on2),
            conn: Some(&lib2.conn),
            ..Default::default()
        },
    )
    .unwrap();
    let stall = wd2.finish().expect("đứng yên 1,5 s với giới hạn 0,4 s phải bị phát hiện");
    assert_eq!(stall.phase, "triage_clean");
    assert!(stall.detail.as_deref().is_some_and(|d| d.contains("Chưa phân loại")), "{stall:?}");
    assert!(r.cancelled, "phát hiện treo phải dừng bước đang chạy");
    assert_eq!(*hanging.calls.borrow(), 1, "dừng ngay sau mục bị treo, không dọn tiếp");
    assert!(stall.to_string().contains("Dọn hộp thư đến"), "{stall}");
}
