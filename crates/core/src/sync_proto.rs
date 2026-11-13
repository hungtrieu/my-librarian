//! Kiểu dữ liệu giao thức đồng bộ `v1` giữa app máy tính và app iPad (docs/BACKLOG-IPAD.md, mục 3.2).
//! Dùng chung hai bên nên không phụ thuộc feature `desktop`.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_PORT: u16 = 47470;

/// `GET /api/v1/hello` (không cần token).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hello {
    pub name: String,
    pub library_id: String,
    pub protocol: u32,
    /// Ổ sách đang cắm (thư mục `books` truy cập được) – I3-09.
    pub drive_connected: bool,
}

/// `POST /api/v1/pair`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    /// Tên iPad, hiện trong danh sách thiết bị đã ghép.
    #[serde(default)]
    pub device_name: String,
    /// `ipad` | `iphone` (bản cũ không gửi → `ipad`). iPhone chỉ nhận sách chữ.
    #[serde(default)]
    pub device_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairResponse {
    pub token: String,
    pub library_id: String,
}

/// Một cuốn trong hàng chờ (`GET /api/v1/outbox`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutboxItem {
    pub id: i64,
    pub title: String,
    pub authors: String,
    pub series: Option<String>,
    pub volume: Option<i64>,
    pub format: String,
    pub size: i64,
    /// `None` khi đang tính hash nền (I3-04): iPad đợi rồi hỏi lại.
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outbox {
    pub library_id: String,
    pub drive_connected: bool,
    pub items: Vec<OutboxItem>,
}

/// `POST /api/v1/outbox/ack`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckRequest {
    pub book_id: i64,
    pub sha256: String,
}

/// `POST /api/v1/status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusRequest {
    pub free_bytes: i64,
    /// `id` (phía máy tính) của các cuốn đang có trên iPad.
    #[serde(default)]
    pub books: Vec<i64>,
}

/// Cờ "không đáng đọc" của một cuốn (`id` phía máy tính). `changed_at`: lúc đổi lần cuối (giây Unix,
/// `0` = chưa từng đổi). Hai bên giữ bản có `changed_at` lớn hơn ("ai đổi sau thắng").
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorthMark {
    pub book_id: i64,
    pub not_worth: bool,
    #[serde(default)]
    pub changed_at: i64,
}

/// `POST /api/v1/marks`: thiết bị gửi cờ của mọi cuốn nó đang có; máy tính gộp rồi trả trạng thái
/// cuối cùng của đúng các cuốn đó. Máy tính bản cũ không có đường này (404) → thiết bị bỏ qua.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarksRequest {
    #[serde(default)]
    pub marks: Vec<WorthMark>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarksResponse {
    #[serde(default)]
    pub marks: Vec<WorthMark>,
}

/// Nội dung mã QR ghép nối (I3-02).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairQr {
    pub v: u32,
    pub addrs: Vec<String>,
    pub port: u16,
    pub code: String,
    pub library_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
}
