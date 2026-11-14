//! Thư viện cục bộ trên iPad (I1-04, I1-05, I1-07, I1-08): SQLite nhỏ chỉ chứa sách đang có trên iPad.
//! Không quét, không dedupe, không phân loại (DI2). Vị trí đọc nằm ở bảng riêng khoá theo SHA-256
//! để **xoá khỏi iPad rồi gửi lại vẫn mở đúng chỗ cũ** (I1-07).

use crate::error::{Error, Result};
use crate::normalize::{fold, natural_cmp, volume_cmp};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const MIGRATIONS: &[&str] = &[
    r#"
    CREATE TABLE books (
        id INTEGER PRIMARY KEY,
        library_id TEXT,
        remote_id INTEGER,
        title TEXT NOT NULL,
        norm_title TEXT NOT NULL,
        authors TEXT NOT NULL DEFAULT '',
        norm_authors TEXT NOT NULL DEFAULT '',
        series TEXT,
        volume INTEGER,
        format TEXT NOT NULL,
        size INTEGER NOT NULL,
        sha256 TEXT NOT NULL UNIQUE,
        -- Đường dẫn tương đối trong Documents/Books (sách chép vào) hoặc bookmark (mở tại chỗ).
        location TEXT NOT NULL,
        in_place INTEGER NOT NULL DEFAULT 0,
        missing INTEGER NOT NULL DEFAULT 0,
        added_at INTEGER NOT NULL,
        read_status TEXT NOT NULL DEFAULT 'unread',
        progress REAL,
        locator TEXT,
        last_read_at INTEGER
    );
    CREATE UNIQUE INDEX books_remote ON books(library_id, remote_id) WHERE remote_id IS NOT NULL;
    CREATE INDEX books_series ON books(series);
    -- Vị trí đọc sống sót khi xoá sách khỏi iPad.
    CREATE TABLE positions (
        sha256 TEXT PRIMARY KEY,
        read_status TEXT NOT NULL,
        progress REAL,
        locator TEXT,
        last_read_at INTEGER
    );
"#,
    r#"
    -- Đánh dấu trang, khoá theo SHA-256 như `positions` (xoá sách rồi gửi lại vẫn còn).
    CREATE TABLE bookmarks (
        id INTEGER PRIMARY KEY,
        sha256 TEXT NOT NULL,
        locator TEXT,
        progress REAL NOT NULL,
        label TEXT NOT NULL DEFAULT '',
        created_at INTEGER NOT NULL
    );
    CREATE INDEX bookmarks_sha ON bookmarks(sha256, progress);
"#,
    r#"
    -- Cờ "không đáng đọc", khoá theo SHA-256 (xoá sách rồi gửi lại vẫn còn). `changed_at`: lúc đổi lần cuối,
    -- để đồng bộ hai chiều với máy tính ("ai đổi sau thắng").
    CREATE TABLE worth (
        sha256 TEXT PRIMARY KEY,
        not_worth INTEGER NOT NULL,
        changed_at INTEGER NOT NULL
    );
"#,
];

/// Định dạng sách chữ – bản iPhone chỉ tải và đọc các định dạng này (không PDF, không truyện tranh).
pub const TEXT_FORMATS: &[&str] = &["epub", "mobi", "prc", "azw", "azw3", "kf8", "fb2", "txt"];

