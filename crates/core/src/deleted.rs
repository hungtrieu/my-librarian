//! Sổ "sách đã xoá": nhớ những cuốn người dùng đã đưa vào Thùng rác (xoá tay, hoặc bỏ bản nghi trùng), kể cả khi
//! Thùng rác Windows đã được dọn sạch.
//!
//! - Tải lại **đúng file đó** (cùng dung lượng + băm nhanh, và SHA-256 nếu lúc xoá đã có) → lần quét sau tự đưa
//!   vào Thùng rác ([`reject_known`], lô `reject_deleted` trong lịch sử).
//! - Tải một bản **gần giống** (cùng tên chuẩn hoá / ISBN, nội dung khác) → hiện ở "Nghi trùng" cùng một mục "đã xoá"
//!   (xem [`ghosts`]) để người dùng quyết định.
//!
//! An toàn: chỉ xét sách được thêm vào thư viện SAU lần kiểm tra trước (sách đang có không bao giờ bị loại), sau bước
//! nhận lại sách chuyển chỗ; bỏ qua khi có sách "mất" cùng dung lượng (có thể chính là sách đó bị chuyển chỗ). Khôi
//! phục từ Thùng rác (file về đúng chỗ cũ, cùng dung lượng + thời điểm sửa) thì xoá khỏi sổ. Bản trùng chắc chắn bị
//! dọn (lô `dedupe`) không ghi vào sổ vì nội dung vẫn còn trong thư viện.

use crate::config::Config;
use crate::db::{self, now, Book};
use crate::error::Result;
use crate::hash;
use crate::trash::{trash_items_with, TrashItem, TrashReport, Trasher};
use crate::{OnStep, Step};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Bước báo tiến trình khi so sách mới với sổ sách đã xoá (x / y sách).
pub const REJECT_PHASE: &str = "reject_deleted";
/// Lô trong lịch sử thao tác khi tự đưa sách đã xoá trước đây vào Thùng rác.
pub const REJECT_KIND: &str = "reject_deleted";
const MARKER: &str = "reject_deleted_after";

/// Loại lô xoá được ghi vào sổ.
pub fn remembers(kind: &str) -> bool {
    matches!(kind, "trash" | "dedupe_suspect" | REJECT_KIND)
}

