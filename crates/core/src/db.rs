//! SQLite (WAL) + FTS5. Migration theo `PRAGMA user_version` (E0-03).

use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE roots (
        id INTEGER PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL,
        added_at INTEGER NOT NULL
    );
    CREATE TABLE books (
        id INTEGER PRIMARY KEY,
        root_id INTEGER NOT NULL REFERENCES roots(id),
        path TEXT NOT NULL UNIQUE,
        rel_path TEXT NOT NULL,
        file_name TEXT NOT NULL,
        format TEXT NOT NULL,
        size INTEGER NOT NULL,
        mtime INTEGER NOT NULL,
        title TEXT NOT NULL,
        title_from_meta INTEGER NOT NULL DEFAULT 0,
        authors TEXT NOT NULL DEFAULT '',
        language TEXT,
        subjects TEXT NOT NULL DEFAULT '',
        isbn TEXT,
        series TEXT,
        volume INTEGER,
        page_count INTEGER,
        drm INTEGER NOT NULL DEFAULT 0,
        norm_title TEXT NOT NULL,
        norm_author TEXT NOT NULL DEFAULT '',
        category TEXT,
        in_inbox INTEGER NOT NULL DEFAULT 0,
        quick_hash TEXT,
        full_hash TEXT,
        hash_size INTEGER,
        hash_mtime INTEGER,
        status TEXT NOT NULL DEFAULT 'present',
        error TEXT,
        first_seen_scan INTEGER,
        last_seen_scan INTEGER,
        added_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        read_status TEXT NOT NULL DEFAULT 'unread',
        progress REAL,
        locator TEXT,
        last_read_at INTEGER
    );
    CREATE INDEX books_size ON books(size) WHERE status = 'present';
    CREATE INDEX books_full_hash ON books(full_hash) WHERE full_hash IS NOT NULL;
    CREATE INDEX books_norm_title ON books(norm_title);
    CREATE INDEX books_isbn ON books(isbn) WHERE isbn IS NOT NULL;
    CREATE INDEX books_status ON books(status, root_id);
    CREATE INDEX books_series ON books(category, series);

    CREATE VIRTUAL TABLE books_fts USING fts5(
        title, author, file, series,
        tokenize = "unicode61 remove_diacritics 2"
    );

    CREATE TABLE dup_decisions (
        fp_a TEXT NOT NULL,
        fp_b TEXT NOT NULL,
        decision TEXT NOT NULL,
        decided_at INTEGER NOT NULL,
        PRIMARY KEY (fp_a, fp_b)
    );

    CREATE TABLE batches (
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL,
        note TEXT,
        created_at INTEGER NOT NULL,
        undone_at INTEGER
    );
    CREATE TABLE operations (
        id INTEGER PRIMARY KEY,
        batch_id INTEGER NOT NULL REFERENCES batches(id),
        kind TEXT NOT NULL,
        book_id INTEGER,
        src TEXT NOT NULL,
        dst TEXT,
        size INTEGER,
        status TEXT NOT NULL,
        message TEXT,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX operations_batch ON operations(batch_id);

    CREATE TABLE scan_reports (
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL,
        started_at INTEGER NOT NULL,
        finished_at INTEGER,
        status TEXT NOT NULL,
        stats TEXT
    );
    "#,
    // v2: đồng bộ sang iPad (I3) – hàng chờ, iPad đã ghép, siêu dữ liệu (library_id).
    r#"
    CREATE TABLE meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE outbox (
        book_id INTEGER PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
        queued_at INTEGER NOT NULL,
        sent_at INTEGER
    );
    CREATE TABLE devices (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        token_hash TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL,
        last_seen INTEGER,
        free_bytes INTEGER,
        on_device TEXT NOT NULL DEFAULT '[]'
    );
    "#,
    // v3: hàng chờ riêng từng thiết bị (nhiều iPad/iPhone dùng chung thư viện). `kind`: ipad | iphone
    // (iPhone chỉ nhận sách chữ). Hàng chờ cũ: cuốn chưa gửi → xếp cho mọi thiết bị đã ghép; cuốn đã gửi →
    // ghi "đã gửi" cho thiết bị đang có cuốn đó.
    r#"
    ALTER TABLE devices ADD COLUMN kind TEXT NOT NULL DEFAULT 'ipad';
    UPDATE devices SET kind = 'iphone' WHERE name LIKE '%iphone%';
    CREATE TABLE outbox_new (
        book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
        device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
        queued_at INTEGER NOT NULL,
        sent_at INTEGER,
        PRIMARY KEY (book_id, device_id)
    );
    INSERT INTO outbox_new (book_id, device_id, queued_at, sent_at)
        SELECT o.book_id, d.id, o.queued_at, NULL FROM outbox o, devices d WHERE o.sent_at IS NULL;
    INSERT OR IGNORE INTO outbox_new (book_id, device_id, queued_at, sent_at)
        SELECT o.book_id, d.id, o.queued_at, o.sent_at FROM outbox o, devices d
        WHERE o.sent_at IS NOT NULL AND o.book_id IN (SELECT value FROM json_each(d.on_device));
    DROP TABLE outbox;
    ALTER TABLE outbox_new RENAME TO outbox;
    CREATE INDEX outbox_device ON outbox(device_id, sent_at);
    "#,
    // v4: đánh dấu "không đáng đọc" – ẩn khỏi thư viện, hiện ở mục riêng. `worth_at`: lúc đổi lần cuối
    // (giây), để đồng bộ hai chiều với iPad/iPhone/Android theo kiểu "ai đổi sau thắng".
    r#"
    ALTER TABLE books ADD COLUMN not_worth INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE books ADD COLUMN worth_at INTEGER NOT NULL DEFAULT 0;
    CREATE INDEX books_not_worth ON books(not_worth) WHERE not_worth = 1;
    "#,
    // v5: tiêu đề người dùng tự sửa (app Windows). Có giá trị thì các lần quét sau giữ tiêu đề này thay vì tiêu đề
    // đọc từ file – để tìm kiếm và so nghi trùng dùng đúng tên đã sửa.
    r#"
    ALTER TABLE books ADD COLUMN user_title TEXT;
    "#,
    // v6: sổ "sách đã xoá" (xem deleted.rs) – nhớ nội dung (dung lượng + băm) và thông tin của sách người dùng đã
    // đưa vào Thùng rác, kể cả sau khi dọn sạch Thùng rác Windows. Tải lại đúng file đó thì lần quét sau tự đưa
    // vào Thùng rác; bản gần giống thì hiện ở "Nghi trùng". Nạp sẵn từ các lần xoá cũ (lô xoá thủ công / nghi
    // trùng). `reject_deleted_after`: chỉ xét sách thêm vào SAU lần quét này (sách có sẵn không bao giờ bị loại).
    r#"
    CREATE TABLE deleted_books (
        id INTEGER PRIMARY KEY,
        book_id INTEGER,
        path TEXT NOT NULL,
        rel_path TEXT NOT NULL,
        file_name TEXT NOT NULL,
        format TEXT NOT NULL,
        size INTEGER NOT NULL,
        mtime INTEGER NOT NULL,
        quick_hash TEXT,
        full_hash TEXT,
        title TEXT NOT NULL,
        authors TEXT NOT NULL DEFAULT '',
        isbn TEXT,
        series TEXT,
        volume INTEGER,
        page_count INTEGER,
        norm_title TEXT NOT NULL,
        norm_author TEXT NOT NULL DEFAULT '',
        reason TEXT NOT NULL,
        deleted_at INTEGER NOT NULL
    );
    CREATE INDEX deleted_books_size ON deleted_books(size);
    CREATE INDEX deleted_books_book ON deleted_books(book_id);
    INSERT INTO deleted_books(book_id, path, rel_path, file_name, format, size, mtime, quick_hash, full_hash, title,
        authors, isbn, series, volume, page_count, norm_title, norm_author, reason, deleted_at)
    SELECT b.id, b.path, b.rel_path, b.file_name, b.format, b.size, b.mtime,
        CASE WHEN b.hash_size = b.size AND b.hash_mtime = b.mtime THEN b.quick_hash END,
        CASE WHEN b.hash_size = b.size AND b.hash_mtime = b.mtime THEN b.full_hash END,
        b.title, b.authors, b.isbn, b.series, b.volume, b.page_count, b.norm_title, b.norm_author,
        (SELECT ba.kind FROM operations o JOIN batches ba ON ba.id = o.batch_id
          WHERE o.book_id = b.id AND o.kind = 'trash' AND o.status = 'done' ORDER BY o.id DESC LIMIT 1),
        b.updated_at
    FROM books b
    WHERE b.status = 'trashed' AND EXISTS (
        SELECT 1 FROM operations o JOIN batches ba ON ba.id = o.batch_id
        WHERE o.book_id = b.id AND o.kind = 'trash' AND o.status = 'done' AND ba.kind IN ('trash', 'dedupe_suspect'));
    INSERT OR REPLACE INTO meta(key, value)
        SELECT 'reject_deleted_after', CAST(COALESCE(MAX(id), 0) AS TEXT) FROM scan_reports;
    "#,
];

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// mtime tính bằng mili-giây (đủ chính xác để phát hiện file đổi).
pub fn mtime_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_millis() as i64
}

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Giao dịch ghi: `BEGIN IMMEDIATE` – giữ chỗ ghi ngay từ đầu.
///
/// Không dùng giao dịch DEFERRED (`unchecked_transaction`) cho việc ghi: ở WAL, giao dịch DEFERRED đọc trước (lấy
/// ảnh chụp) rồi mới ghi; nếu kết nối khác (luồng nền, giao diện) đã ghi xen vào giữa thì SQLite trả SQLITE_BUSY
/// ("database is locked") NGAY, không chờ `busy_timeout`. IMMEDIATE thì chờ tới lượt theo `busy_timeout`.
pub fn write_tx(conn: &Connection) -> Result<rusqlite::Transaction<'_>> {
    Ok(rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?)
}

