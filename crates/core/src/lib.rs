//! My Librarian – lõi quản lý kho ebook (không phụ thuộc UI).
//!
//! Luồng chính: [`scan::scan`] → [`hash::ensure_hashes`] → [`dedupe`] / [`classify`] → [`trash`] / [`moves`].
//! [`report::run_analysis`] gói cả chuỗi cho tác vụ quét ngầm (chỉ đọc với file sách).

#[cfg(feature = "desktop")]
pub mod backup;
#[cfg(feature = "desktop")]
pub mod bench;
pub mod browse;
#[cfg(feature = "desktop")]
pub mod classify;
pub mod config;
pub mod covers;
pub mod db;
#[cfg(feature = "desktop")]
pub mod dedupe;
pub mod error;
pub mod formats;
#[cfg(feature = "desktop")]
pub mod hash;
pub mod ipad_lib;
#[cfg(feature = "desktop")]
pub mod lock;
#[cfg(feature = "desktop")]
pub mod logging;
#[cfg(feature = "desktop")]
pub mod moves;
pub mod normalize;
#[cfg(feature = "desktop")]
pub mod pdf_repair;
pub mod progress;
pub mod reader;
#[cfg(feature = "desktop")]
pub mod report;
#[cfg(feature = "desktop")]
pub mod scan;
pub mod search;
#[cfg(feature = "desktop")]
pub mod sync;
pub mod sync_client;
pub mod sync_proto;
#[cfg(feature = "desktop")]
pub mod testdata;
#[cfg(feature = "desktop")]
pub mod trash;
#[cfg(feature = "desktop")]
pub mod triage;

pub use config::Config;
pub use error::{Error, Result};
pub use rusqlite;

use std::path::{Path, PathBuf};

/// Tiến trình một bước dài (quét / phân loại / băm): đã xong `done` trên `total` mục (`total = 0`: chưa biết tổng).
/// Mỗi mục xong là đã được ghi lại ngay (file đã chuyển, băm đã lưu DB), nên tắt máy giữa chừng thì lần sau
/// chỉ còn phải làm phần chưa xong.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Step {
    /// `scan` · `recover` · `triage_read` · `triage` · `triage_clean` · `hash_quick` · `hash_full` · `analyze`.
    pub phase: &'static str,
    pub done: usize,
    pub total: usize,
}

/// Hàm nhận tiến trình.
pub type OnStep<'a> = &'a dyn Fn(Step);

/// Thư viện đã mở: thư mục dữ liệu app + cấu hình + kết nối DB.
pub struct Library {
    pub home: PathBuf,
    pub config: Config,
    pub conn: rusqlite::Connection,
    /// Thông báo cho người dùng khi mở (vd. DB hỏng đã được khôi phục từ bản sao lưu).
    pub notice: Option<String>,
}

impl Library {
    pub fn open(home: &Path) -> Result<Library> {
        std::fs::create_dir_all(home)?;
        let config = Config::load(home)?;
        let conn = db::open(&home.join("library.db"))?;
        Ok(Library { home: home.to_path_buf(), config, conn, notice: None })
    }

    #[cfg(feature = "desktop")]
    /// Như [`Library::open`] nhưng tự khôi phục DB hỏng từ bản sao lưu (E10-06). Dùng ở điểm
    /// khởi động của app / CLI / quét ngầm, không dùng cho các kết nối phụ.
    pub fn open_recovering(home: &Path) -> Result<Library> {
        std::fs::create_dir_all(home)?;
        let config = Config::load(home)?;
        let (conn, notice) = backup::open_or_recover(home)?;
        Ok(Library { home: home.to_path_buf(), config, conn, notice })
    }

    pub fn save_config(&self) -> Result<()> {
        self.config.save(&self.home)
    }

    /// Đặt thư mục thư viện `books` (E1-01).
    pub fn set_library_root(&mut self, root: &Path) -> Result<()> {
        if !root.is_dir() {
            return Err(Error::msg(format!("{} không phải thư mục", root.display())));
        }
        self.config.library_root = Some(root.to_path_buf());
        self.save_config()?;
        db::ensure_root(&self.conn, &root.to_string_lossy(), "library")?;
        Ok(())
    }
}
