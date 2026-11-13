//! `librarian-bg.exe` – tác vụ quét ngầm cho Task Scheduler (E8-01).
//! Subsystem `windows`: không hiện cửa sổ console. Chỉ ghi DB + log; không xoá/di chuyển sách
//! (trừ khi bật "tự dọn trùng chắc chắn" – E8-05, mặc định tắt – hoặc "tự phân loại hộp thư đến" `auto_triage`, mặc định bật).
//!
//! Tham số: `--home <dir>` (tuỳ chọn). Mã thoát: 0 xong, 2 đang có tiến trình quét khác, 1 lỗi.
#![cfg_attr(windows, windows_subsystem = "windows")]

use librarian_core::progress::{self, Watchdog};
use librarian_core::trash::SystemTrasher;
use librarian_core::{config, lock, report, Error, Library};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut home = None;
    while let Some(a) = args.next() {
        if a == "--home" {
            home = args.next().map(PathBuf::from);
        }
    }
    let home = home.unwrap_or_else(config::default_home);
    // Chạy trước app sau khi cập nhật: chuyển dữ liệu cũ ở %APPDATA% sang thư mục cài, kẻo tạo DB trống.
    let moved = config::migrate_legacy_home(&home);
    let _guard = librarian_core::logging::init(&home, false);
    if let Err(e) = moved {
        tracing::warn!("chuyển dữ liệu cũ sang {} lỗi: {e}", home.display());
    }
    let code = match run(&home) {
        Ok(()) => 0,
        Err(Error::Locked) => {
            tracing::info!("quét ngầm: app đang quét – thoát êm");
            2
        }
        Err(e) => {
            tracing::error!("quét ngầm lỗi: {e}");
            1
        }
    };
    std::process::exit(code);
}

fn run(home: &std::path::Path) -> librarian_core::Result<()> {
    let _lock = lock::ScanLock::try_acquire(home)?;
    let lib = Library::open_recovering(home)?;
    if let Some(n) = &lib.notice {
        tracing::warn!("{n}");
    }
    tracing::info!("quét ngầm bắt đầu");
    // Giám sát treo như trong app: quá 60 giây không có tiến triển thì huỷ và ghi lỗi.
    let cancel = Arc::new(AtomicBool::new(false));
    let watchdog = Watchdog::start(cancel.clone(), progress::STALL_LIMIT, |_| {});
    let on = |s: librarian_core::Step| watchdog.observe(s);
    let res =
        report::run_analysis_with(&lib.conn, &lib.config, "background", Some(&cancel), Some(&SystemTrasher), Some(&on));
    if let Some(stall) = watchdog.finish() {
        return Err(Error::msg(stall.to_string()));
    }
    let st = res?;
    if let Some(r) = report::latest(&lib.conn)? {
        if let Some(b) = report::banner(&r) {
            tracing::info!("quét ngầm xong: {b}");
        }
    }
    // Sao lưu DB mỗi ngày (E10-06), giữ 7 bản.
    match librarian_core::backup::backup_if_due(&lib.conn, home, librarian_core::backup::DAILY) {
        Ok(Some(p)) => tracing::info!("đã sao lưu DB: {}", p.display()),
        Ok(None) => {}
        Err(e) => tracing::warn!("sao lưu DB lỗi: {e}"),
    }
    tracing::info!(
        "sách: {}, trùng chắc chắn: {}, nghi trùng: {}, chờ phân loại: {}, bỏ qua: {:?}",
        st.total_books,
        st.exact_files,
        st.suspect_groups,
        st.inbox_books,
        st.scan.skipped_roots
    );
    Ok(())
}