fn configure(conn: &Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

pub fn schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

pub fn migrate(conn: &Connection) -> Result<()> {
    let current = schema_version(conn)? as usize;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = write_tx(conn)?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
        tracing::info!("DB migrate lên v{}", i + 1);
    }
    Ok(())
}

/// Sao lưu DB (E10-06 sẽ dùng), giữ `keep` bản gần nhất.
pub fn backup(conn: &Connection, dir: &Path, keep: usize) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let dst = dir.join(format!("library-{}.db", now()));
    conn.execute("VACUUM INTO ?1", params![dst.to_string_lossy()])?;
    let mut files: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "db"))
        .collect();
    files.sort();
    while files.len() > keep {
        let _ = std::fs::remove_file(files.remove(0));
    }
    Ok(dst)
}

/// Đánh dấu đúng các thư mục thư viện trong cấu hình là `library`; thư mục thư viện cũ không còn trong cấu hình trở
/// thành nguồn phụ (sách trong đó vẫn còn, chỉ không còn là "thư viện").
pub fn sync_library_roots(conn: &Connection, cfg: &crate::config::Config) -> Result<()> {
    let libs: Vec<String> = cfg.library_roots().iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let mut st = conn.prepare("SELECT id, path FROM roots WHERE kind = 'library'")?;
    let current = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, path) in current {
        if !libs.iter().any(|l| crate::config::same_path(Path::new(l), Path::new(&path))) {
            conn.execute("UPDATE roots SET kind = 'source' WHERE id = ?1", [id])?;
        }
    }
    for l in &libs {
        ensure_root(conn, l, "library")?;
    }
    Ok(())
}

