//! Kiểu lỗi chung của lõi.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Lỗi I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("Lỗi cơ sở dữ liệu: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("Lỗi JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Chế độ chỉ đọc đang bật: không thay đổi file sách nào")]
    ReadOnly,
    #[error("Một tiến trình quét khác đang chạy")]
    Locked,
    #[error("Chưa đặt thư mục thư viện (books)")]
    NoLibrary,
    #[error("{0}")]
    Msg(String),
}

impl Error {
    pub fn msg(s: impl Into<String>) -> Self {
        Error::Msg(s.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
