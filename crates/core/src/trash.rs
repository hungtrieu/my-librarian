//! Xoá vào Thùng rác Windows (E5-01, E5-02, E5-06). Không bao giờ xoá vĩnh viễn.

use crate::config::Config;
use crate::db::{self, mtime_ms, Book};
use crate::error::{Error, Result};
use rusqlite::Connection;
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub trait Trasher {
    /// Đưa file vào Thùng rác. Lỗi = không chắc file đã vào Thùng rác.
    fn trash(&self, path: &Path) -> std::result::Result<(), String>;
}

/// Thùng rác của hệ điều hành (Windows: IFileOperation với cờ cho phép hoàn tác).
pub struct SystemTrasher;

impl Trasher for SystemTrasher {
    fn trash(&self, path: &Path) -> std::result::Result<(), String> {
        trash::delete(path).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TrashFailure {
    pub book_id: i64,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct TrashReport {
    pub batch_id: i64,
    pub trashed: usize,
    pub freed_bytes: u64,
    pub failures: Vec<TrashFailure>,
    /// Lô bị dừng giữa chừng vì không chắc file đã vào Thùng rác (E5-02).
    pub aborted: Option<String>,
    /// Người dùng bấm dừng (hoặc đóng app) giữa chừng: các file đã xoá vẫn được ghi nhật ký, phần còn lại giữ nguyên.
    pub cancelled: bool,
}

/// Bước báo tiến trình khi đưa từng file vào Thùng rác (x / y file).
pub const TRASH_PHASE: &str = "trash";

/// Một mục cần xoá; `expect` = (size, mtime) file phải còn khớp (E5-04), `keeper` = bản giữ phải còn nguyên.
pub struct TrashItem<'a> {
    pub book: &'a Book,
    pub expect: Option<(i64, i64)>,
    pub keeper: Option<&'a Book>,
    pub note: Option<String>,
}

fn file_matches(path: &Path, size: i64, mtime: i64) -> bool {
    match std::fs::metadata(path) {
        Ok(m) => m.is_file() && m.len() as i64 == size && m.modified().map(mtime_ms).ok() == Some(mtime),
        Err(_) => false,
    }
}

/// Xoá một lô vào Thùng rác, ghi nhật ký (E5-05).
pub fn trash_items(
    conn: &Connection,
    cfg: &Config,
    trasher: &dyn Trasher,
    kind: &str,
    items: &[TrashItem],
) -> Result<TrashReport> {
    trash_items_with(conn, cfg, trasher, kind, items, None, None)
}

/// Như [`trash_items`], báo tiến trình [`TRASH_PHASE`] x / y qua `on` và dừng được bằng `cancel` (kiểm tra trước
/// mỗi file). Mỗi file xoá xong được ghi DB ngay, nên dừng giữa chừng / tắt máy không làm sai lệch thư viện.
pub fn trash_items_with(
    conn: &Connection,
    cfg: &Config,
    trasher: &dyn Trasher,
    kind: &str,
    items: &[TrashItem],
    cancel: Option<&AtomicBool>,
    on: Option<crate::OnStep>,
) -> Result<TrashReport> {
    if cfg.read_only {
        return Err(Error::ReadOnly);
    }
    let batch = db::new_batch(conn, kind, None)?;
    let mut rep = TrashReport { batch_id: batch, ..Default::default() };
    let total = items.len();
    let step = |done: usize| {
        if let Some(f) = on {
            f(crate::Step { phase: TRASH_PHASE, done, total });
        }
    };
    step(0);
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            step(i);
        }
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            rep.cancelled = true;
            break;
        }
        let b = it.book;
        let p = Path::new(&b.path);
        let fail = |rep: &mut TrashReport, msg: String| -> Result<()> {
            db::log_op(conn, batch, "trash", Some(b.id), &b.path, None, Some(b.size), "failed", Some(&msg))?;
            rep.failures.push(TrashFailure { book_id: b.id, path: b.path.clone(), message: msg });
            Ok(())
        };
        if let Some(k) = it.keeper {
            let (ks, km) = (k.hash_size.unwrap_or(k.size), k.hash_mtime.unwrap_or(k.mtime));
            if !file_matches(Path::new(&k.path), ks, km) {
                fail(&mut rep, format!("Bản giữ lại đã thay đổi hoặc không còn: {}", k.path))?;
                continue;
            }
        }
        if let Some((s, m)) = it.expect {
            if !file_matches(p, s, m) {
                fail(&mut rep, "File đã thay đổi kể từ lúc băm – bỏ qua để an toàn".into())?;
                continue;
            }
        } else if !p.is_file() {
            fail(&mut rep, "File không còn tồn tại".into())?;
            continue;
        }
        crate::progress::working_on(p);
        // Sổ sách đã xoá: lấy băm nhanh TRƯỚC khi file biến mất (rẻ: dung lượng + 3 đoạn 64 KB).
        let quick = if crate::deleted::remembers(kind) { quick_of(conn, b) } else { None };
        match trasher.trash(p) {
            Ok(()) => {
                db::set_status(conn, b.id, "trashed")?;
                if crate::deleted::remembers(kind) {
                    crate::deleted::record(conn, b, quick.as_deref(), kind)?;
                }
                db::log_op(
                    conn,
                    batch,
                    "trash",
                    Some(b.id),
                    &b.path,
                    it.keeper.map(|k| k.path.as_str()),
                    Some(b.size),
                    "done",
                    it.note.as_deref(),
                )?;
                rep.trashed += 1;
                rep.freed_bytes += b.size as u64;
            }
            Err(e) if p.exists() => {
                // File còn nguyên (đang mở / bị khoá / không có quyền): báo lỗi, tiếp tục lô (E5-06).
                fail(&mut rep, format!("Không xoá được: {e}"))?;
            }
            Err(e) => {
                // File đã biến mất nhưng API báo lỗi → không chắc đã vào Thùng rác: dừng cả lô (E5-02).
                let msg = format!("Không chắc file đã vào Thùng rác ({e}); đã dừng cả lô");
                fail(&mut rep, msg.clone())?;
                db::set_status(conn, b.id, "missing")?;
                rep.aborted = Some(msg);
                break;
            }
        }
    }
    if !rep.cancelled && rep.aborted.is_none() {
        step(total);
    }
    Ok(rep)
}