pub fn is_text_format(format: &str) -> bool {
    TEXT_FORMATS.iter().any(|f| f.eq_ignore_ascii_case(format))
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let conn = Connection::open(path)?;
    setup(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    setup(&conn)?;
    Ok(conn)
}

fn setup(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let cur: usize = conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as usize;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(cur) {
        let tx = crate::db::write_tx(conn)?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NewBook {
    pub library_id: Option<String>,
    pub remote_id: Option<i64>,
    pub title: String,
    pub authors: String,
    pub series: Option<String>,
    pub volume: Option<i64>,
    pub format: String,
    pub size: i64,
    pub sha256: String,
    pub location: String,
    pub in_place: bool,
}

/// Vị trí đọc đã lưu: (trạng thái, tiến độ, locator, lần đọc cuối).
type SavedPosition = (String, Option<f64>, Option<String>, Option<i64>);

/// Thêm sách. Trùng SHA-256 thì không nhân đôi (trả về id cũ, `false`). Vị trí đọc cũ (nếu có) được khôi phục.
pub fn add_book(conn: &Connection, b: &NewBook) -> Result<(i64, bool)> {
    if let Some(id) = conn.query_row("SELECT id FROM books WHERE sha256 = ?1", [&b.sha256], |r| r.get(0)).optional()? {
        return Ok((id, false));
    }
    let pos: Option<SavedPosition> = conn
        .query_row(
            "SELECT read_status, progress, locator, last_read_at FROM positions WHERE sha256 = ?1",
            [&b.sha256],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let (rs, pr, loc, lr) = pos.unwrap_or(("unread".into(), None, None, None));
    conn.execute(
        "INSERT INTO books (library_id, remote_id, title, norm_title, authors, norm_authors, series, volume, format, size, sha256,
                            location, in_place, added_at, read_status, progress, locator, last_read_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            b.library_id, b.remote_id, b.title, fold(&b.title), b.authors, fold(&b.authors), b.series, b.volume, b.format, b.size,
            b.sha256, b.location, b.in_place, now(), rs, pr, loc, lr
        ],
    )?;
    Ok((conn.last_insert_rowid(), true))
}

#[derive(Debug, Clone, Serialize)]
pub struct IpadBook {
    pub id: i64,
    pub title: String,
    pub authors: String,
    pub series: Option<String>,
    pub volume: Option<i64>,
    pub format: String,
    pub size: i64,
    pub sha256: String,
    pub location: String,
    /// Tên file (phần cuối của `location`) – trình đọc dùng đuôi file để chọn cách mở.
    pub file_name: String,
    pub in_place: bool,
    pub missing: bool,
    pub added_at: i64,
    pub read_status: String,
    pub progress: Option<f64>,
    pub locator: Option<String>,
    pub last_read_at: Option<i64>,
    /// Đã đánh dấu "không đáng đọc" (ẩn khỏi thư viện, nằm ở tab riêng).
    pub not_worth: bool,
}

const COLS: &str = "id, title, authors, series, volume, format, size, sha256, location, in_place, missing, added_at, read_status, progress, locator, last_read_at, \
    COALESCE((SELECT w.not_worth FROM worth w WHERE w.sha256 = books.sha256), 0)";

fn file_name_of(location: &str, title: &str, format: &str) -> String {
    let base = location.rsplit(['/', '\\']).next().unwrap_or("");
    if base.contains('.') {
        base.to_string()
    } else {
        format!("{title}.{format}")
    }
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<IpadBook> {
    Ok(IpadBook {
        id: r.get(0)?,
        title: r.get(1)?,
        authors: r.get(2)?,
        series: r.get(3)?,
        volume: r.get(4)?,
        format: r.get(5)?,
        size: r.get(6)?,
        sha256: r.get(7)?,
        file_name: file_name_of(&r.get::<_, String>(8)?, &r.get::<_, String>(1)?, &r.get::<_, String>(5)?),
        location: r.get(8)?,
        in_place: r.get(9)?,
        missing: r.get(10)?,
        added_at: r.get(11)?,
        read_status: r.get(12)?,
        progress: r.get(13)?,
        locator: r.get(14)?,
        last_read_at: r.get(15)?,
        not_worth: r.get(16)?,
    })
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<IpadBook>> {
    Ok(conn.query_row(&format!("SELECT {COLS} FROM books WHERE id = ?1"), [id], row).optional()?)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    #[default]
    Added,
    Recent,
    Title,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Query {
    pub q: Option<String>,
    pub sort: Sort,
    /// Tab "Không đáng đọc": chỉ trả các cuốn đã đánh dấu. Mặc định các cuốn này bị ẩn.
    #[serde(default)]
    pub not_worth: bool,
}

/// Một mục trên lưới: sách lẻ hoặc ô bộ truyện.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Item {
    Book(IpadBook),
    Series(SeriesTile),
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesTile {
    pub series: String,
    pub volumes: usize,
    pub read: usize,
    pub cover_book_id: i64,
    /// Định dạng của cuốn dùng làm bìa (để giao diện biết có trích bìa được không).
    pub cover_format: String,
    /// Vol đang đọc dở, hoặc vol kế tiếp sau vol đọc xong cuối cùng ("Đọc tiếp").
    pub continue_book_id: Option<i64>,
    pub size: i64,
    pub last_read_at: Option<i64>,
    pub added_at: i64,
}

fn vol_cmp(a: &IpadBook, b: &IpadBook) -> std::cmp::Ordering {
    volume_cmp(a.volume, &a.title, b.volume, &b.title)
}

/// Các vol của một bộ theo thứ tự vol (Vol 2 trước Vol 10).
pub fn series_volumes(conn: &Connection, series: &str) -> Result<Vec<IpadBook>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM books WHERE series = ?1"))?;
    let mut v: Vec<IpadBook> = st.query_map([series], row)?.collect::<rusqlite::Result<_>>()?;
    v.sort_by(vol_cmp);
    Ok(v)
}

/// Lưới thư viện: sách lẻ + ô bộ (≥ 2 vol cùng bộ gộp thành một ô). Tìm không dấu theo tên/tác giả/bộ.
pub fn list(conn: &Connection, q: &Query) -> Result<Vec<Item>> {
    let toks: Vec<String> =
        q.q.as_deref().map(|s| fold(s).split_whitespace().map(String::from).collect()).unwrap_or_default();
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM books"))?;
    let all: Vec<IpadBook> = st.query_map([], row)?.collect::<rusqlite::Result<_>>()?;
    let matches = |b: &IpadBook| {
        let hay = format!("{} {} {}", fold(&b.title), fold(&b.authors), fold(b.series.as_deref().unwrap_or("")));
        toks.iter().all(|t| hay.contains(t.as_str()))
    };
    let mut by_series: std::collections::BTreeMap<String, Vec<IpadBook>> = Default::default();
    let mut items: Vec<Item> = Vec::new();
    for b in all.into_iter().filter(|b| b.not_worth == q.not_worth).filter(matches) {
        match b.series.clone() {
            Some(s) => by_series.entry(s).or_default().push(b),
            None => items.push(Item::Book(b)),
        }
    }
    for (name, mut vols) in by_series {
        if vols.len() == 1 {
            items.push(Item::Book(vols.remove(0)));
            continue;
        }
        vols.sort_by(vol_cmp);
        let continue_book_id = vols.iter().find(|b| b.read_status == "reading").map(|b| b.id).or_else(|| {
            let last = vols.iter().rposition(|b| b.read_status == "read")?;
            vols.get(last + 1).map(|b| b.id)
        });
        items.push(Item::Series(SeriesTile {
            series: name,
            volumes: vols.len(),
            read: vols.iter().filter(|b| b.read_status == "read").count(),
            cover_book_id: vols[0].id,
            cover_format: vols[0].format.clone(),
            continue_book_id,
            size: vols.iter().map(|b| b.size).sum(),
            last_read_at: vols.iter().filter_map(|b| b.last_read_at).max(),
            added_at: vols.iter().map(|b| b.added_at).max().unwrap_or(0),
        }));
    }
    let key = |i: &Item| match i {
        Item::Book(b) => (b.last_read_at, b.added_at, b.title.clone()),
        Item::Series(s) => (s.last_read_at, s.added_at, s.series.clone()),
    };
    match q.sort {
        Sort::Added => items.sort_by(|a, b| key(b).1.cmp(&key(a).1).then_with(|| natural_cmp(&key(a).2, &key(b).2))),
        Sort::Recent => items.sort_by(|a, b| key(b).0.cmp(&key(a).0).then_with(|| natural_cmp(&key(a).2, &key(b).2))),
        Sort::Title => items.sort_by(|a, b| natural_cmp(&key(a).2, &key(b).2)),
    }
    Ok(items)
}

/// Mục "Đang đọc" ở đầu thư viện: sách đang đọc dở, đọc gần nhất trước.
pub fn continue_reading(conn: &Connection, limit: usize) -> Result<Vec<IpadBook>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM books WHERE read_status = 'reading'
           AND NOT EXISTS (SELECT 1 FROM worth w WHERE w.sha256 = books.sha256 AND w.not_worth = 1)
         ORDER BY last_read_at DESC, id DESC LIMIT ?1"
    ))?;
    let v = st.query_map([limit as i64], row)?.collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

/// Lưu vị trí đọc (cùng định dạng CFI / `page:n` như bản Windows) và cập nhật bảng `positions`.
/// Trả về trạng thái mới: `reading` | `read` (đã đọc thì giữ `read`).
pub fn save_position(conn: &Connection, id: i64, locator: Option<&str>, progress: f64) -> Result<String> {
    let (sha, prev): (String, String) = conn
        .query_row("SELECT sha256, read_status FROM books WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| Error::msg("không có sách này"))?;
    let p = if progress.is_finite() { progress.clamp(0.0, 1.0) } else { 0.0 };
    let status = if p >= 0.995 || prev == "read" { "read" } else { "reading" };
    let t = now();
    conn.execute(
        "UPDATE books SET locator = ?2, progress = ?3, read_status = ?4, last_read_at = ?5 WHERE id = ?1",
        params![id, locator, p, status, t],
    )?;
    conn.execute(
        "INSERT INTO positions (sha256, read_status, progress, locator, last_read_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(sha256) DO UPDATE SET read_status = ?2, progress = ?3, locator = ?4, last_read_at = ?5",
        params![sha, status, p, locator, t],
    )?;
    Ok(status.to_string())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Removed {
    pub count: usize,
    pub freed_bytes: i64,
    /// `location` của các sách chép vào app – bên gọi xoá file thật (sách mở tại chỗ thì không đụng file gốc).
    pub files_to_delete: Vec<String>,
}

/// Xoá khỏi iPad (I1-07). Giữ vị trí đọc. Không đụng file gốc của sách mở tại chỗ.
pub fn remove_books(conn: &Connection, ids: &[i64]) -> Result<Removed> {
    let mut out = Removed { count: 0, freed_bytes: 0, files_to_delete: vec![] };
    let tx = crate::db::write_tx(conn)?;
    for &id in ids {
        if let Some(b) = tx
            .query_row("SELECT size, location, in_place FROM books WHERE id = ?1", [id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?))
            })
            .optional()?
        {
            tx.execute("DELETE FROM books WHERE id = ?1", [id])?;
            out.count += 1;
            if !b.2 {
                out.freed_bytes += b.0;
                out.files_to_delete.push(b.1);
            }
        }
    }
    tx.commit()?;
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct VolumeRef {
    pub id: i64,
    pub title: String,
    pub volume: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReaderInfo {
    pub book: IpadBook,
    pub engine: crate::reader::Engine,
    pub locator: Option<String>,
    pub progress: Option<f64>,
    pub prev: Option<VolumeRef>,
    pub next: Option<VolumeRef>,
    pub series_pos: Option<(usize, usize)>,
}

/// Thông tin cho trình đọc dùng chung (`reader.js`): bộ máy hiển thị, vị trí, vol trước/sau.
pub fn reader_info(conn: &Connection, id: i64) -> Result<ReaderInfo> {
    use crate::formats::Format;
    use crate::reader::Engine;
    let book = get(conn, id)?.ok_or_else(|| Error::msg("Không có sách này"))?;
    if book.missing {
        return Err(Error::msg("File sách không còn trên iPad (đã xoá hoặc không truy cập được)"));
    }
    let engine = Format::parse(&book.format).map(Engine::for_format).unwrap_or(Engine::External);
    let (mut prev, mut next, mut series_pos) = (None, None, None);
    if let Some(series) = &book.series {
        let vols = series_volumes(conn, series)?;
        if let Some(i) = vols.iter().position(|v| v.id == id) {
            let r = |v: &IpadBook| VolumeRef { id: v.id, title: v.title.clone(), volume: v.volume };
            series_pos = Some((i + 1, vols.len()));
            prev = i.checked_sub(1).map(|j| r(&vols[j]));
            next = vols.get(i + 1).map(r);
        }
    }
    Ok(ReaderInfo { engine, locator: book.locator.clone(), progress: book.progress, prev, next, series_pos, book })
}

/// Đánh dấu đã đọc / chưa đọc (`read` | `unread`); cập nhật cả bảng `positions`.
pub fn set_read_status(conn: &Connection, ids: &[i64], status: &str) -> Result<()> {
    if status != "read" && status != "unread" {
        return Err(Error::msg("trạng thái không hợp lệ"));
    }
    let p: Option<f64> = (status == "read").then_some(1.0);
    for &id in ids {
        conn.execute("UPDATE books SET read_status = ?2, progress = ?3, locator = CASE WHEN ?2 = 'unread' THEN NULL ELSE locator END WHERE id = ?1", params![id, status, p])?;
        conn.execute(
            "INSERT INTO positions (sha256, read_status, progress, locator, last_read_at)
             SELECT sha256, read_status, progress, locator, last_read_at FROM books WHERE id = ?1
             ON CONFLICT(sha256) DO UPDATE SET read_status = excluded.read_status, progress = excluded.progress, locator = excluded.locator",
            [id],
        )?;
    }
    Ok(())
}

/// Các vol đã đọc xong của một bộ (hoặc của cả thư viện khi `series` là `None`) – để xoá, giữ vol đang đọc / chưa đọc (mục 3.1 bước 4).
pub fn read_volume_ids(conn: &Connection, series: Option<&str>) -> Result<Vec<i64>> {
    let mut st = conn.prepare("SELECT id FROM books WHERE read_status = 'read' AND (?1 IS NULL OR series = ?1)")?;
    let v = st.query_map([series], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    pub books: usize,
    /// Dung lượng sách đã chép vào app (không tính sách mở tại chỗ).
    pub app_bytes: i64,
    /// Dung lượng các sách đã đọc xong có thể xoá để lấy lại chỗ.
    pub read_bytes: i64,
    pub read_count: usize,
}

pub fn usage(conn: &Connection) -> Result<Usage> {
    let (books, app_bytes): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN in_place = 0 THEN size END), 0) FROM books",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let (read_count, read_bytes): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM books WHERE read_status = 'read' AND in_place = 0",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(Usage { books: books as usize, app_bytes, read_bytes, read_count: read_count as usize })
}

/// iPad còn trống dưới ngưỡng này thì gợi ý xoá sách đã đọc (I1-08).
pub const LOW_SPACE_BYTES: i64 = 5 * 1024 * 1024 * 1024;

pub fn low_space(free_bytes: i64) -> bool {
    free_bytes < LOW_SPACE_BYTES
}

/// Sách ở `remote_id` của thư viện máy tính nào đó đã có trên iPad chưa (để báo `status` và bỏ qua tải lại).
pub fn remote_ids(conn: &Connection, library_id: &str) -> Result<Vec<i64>> {
    let mut st = conn.prepare("SELECT remote_id FROM books WHERE library_id = ?1 AND remote_id IS NOT NULL")?;
    let v = st.query_map([library_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

// ------------------------------------------------------------------ "không đáng đọc"

/// Đánh dấu "không đáng đọc" (`true`) hoặc đưa lại vào thư viện (`false`). Trả về số cuốn đã đổi.
pub fn set_not_worth(conn: &Connection, ids: &[i64], not_worth: bool) -> Result<usize> {
    let tx = crate::db::write_tx(conn)?;
    let t = now();
    let mut n = 0;
    for &id in ids {
        // Đổi hai lần trong cùng một giây vẫn tăng mốc, để lần đổi sau cùng thắng bản trên máy tính.
        n += tx.execute(
            "INSERT INTO worth (sha256, not_worth, changed_at) SELECT sha256, ?2, ?3 FROM books WHERE id = ?1
             ON CONFLICT(sha256) DO UPDATE SET not_worth = excluded.not_worth, changed_at = MAX(excluded.changed_at, worth.changed_at + 1)
             WHERE worth.not_worth <> excluded.not_worth",
            params![id, not_worth, t],
        )?;
    }
    tx.commit()?;
    Ok(n)
}

/// Số cuốn đang có trên máy đã đánh dấu "không đáng đọc" (nhãn tab).
pub fn not_worth_count(conn: &Connection) -> Result<usize> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM books b JOIN worth w ON w.sha256 = b.sha256 WHERE w.not_worth = 1",
        [],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}

/// Cờ của mọi cuốn đến từ thư viện máy tính `library_id` – gửi lên khi đồng bộ.
pub fn worth_marks(conn: &Connection, library_id: &str) -> Result<Vec<crate::sync_proto::WorthMark>> {
    let mut st = conn.prepare(
        "SELECT b.remote_id, COALESCE(w.not_worth, 0), COALESCE(w.changed_at, 0) FROM books b
         LEFT JOIN worth w ON w.sha256 = b.sha256 WHERE b.library_id = ?1 AND b.remote_id IS NOT NULL",
    )?;
    let v = st
        .query_map([library_id], |r| {
            Ok(crate::sync_proto::WorthMark { book_id: r.get(0)?, not_worth: r.get(1)?, changed_at: r.get(2)? })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

/// Nhận kết quả máy tính đã gộp (`theirs`) cho các cờ máy này đã gửi (`sent`). Máy tính là nơi quyết định
/// (nó đã so "ai đổi sau" và kẹp đồng hồ chạy nhanh), nên máy này nhận theo – trừ cuốn người dùng vừa đổi lại
/// trong lúc đang đồng bộ (mốc trên máy mới hơn mốc đã gửi; lần đồng bộ sau sẽ gửi tiếp).
/// Trả về số cuốn đổi cờ.
pub fn apply_worth_marks(
    conn: &Connection,
    library_id: &str,
    theirs: &[crate::sync_proto::WorthMark],
    sent: &[crate::sync_proto::WorthMark],
) -> Result<usize> {
    let sent: std::collections::HashMap<i64, i64> = sent.iter().map(|m| (m.book_id, m.changed_at)).collect();
    let tx = crate::db::write_tx(conn)?;
    let mut n = 0;
    for m in theirs.iter().filter(|m| m.changed_at > 0) {
        let Some(sha) = tx
            .query_row(
                "SELECT sha256 FROM books WHERE library_id = ?1 AND remote_id = ?2",
                params![library_id, m.book_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        else {
            continue;
        };
        let mine: Option<(bool, i64)> = tx
            .query_row("SELECT not_worth, changed_at FROM worth WHERE sha256 = ?1", [&sha], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let (nw, at) = mine.unwrap_or((false, 0));
        if at > sent.get(&m.book_id).copied().unwrap_or(0) || (nw == m.not_worth && at == m.changed_at) {
            continue;
        }
        tx.execute(
            "INSERT INTO worth (sha256, not_worth, changed_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(sha256) DO UPDATE SET not_worth = excluded.not_worth, changed_at = excluded.changed_at",
            params![sha, m.not_worth, m.changed_at],
        )?;
        n += (nw != m.not_worth) as usize;
    }
    tx.commit()?;
    Ok(n)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Bookmark {
    pub id: i64,
    pub locator: Option<String>,
    pub progress: f64,
    pub label: String,
    pub created_at: i64,
}

fn sha_of(conn: &Connection, id: i64) -> Result<String> {
    conn.query_row("SELECT sha256 FROM books WHERE id = ?1", [id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| Error::msg("không có sách này"))
}

/// Đánh dấu trang của một cuốn, theo thứ tự trong sách.
pub fn bookmarks(conn: &Connection, book_id: i64) -> Result<Vec<Bookmark>> {
    let sha = sha_of(conn, book_id)?;
    let mut st = conn.prepare(
        "SELECT id, locator, progress, label, created_at FROM bookmarks WHERE sha256 = ?1 ORDER BY progress, id",
    )?;
    let v = st
        .query_map([sha], |r| {
            Ok(Bookmark {
                id: r.get(0)?,
                locator: r.get(1)?,
                progress: r.get(2)?,
                label: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

pub fn add_bookmark(
    conn: &Connection,
    book_id: i64,
    locator: Option<&str>,
    progress: f64,
    label: &str,
) -> Result<Bookmark> {
    let sha = sha_of(conn, book_id)?;
    let p = if progress.is_finite() { progress.clamp(0.0, 1.0) } else { 0.0 };
    let t = now();
    conn.execute(
        "INSERT INTO bookmarks (sha256, locator, progress, label, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![sha, locator, p, label, t],
    )?;
    Ok(Bookmark {
        id: conn.last_insert_rowid(),
        locator: locator.map(String::from),
        progress: p,
        label: label.into(),
        created_at: t,
    })
}

pub fn remove_bookmark(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM bookmarks WHERE id = ?1", [id])?;
    Ok(())
}

/// Đánh dấu "Không tìm thấy file" cho sách mở tại chỗ bị xoá/di chuyển (I1-06).
pub fn set_missing(conn: &Connection, id: i64, missing: bool) -> Result<()> {
    conn.execute("UPDATE books SET missing = ?2 WHERE id = ?1", params![id, missing])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nb(title: &str, series: Option<&str>, vol: Option<i64>, sha: &str, size: i64) -> NewBook {
        NewBook {
            title: title.into(),
            series: series.map(String::from),
            volume: vol,
            format: "cbz".into(),
            size,
            sha256: sha.into(),
            location: format!("{sha}.cbz"),
            ..Default::default()
        }
    }

    #[test]
    fn reader_info_and_read_status() {
        let c = open_in_memory().unwrap();
        let ids: Vec<i64> = (1..=3)
            .map(|v| add_book(&c, &nb(&format!("S {v}"), Some("S"), Some(v), &format!("h{v}"), 10)).unwrap().0)
            .collect();
        let i = reader_info(&c, ids[1]).unwrap();
        assert_eq!(i.series_pos, Some((2, 3)));
        assert_eq!((i.prev.unwrap().id, i.next.unwrap().id), (ids[0], ids[2]));
        assert_eq!(i.book.file_name, "h2.cbz");
        set_read_status(&c, &[ids[0]], "read").unwrap();
        assert_eq!(get(&c, ids[0]).unwrap().unwrap().read_status, "read");
        assert!(set_read_status(&c, &[ids[0]], "x").is_err());
    }

    #[test]
    fn dedupe_by_hash_and_series_order() {
        let c = open_in_memory().unwrap();
        for v in [10, 2, 1] {
            add_book(&c, &nb(&format!("One Piece {v}"), Some("One Piece"), Some(v), &format!("h{v}"), 100)).unwrap();
        }
        let (id, added) = add_book(&c, &nb("One Piece 2 (bản chép)", Some("One Piece"), Some(2), "h2", 100)).unwrap();
        assert!(!added);
        assert_eq!(get(&c, id).unwrap().unwrap().volume, Some(2));
        let vols: Vec<_> = series_volumes(&c, "One Piece").unwrap().iter().map(|b| b.volume.unwrap()).collect();
        assert_eq!(vols, vec![1, 2, 10]);
        let items = list(&c, &Query::default()).unwrap();
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], Item::Series(s) if s.volumes == 3));
    }

    #[test]
    fn search_ignores_diacritics_and_continue_tile() {
        let c = open_in_memory().unwrap();
        let (a, _) =
            add_book(&c, &NewBook { authors: "Nguyễn Nhật Ánh".into(), ..nb("Mắt Biếc", None, None, "m", 10) })
                .unwrap();
        let ids: Vec<i64> = (1..=3)
            .map(|v| add_book(&c, &nb(&format!("Conan {v}"), Some("Conan"), Some(v), &format!("c{v}"), 5)).unwrap().0)
            .collect();
        let hit =
            list(&c, &Query { q: Some("mat biec nhat anh".into()), sort: Sort::Title, ..Default::default() }).unwrap();
        assert_eq!(hit.len(), 1);
        assert!(matches!(&hit[0], Item::Book(b) if b.id == a));
        save_position(&c, ids[0], Some("page:3"), 1.0).unwrap();
        let Item::Series(t) = list(&c, &Query { q: Some("conan".into()), ..Default::default() }).unwrap().remove(0)
        else {
            panic!()
        };
        assert_eq!((t.read, t.continue_book_id), (1, Some(ids[1])));
        save_position(&c, ids[1], Some("page:1"), 0.2).unwrap();
        assert_eq!(continue_reading(&c, 10).unwrap().len(), 1);
    }

    #[test]
    fn position_survives_remove_and_resend() {
        let c = open_in_memory().unwrap();
        let (id, _) = add_book(&c, &nb("Sách", None, None, "x1", 1000)).unwrap();
        assert_eq!(save_position(&c, id, Some("epubcfi(/6/4)"), 0.4).unwrap(), "reading");
        let r = remove_books(&c, &[id]).unwrap();
        assert_eq!((r.count, r.freed_bytes, r.files_to_delete), (1, 1000, vec!["x1.cbz".to_string()]));
        assert!(get(&c, id).unwrap().is_none());
        let (id2, added) = add_book(&c, &nb("Sách", None, None, "x1", 1000)).unwrap();
        assert!(added);
        let b = get(&c, id2).unwrap().unwrap();
        assert_eq!(
            (b.locator.as_deref(), b.progress, b.read_status.as_str()),
            (Some("epubcfi(/6/4)"), Some(0.4), "reading")
        );
    }

    #[test]
    fn in_place_books_free_nothing_and_usage() {
        let c = open_in_memory().unwrap();
        let (a, _) = add_book(&c, &NewBook { in_place: true, ..nb("Ngoài", None, None, "i", 500) }).unwrap();
        let (b, _) = add_book(&c, &nb("Trong", None, None, "j", 300)).unwrap();
        save_position(&c, a, None, 1.0).unwrap();
        save_position(&c, b, None, 1.0).unwrap();
        let u = usage(&c).unwrap();
        assert_eq!((u.books, u.app_bytes, u.read_count, u.read_bytes), (2, 300, 1, 300));
        let r = remove_books(&c, &[a]).unwrap();
        assert_eq!((r.freed_bytes, r.files_to_delete.len()), (0, 0));
        assert!(low_space(1 << 30) && !low_space(10 << 30));
    }

    #[test]
    fn bookmarks_survive_remove_and_resend() {
        let c = open_in_memory().unwrap();
        let (id, _) = add_book(&c, &nb("Sách", None, None, "hb", 10)).unwrap();
        let b2 = add_bookmark(&c, id, Some("epubcfi(/6/8)"), 0.5, "Chương 2").unwrap();
        add_bookmark(&c, id, Some("epubcfi(/6/4)"), 0.1, "Chương 1").unwrap();
        let v = bookmarks(&c, id).unwrap();
        assert_eq!(v.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(), ["Chương 1", "Chương 2"]);
        remove_books(&c, &[id]).unwrap();
        let (id2, _) = add_book(&c, &nb("Sách", None, None, "hb", 10)).unwrap();
        assert_eq!(bookmarks(&c, id2).unwrap().len(), 2);
        remove_bookmark(&c, b2.id).unwrap();
        assert_eq!(bookmarks(&c, id2).unwrap().len(), 1);
        assert!(is_text_format("EPUB") && is_text_format("azw3") && !is_text_format("pdf") && !is_text_format("cbz"));
    }
}