/// Ghi một cuốn vừa vào Thùng rác. `quick`: băm nhanh của file (tính ngay trước khi xoá).
pub fn record(conn: &Connection, b: &Book, quick: Option<&str>, kind: &str) -> Result<()> {
    let full = b.full_hash.as_deref().filter(|_| b.hash_size == Some(b.size) && b.hash_mtime == Some(b.mtime));
    if let Some(q) = quick {
        let existing: Option<i64> = conn
            .query_row("SELECT id FROM deleted_books WHERE size = ?1 AND quick_hash = ?2", params![b.size, q], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(id) = existing {
            // Cùng nội dung đã có trong sổ (vd. bản tải lại vừa bị loại): chỉ bổ sung băm đầy đủ nếu mới có.
            conn.execute(
                "UPDATE deleted_books SET full_hash = COALESCE(full_hash, ?2) WHERE id = ?1",
                params![id, full],
            )?;
            return Ok(());
        }
    }
    conn.execute(
        "INSERT INTO deleted_books(book_id, path, rel_path, file_name, format, size, mtime, quick_hash, full_hash,
           title, authors, isbn, series, volume, page_count, norm_title, norm_author, reason, deleted_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
        params![
            b.id,
            b.path,
            b.rel_path,
            b.file_name,
            b.format,
            b.size,
            b.mtime,
            quick,
            full,
            b.title,
            b.authors,
            b.isbn,
            b.series,
            b.volume,
            b.page_count,
            b.norm_title,
            b.norm_author,
            kind,
            now()
        ],
    )?;
    Ok(())
}

/// Sách `book_id` (đang ở trạng thái đã xoá) xuất hiện lại đúng chỗ cũ: phân biệt khôi phục từ Thùng rác với tải lại.
/// Khôi phục → xoá khỏi sổ, trả `true`. Ngược lại trả `false` (file mới ở cùng đường dẫn – coi như sách mới).
///
/// Khôi phục = cùng dung lượng + thời điểm sửa như lúc xoá **và** file được tạo trước lúc xoá. Chỉ so thời điểm sửa
/// thì không đủ: copy bằng Explorer (từ ổ sao lưu, thư mục tải về…) giữ nguyên thời điểm sửa, nhưng thời điểm tạo
/// là lúc copy; còn Restore trong Thùng rác chỉ đổi tên nên giữ nguyên thời điểm tạo cũ.
pub fn on_reappear(conn: &Connection, book_id: i64, path: &Path, size: i64, mtime: i64) -> Result<bool> {
    let (s, m, q, hs): (i64, i64, Option<String>, Option<i64>) =
        conn.query_row("SELECT size, mtime, quick_hash, hash_size FROM books WHERE id = ?1", [book_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
    if s != size || m != mtime {
        return Ok(false);
    }
    let deleted_at: Option<i64> =
        conn.query_row("SELECT MAX(deleted_at) FROM deleted_books WHERE book_id = ?1", [book_id], |r| r.get(0))?;
    let created = std::fs::metadata(path).and_then(|md| md.created()).ok();
    if let (Some(at), Some(c)) = (deleted_at, created) {
        let created_secs = c.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        if created_secs > at {
            return Ok(false); // tạo sau lúc xoá: bản copy / tải lại, không phải khôi phục
        }
    }
    conn.execute("DELETE FROM deleted_books WHERE book_id = ?1", [book_id])?;
    if let (Some(q), Some(hs)) = (q, hs) {
        conn.execute("DELETE FROM deleted_books WHERE size = ?1 AND quick_hash = ?2", params![hs, q])?;
    }
    Ok(true)
}

/// Số sách trong sổ.
pub fn count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM deleted_books", [], |r| r.get(0))?)
}

/// Id "ma" (âm) của mục sổ `id` khi xuất hiện như một thành viên nhóm nghi trùng.
pub fn ghost_id(id: i64) -> i64 {
    -id
}

/// Các mục trong sổ dưới dạng [`Book`] "ma" (id âm, `status = "deleted"`, `path` = chỗ cũ), kèm thời điểm xoá – để so
/// nghi trùng với sách mới tải về.
pub fn ghosts(conn: &Connection) -> Result<Vec<(Book, i64)>> {
    let mut st = conn.prepare(
        "SELECT id, path, rel_path, file_name, format, size, mtime, quick_hash, full_hash, title, authors, isbn,
                series, volume, page_count, norm_title, norm_author, deleted_at
         FROM deleted_books ORDER BY id",
    )?;
    let v = st
        .query_map([], |r| {
            let size: i64 = r.get(5)?;
            let mtime: i64 = r.get(6)?;
            Ok((
                Book {
                    id: ghost_id(r.get(0)?),
                    root_id: 0,
                    path: r.get(1)?,
                    rel_path: r.get(2)?,
                    file_name: r.get(3)?,
                    format: r.get(4)?,
                    size,
                    mtime,
                    title: r.get(9)?,
                    authors: r.get(10)?,
                    language: None,
                    subjects: String::new(),
                    isbn: r.get(11)?,
                    series: r.get(12)?,
                    volume: r.get(13)?,
                    page_count: r.get(14)?,
                    norm_title: r.get(15)?,
                    norm_author: r.get(16)?,
                    category: None,
                    in_inbox: false,
                    quick_hash: r.get(7)?,
                    full_hash: r.get(8)?,
                    hash_size: Some(size),
                    hash_mtime: Some(mtime),
                    status: "deleted".into(),
                    error: None,
                    read_status: "unread".into(),
                    progress: None,
                    title_from_meta: true,
                    drm: false,
                    not_worth: false,
                },
                r.get(17)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Mục sổ ứng với id "ma" (âm).
pub fn ghost(conn: &Connection, id: i64) -> Result<Option<Book>> {
    if id >= 0 {
        return Ok(None);
    }
    Ok(ghosts(conn)?.into_iter().map(|(b, _)| b).find(|b| b.id == id))
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct RejectReport {
    /// Số sách mới đã so với sổ.
    pub checked: usize,
    pub trash: TrashReport,
    pub cancelled: bool,
}

fn marker(conn: &Connection) -> Result<i64> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [MARKER], |r| r.get::<_, String>(0))
        .optional()?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

/// Đưa vào Thùng rác những sách mới (thêm từ lần kiểm tra trước) có nội dung y hệt một cuốn trong sổ. Gọi sau
/// [`crate::scan::scan`] (đã nhận lại sách chuyển chỗ). Báo tiến trình [`REJECT_PHASE`] rồi `trash`, dừng được giữa
/// chừng (lần sau xét lại phần còn lại).
pub fn reject_known(
    conn: &Connection,
    cfg: &Config,
    trasher: &dyn Trasher,
    cancel: Option<&AtomicBool>,
    on: Option<OnStep>,
) -> Result<RejectReport> {
    let mut rep = RejectReport::default();
    let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    let after = marker(conn)?;
    let upto: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM scan_reports", [], |r| r.get(0))?;
    let cands = db::books_where(
        conn,
        "status = 'present' AND first_seen_scan > ?1 AND size IN (SELECT size FROM deleted_books) ORDER BY id",
        [after],
    )?;
    let total = cands.len();
    let step = |done: usize| {
        if let Some(f) = on {
            f(Step { phase: REJECT_PHASE, done, total });
        }
    };
    step(0);
    let mut last = std::time::Instant::now();
    let mut hits: Vec<(Book, String)> = Vec::new();
    for (i, b) in cands.iter().enumerate() {
        crate::progress::pulse();
        if cancelled() {
            rep.cancelled = true;
            return Ok(rep);
        }
        if last.elapsed() >= std::time::Duration::from_millis(250) {
            step(i);
            last = std::time::Instant::now();
        }
        rep.checked += 1;
        // Có sách "mất" cùng dung lượng: có thể chính là nó bị chuyển chỗ mà chưa nhận lại được – không đụng.
        let missing_same: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM books WHERE status = 'missing' AND size = ?1)",
            [b.size],
            |r| r.get(0),
        )?;
        if missing_same {
            continue;
        }
        crate::progress::working_on(Path::new(&b.path));
        let Some(q) = hash::ensure_quick(conn, b.id)? else { continue };
        let tomb: Option<(i64, Option<String>, String, i64)> = conn
            .query_row(
                "SELECT id, full_hash, path, deleted_at FROM deleted_books WHERE size = ?1 AND quick_hash = ?2",
                params![b.size, q],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((_, full, old_path, _)) = tomb else { continue };
        if let Some(fh) = full {
            match hash::full_hash(Path::new(&b.path), cancel) {
                Ok(h) if h == fh => {}
                Ok(_) => continue,
                Err(_) if cancelled() => {
                    rep.cancelled = true;
                    return Ok(rep);
                }
                Err(_) => continue,
            }
        }
        hits.push((b.clone(), old_path));
    }
    step(total);
    // Lấy lại bản ghi (băm nhanh vừa lưu) để kiểm tra file chưa đổi kể từ lúc băm.
    let mut books = Vec::new();
    for (b, old) in hits {
        if let Some(fresh) = db::get_book(conn, b.id)? {
            books.push((fresh, old));
        }
    }
    let items: Vec<TrashItem> = books
        .iter()
        .map(|(b, old)| TrashItem {
            book: b,
            expect: Some((b.size, b.mtime)),
            keeper: None,
            note: Some(format!("đã xoá trước đây: {old}")),
        })
        .collect();
    if !items.is_empty() {
        rep.trash = trash_items_with(conn, cfg, trasher, REJECT_KIND, &items, cancel, on)?;
        rep.cancelled = rep.trash.cancelled;
    }
    // Chỉ tiến mốc khi đã xử lý trọn vẹn: file bị khoá / lỗi thì lần quét sau xét lại.
    if !rep.cancelled && rep.trash.aborted.is_none() && rep.trash.failures.is_empty() {
        conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)", params![MARKER, upto.to_string()])?;
    }
    Ok(rep)
}
