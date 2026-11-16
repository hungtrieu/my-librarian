//! Chặn lỗi "quét đứng yên không báo gì" (xem `librarian_core::progress`):
//! - mọi bước của một lần quét đầy đủ phải báo tiến trình với tên bước đã biết;
//! - vòng lặp chậm (mỗi mục chậm nhưng vẫn chạy) không bị coi là treo – tức là có nhịp theo từng mục;
//! - một mục đứng yên quá giới hạn thì bộ giám sát phát hiện, chỉ đúng bước + mục, và quét dừng lại.
//!
//! File test riêng = tiến trình riêng: đặt `LIBRARIAN_HOME` không ảnh hưởng test khác, và không có test
//! song song nào gọi `pulse()` làm nhiễu bộ đếm.

use librarian_core::progress::{phase_label, Stall, Watch, Watchdog, SUSPEND_GAP};
use librarian_core::trash::Trasher;
use librarian_core::{report, Library, Step};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// Các test dưới đều dựa vào bộ đếm nhịp toàn cục: chạy lần lượt để nhịp của test này không che treo của test kia.
static SERIAL: Mutex<()> = Mutex::new(());

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
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
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
    assert!(!st.scan.cancelled);
    // Chặn thư mục "Thơ" (đã có sách sau phân loại): xoá từng sách chậm vẫn có nhịp, bước dọn thư mục có tên.
    let mut cfg = lib.config.clone();
    let rel = librarian_core::blocked::add(&mut cfg, "Thơ").unwrap();
    let b = librarian_core::blocked::enforce(&lib.conn, &cfg, &slow, Some(&rel), Some(&cancel), Some(&on)).unwrap();
    assert!(b.trashed > 0 && b.failures.is_empty(), "{b:?}");
    assert!(wd.finish().is_none(), "bước chậm nhưng vẫn chạy không được coi là treo");
    let seen = phases.into_inner();
    for p in [
        "count",
        "scan",
        "triage",
        "triage_read",
        "triage_clean",
        "hash_quick",
        "hash_full",
        "analyze",
        "block",
        "block_clean",
    ] {
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

/// Thùng rác kẹt mãi ở lần gọi đầu (như `IFileOperation` trên ổ USB rớt kết nối): chỉ trả về khi test mở cổng.
struct StuckTrash {
    gate: Mutex<mpsc::Receiver<()>>,
    calls: RefCell<usize>,
}
impl Trasher for StuckTrash {
    fn trash(&self, _path: &Path) -> Result<(), String> {
        *self.calls.borrow_mut() += 1;
        let _ = self.gate.lock().unwrap().recv();
        Err("bỏ dở".into())
    }
}

/// Lệnh kẹt không bao giờ trả về: cờ huỷ vô dụng (luồng quét không tới chỗ kiểm tra kế tiếp), nên bộ giám sát
/// phải tự báo treo qua `on_stall` – trong khoảng giới hạn, khi luồng quét VẪN đang kẹt – để giao diện thoát
/// khỏi trạng thái "đang quét" mà không chờ `finish()`.
#[test]
fn stall_is_reported_while_the_hung_call_never_returns() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("LIBRARIAN_HOME", tmp.path().join("home"));
    let lib = library(tmp.path());
    let (open_gate, gate) = mpsc::channel::<()>();
    let (stall_tx, stall_rx) = mpsc::channel::<Stall>();
    let cancel = Arc::new(AtomicBool::new(false));
    let limit = Duration::from_millis(400);
    let wd = Arc::new(Watchdog::start_with(
        cancel.clone(),
        limit,
        |_| {},
        move |s| {
            let _ = stall_tx.send(s.clone());
        },
    ));
    let returned = Arc::new(AtomicBool::new(false));
    let worker = {
        let (wd, cancel, returned) = (wd.clone(), cancel.clone(), returned.clone());
        std::thread::spawn(move || {
            let stuck = StuckTrash { gate: Mutex::new(gate), calls: RefCell::new(0) };
            let on = |s: Step| wd.observe(s);
            let r = librarian_core::triage::run(
                &lib.config,
                &stuck,
                &librarian_core::triage::TriageOptions {
                    cancel: Some(&cancel),
                    progress: Some(&on),
                    conn: Some(&lib.conn),
                    ..Default::default()
                },
            );
            returned.store(true, Ordering::SeqCst);
            let calls = *stuck.calls.borrow();
            (r.map(|t| t.cancelled), calls)
        })
    };

    let stall = match stall_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(s) => s,
        Err(e) => {
            panic!("phải báo treo khi lệnh kẹt mãi ({e}); luồng quét đã trả về: {}", returned.load(Ordering::SeqCst))
        }
    };
    assert!(!returned.load(Ordering::SeqCst), "báo treo phải tới trước khi luồng quét trả về");
    assert!(cancel.load(Ordering::SeqCst));
    assert_eq!(stall.phase, "triage_clean", "{stall:?}");
    assert!(stall.detail.as_deref().is_some_and(|d| d.contains("Chưa phân loại")), "{stall:?}");
    assert!(stall.idle_secs < 5, "phát hiện quanh giới hạn, không phải muộn: {stall:?}");
    // chỉ báo một lần dù luồng quét còn kẹt thêm
    std::thread::sleep(limit * 2);
    assert!(stall_rx.try_recv().is_err(), "on_stall chỉ gọi một lần");
    assert!(wd.stalled().is_some());

    // mở cổng: lệnh kẹt trả về, luồng quét thấy cờ huỷ và dừng ngay
    open_gate.send(()).unwrap();
    let (cancelled, calls) = worker.join().unwrap();
    assert!(cancelled.unwrap(), "phải dừng vì cờ huỷ");
    assert_eq!(calls, 1);
    let wd = Arc::try_unwrap(wd).ok().expect("luồng quét đã nhả bộ giám sát");
    assert!(wd.finish().is_some());
}

/// Máy ngủ (Modern Standby đóng băng cả tiến trình, kể cả luồng giám sát, còn `Instant` vẫn đếm): khoảng ngủ
/// không được tính là đứng yên. Trước đây: ngủ 23 phút giữa bước phân loại → thức dậy báo "treo 1408 giây"
/// và huỷ một lần quét đang chạy tốt.
#[test]
fn system_sleep_is_not_a_stall() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let t0 = Instant::now();
    let mut w = Watch::new(t0);
    let p = librarian_core::progress::pulses();
    let tick = Duration::from_millis(200);
    let mut t = t0;
    // 10 s đứng yên thật trước khi ngủ, đọc đều 200 ms/lần
    while t < t0 + Duration::from_secs(10) {
        t += tick;
        w.tick(p, t);
    }
    assert_eq!(w.tick(p, t), Duration::from_secs(10));
    // luồng giám sát bị đóng băng 1408 s
    t += Duration::from_secs(1408);
    let idle = w.tick(p, t);
    assert!(idle <= Duration::from_secs(10), "khoảng ngủ bị tính là đứng yên: {idle:?}");
    assert_eq!(w.suspended, Duration::from_secs(1408));
    // sau khi thức dậy vẫn đếm tiếp bình thường → treo thật vẫn bị bắt
    t += Duration::from_secs(1);
    assert!(w.tick(p, t) > idle);
    // trễ lịch nhỏ (dưới SUSPEND_GAP) vẫn tính là đứng yên như cũ
    let before = w.tick(p, t);
    t += SUSPEND_GAP - Duration::from_millis(1);
    assert_eq!(w.tick(p, t), before + SUSPEND_GAP - Duration::from_millis(1));
}