/// Băm nhanh của file sắp xoá (dùng băm đã lưu nếu còn khớp), lưu lại vào bản ghi để khôi phục từ Thùng rác nhận
/// ra được đúng mục trong sổ.
fn quick_of(conn: &Connection, b: &Book) -> Option<String> {
    if let Some(q) = b.quick_hash.as_ref().filter(|_| b.hash_size == Some(b.size) && b.hash_mtime == Some(b.mtime)) {
        return Some(q.clone());
    }
    let q = crate::hash::quick_hash(Path::new(&b.path)).ok()?;
    let _ = conn.execute(
        "UPDATE books SET quick_hash = ?2, full_hash = NULL, hash_size = ?3, hash_mtime = ?4 WHERE id = ?1",
        rusqlite::params![b.id, q, b.size, b.mtime],
    );
    Some(q)
}

/// Xoá thẳng các sách được chọn vào Thùng rác (D7, E5-03).
pub fn trash_books(conn: &Connection, cfg: &Config, trasher: &dyn Trasher, ids: &[i64]) -> Result<TrashReport> {
    let books: Vec<Book> =
        ids.iter().filter_map(|id| db::get_book(conn, *id).transpose()).collect::<Result<Vec<_>>>()?;
    let items: Vec<TrashItem> = books
        .iter()
        .filter(|b| b.status == "present")
        .map(|b| TrashItem { book: b, expect: None, keeper: None, note: Some("xoá thủ công".into()) })
        .collect();
    trash_items(conn, cfg, trasher, "trash", &items)
}

/// Mở Thùng rác trong Explorer (E5-05).
pub fn open_recycle_bin() -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe").arg("shell:RecycleBinFolder").spawn()?;
    }
    Ok(())
}
