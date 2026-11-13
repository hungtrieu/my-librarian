//! Ghi log xoay vòng hằng ngày vào `<home>/logs` (E0-03), giữ 14 file.

use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Khởi tạo logging. Giữ `WorkerGuard` sống tới hết chương trình để log được ghi hết.
pub fn init(home: &Path, also_stderr: bool) -> Option<WorkerGuard> {
    let dir = home.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("librarian")
        .filename_suffix("log")
        .max_log_files(14)
        .build(&dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("LIBRARIAN_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let file_layer = fmt::layer().with_ansi(false).with_writer(writer);
    let reg = tracing_subscriber::registry().with(filter).with(file_layer);
    let res = if also_stderr {
        reg.with(
            fmt::layer().with_writer(std::io::stderr).with_target(false).compact().with_filter(EnvFilter::new("warn")),
        )
        .try_init()
    } else {
        reg.try_init()
    };
    let _ = res;
    Some(guard)
}