pub fn ensure_root(conn: &Connection, path: &str, kind: &str) -> Result<i64> {
    if let Some(id) = conn.query_row("SELECT id FROM roots WHERE path = ?1", [path], |r| r.get(0)).optional()? {
        conn.execute("UPDATE roots SET kind = ?2 WHERE id = ?1", params![id, kind])?;
        return Ok(id);
    }
    conn.execute("INSERT INTO roots(path, kind, added_at) VALUES (?1, ?2, ?3)", params![path, kind, now()])?;
    Ok(conn.last_insert_rowid())
}

/// Một cuốn sách trong DB.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Book {
    pub id: i64,
    pub root_id: i64,
    pub path: String,
    pub rel_path: String,
    pub file_name: String,
    pub format: String,
    pub size: i64,
    pub mtime: i64,
    pub title: String,
    pub authors: String,
    pub language: Option<String>,
    pub subjects: String,
    pub isbn: Option<String>,
    pub series: Option<String>,
    pub volume: Option<i64>,
    pub page_count: Option<i64>,
    pub norm_title: String,
    pub norm_author: String,
    pub category: Option<String>,
    pub in_inbox: bool,
    pub quick_hash: Option<String>,
    pub full_hash: Option<String>,
    pub hash_size: Option<i64>,
    pub hash_mtime: Option<i64>,
    pub status: String,
    pub error: Option<String>,
    pub read_status: String,
    pub progress: Option<f64>,
    pub title_from_meta: bool,
    pub drm: bool,
    /// Đã đánh dấu "không đáng đọc" (ẩn khỏi thư viện).
    pub not_worth: bool,
}

pub const BOOK_COLS: &str = "id, root_id, path, rel_path, file_name, format, size, mtime, title, authors, \
    language, subjects, isbn, series, volume, page_count, norm_title, norm_author, category, in_inbox, \
    quick_hash, full_hash, hash_size, hash_mtime, status, error, read_status, progress, title_from_meta, drm, not_worth";

impl Book {
    pub fn from_row(r: &Row) -> rusqlite::Result<Book> {
        Ok(Book {
            id: r.get(0)?,
            root_id: r.get(1)?,
            path: r.get(2)?,
            rel_path: r.get(3)?,
            file_name: r.get(4)?,
            format: r.get(5)?,
            size: r.get(6)?,
            mtime: r.get(7)?,
            title: r.get(8)?,
            authors: r.get(9)?,
            language: r.get(10)?,
            subjects: r.get(11)?,
            isbn: r.get(12)?,
            series: r.get(13)?,
            volume: r.get(14)?,
            page_count: r.get(15)?,
            norm_title: r.get(16)?,
            norm_author: r.get(17)?,
            category: r.get(18)?,
            in_inbox: r.get(19)?,
            quick_hash: r.get(20)?,
            full_hash: r.get(21)?,
            hash_size: r.get(22)?,
            hash_mtime: r.get(23)?,
            status: r.get(24)?,
            error: r.get(25)?,
            read_status: r.get(26)?,
            progress: r.get(27)?,
            title_from_meta: r.get(28)?,
            drm: r.get(29)?,
            not_worth: r.get(30)?,
        })
    }

    pub fn stem(&self) -> &str {
        match self.file_name.rfind('.') {
            Some(i) if i > 0 => &self.file_name[..i],
            _ => &self.file_name,
        }
    }

    /// Số cấp thư mục tính từ gốc (dùng cho quy tắc "đường dẫn nông hơn").
    pub fn depth(&self) -> usize {
        self.rel_path.matches(['/', '\\']).count()
    }
}

pub fn get_book(conn: &Connection, id: i64) -> Result<Option<Book>> {
    Ok(conn.query_row(&format!("SELECT {BOOK_COLS} FROM books WHERE id = ?1"), [id], Book::from_row).optional()?)
}

pub fn books_where(conn: &Connection, clause: &str, p: impl rusqlite::Params) -> Result<Vec<Book>> {
    let mut st = conn.prepare(&format!("SELECT {BOOK_COLS} FROM books WHERE {clause}"))?;
    let rows = st.query_map(p, Book::from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Ghi lại chỉ mục FTS của một cuốn (văn bản đã bỏ dấu để tìm không dấu, kể cả `đ`).
pub fn fts_upsert(conn: &Connection, id: i64) -> Result<()> {
    use crate::normalize::simple_key;
    conn.execute("DELETE FROM books_fts WHERE rowid = ?1", [id])?;
    let row: Option<(String, String, String, Option<String>, String)> = conn
        .query_row("SELECT title, authors, file_name, series, status FROM books WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .optional()?;
    if let Some((title, authors, file, series, status)) = row {
        if status == "present" {
            conn.execute(
                "INSERT INTO books_fts(rowid, title, author, file, series) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    simple_key(&title),
                    simple_key(&authors),
                    simple_key(&file),
                    simple_key(series.as_deref().unwrap_or(""))
                ],
            )?;
        }
    }
    Ok(())
}

pub fn fts_delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM books_fts WHERE rowid = ?1", [id])?;
    Ok(())
}

pub fn set_status(conn: &Connection, id: i64, status: &str) -> Result<()> {
    conn.execute("UPDATE books SET status = ?2, updated_at = ?3 WHERE id = ?1", params![id, status, now()])?;
    if status == "present" {
        fts_upsert(conn, id)
    } else {
        fts_delete(conn, id)
    }
}

pub fn new_batch(conn: &Connection, kind: &str, note: Option<&str>) -> Result<i64> {
    conn.execute("INSERT INTO batches(kind, note, created_at) VALUES (?1, ?2, ?3)", params![kind, note, now()])?;
    Ok(conn.last_insert_rowid())
}

#[allow(clippy::too_many_arguments)]
pub fn log_op(
    conn: &Connection,
    batch: i64,
    kind: &str,
    book_id: Option<i64>,
    src: &str,
    dst: Option<&str>,
    size: Option<i64>,
    status: &str,
    message: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO operations(batch_id, kind, book_id, src, dst, size, status, message, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![batch, kind, book_id, src, dst, size, status, message, now()],
    )?;
    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migrate_twice_is_noop() {
        let c = open_in_memory().unwrap();
        assert_eq!(schema_version(&c).unwrap(), MIGRATIONS.len() as i64);
        migrate(&c).unwrap();
        assert_eq!(schema_version(&c).unwrap(), MIGRATIONS.len() as i64);
    }

    /// v5 → v6: sổ sách đã xoá nạp từ các lần xoá tay / bỏ nghi trùng cũ (không lấy bản trùng chắc chắn, không lấy
    /// file phân loại tự bỏ).
    #[test]
    fn v6_backfills_deleted_books_from_trash_batches() {
        let c = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..5] {
            c.execute_batch(sql).unwrap();
        }
        c.pragma_update(None, "user_version", 5).unwrap();
        c.execute("INSERT INTO roots (id, path, kind, added_at) VALUES (1, 'r', 'library', 0)", []).unwrap();
        for id in 1..=4 {
            c.execute(
                "INSERT INTO books (id, root_id, path, rel_path, file_name, format, size, mtime, title, norm_title,
                   quick_hash, hash_size, hash_mtime, status, added_at, updated_at)
                 VALUES (?1, 1, ?1, ?1, ?1, 'epub', 10, 5, 't', 't', 'q' || ?1, 10, 5, 'trashed', 0, 7)",
                [id],
            )
            .unwrap();
        }
        c.execute_batch(
            "INSERT INTO batches (id, kind, created_at) VALUES (1, 'trash', 0), (2, 'dedupe', 0), (3, 'dedupe_suspect', 0);
             INSERT INTO operations (batch_id, kind, book_id, src, status, created_at)
               VALUES (1, 'trash', 1, '1', 'done', 0), (2, 'trash', 2, '2', 'done', 0), (3, 'trash', 3, '3', 'done', 0);
             INSERT INTO scan_reports (id, kind, started_at, status) VALUES (9, 'app', 0, 'done');",
        )
        .unwrap();
        migrate(&c).unwrap();
        let rows: Vec<(i64, Option<String>, String, i64)> = c
            .prepare("SELECT book_id, quick_hash, reason, deleted_at FROM deleted_books ORDER BY book_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![(1, Some("q1".into()), "trash".into(), 7), (3, Some("q3".into()), "dedupe_suspect".into(), 7)]
        );
        let after: String =
            c.query_row("SELECT value FROM meta WHERE key = 'reject_deleted_after'", [], |r| r.get(0)).unwrap();
        assert_eq!(after, "9", "sách có sẵn lúc nâng cấp không bị xét");
    }

    /// v2 → v3: hàng chờ chung tách theo thiết bị.
    #[test]
    fn v3_splits_shared_outbox_per_device() {
        let c = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..2] {
            c.execute_batch(sql).unwrap();
        }
        c.pragma_update(None, "user_version", 2).unwrap();
        c.execute("INSERT INTO roots (id, path, kind, added_at) VALUES (1, 'r', 'library', 0)", []).unwrap();
        for id in 1..=3 {
            c.execute(
                "INSERT INTO books (id, root_id, path, rel_path, file_name, format, size, mtime, title, norm_title, added_at, updated_at)
                 VALUES (?1, 1, ?1, ?1, ?1, 'epub', 1, 1, 't', 't', 0, 0)",
                [id],
            )
            .unwrap();
        }
        c.execute_batch(
            "INSERT INTO devices (id, name, token_hash, created_at, on_device) VALUES (1, 'iPad Air', 'a', 0, '[2]'), (2, 'iPhone', 'b', 0, '[]');
             INSERT INTO outbox VALUES (1, 10, NULL), (2, 10, 20), (3, 10, 20);",
        )
        .unwrap();
        migrate(&c).unwrap();
        let rows: Vec<(i64, i64, Option<i64>)> = c
            .prepare("SELECT book_id, device_id, sent_at FROM outbox ORDER BY book_id, device_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        // Chưa gửi → mọi thiết bị; đã gửi → thiết bị đang có cuốn đó; cuốn 3 đã gửi nhưng không máy nào có → bỏ.
        assert_eq!(rows, vec![(1, 1, None), (1, 2, None), (2, 1, Some(20))]);
        let kinds: Vec<String> = c
            .prepare("SELECT kind FROM devices ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|x| x.unwrap())
            .collect();
        assert_eq!(kinds, ["ipad", "iphone"]);
    }
}
