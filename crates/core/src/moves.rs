//! Di chuyển file thật + nhật ký + hoàn tác theo lô (E6-06, E6-07, E6-08). BRD 3.5:
//! không ghi đè; trùng tên thêm `(1)`; khác ổ thì copy → kiểm tra dung lượng → mới xoá nguồn.

use crate::config::Config;
use crate::db::{self, mtime_ms, now};
use crate::error::{Error, Result};
use crate::scan;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct MoveRequest {
    pub book_id: i64,
    pub target_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct MoveDone {
    pub book_id: i64,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MoveFailure {
    pub book_id: i64,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct MoveReport {
    pub batch_id: i64,
    pub moved: Vec<MoveDone>,
    pub failures: Vec<MoveFailure>,
}

/// `dir/name.ext`, hoặc `dir/name (1).ext`, `dir/name (2).ext`… nếu đã có.
pub fn unique_dest(dir: &Path, file_name: &str) -> PathBuf {
    let first = dir.join(file_name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match file_name.rfind('.') {
        Some(i) if i > 0 => (&file_name[..i], &file_name[i..]),
        _ => (file_name, ""),
    };
    (1..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap()
}

/// Di chuyển một file, không bao giờ ghi đè.
pub fn move_file(src: &Path, dst: &Path) -> io::Result<()> {
    if dst.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "đích đã tồn tại"));
    }
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => copy_then_delete(src, dst),
        Err(e) => Err(e),
    }
}

fn copy_then_delete(src: &Path, dst: &Path) -> io::Result<()> {
    let meta = std::fs::metadata(src)?;
    {
        let mut from = std::fs::File::open(src)?;
        let mut to = std::fs::File::create_new(dst)?;
        let r = io::copy(&mut from, &mut to).and_then(|_| to.sync_all());
        if let Err(e) = r {
            drop(to);
            let _ = std::fs::remove_file(dst);
            return Err(e);
        }
        if let Ok(m) = meta.modified() {
            let _ = to.set_modified(m);
        }
    }
    if std::fs::metadata(dst)?.len() != meta.len() {
        let _ = std::fs::remove_file(dst);
        return Err(io::Error::other("bản copy sai dung lượng – đã giữ nguyên file gốc"));
    }
    std::fs::remove_file(src)
}

fn ensure_writable(cfg: &Config) -> Result<()> {
    if cfg.read_only {
        Err(Error::ReadOnly)
    } else {
        Ok(())
    }
}

/// Áp dụng một lô di chuyển (sau khi người dùng bấm "Áp dụng" ở màn xem trước).
/// Thư mục đích phải đã tồn tại và nằm trong thư viện (D3: không tạo thư mục mới).
pub fn apply_moves(conn: &Connection, cfg: &Config, reqs: &[MoveRequest], kind: &str) -> Result<MoveReport> {
    ensure_writable(cfg)?;
    let lib = cfg.library_root.as_ref().ok_or(Error::NoLibrary)?;
    let batch = db::new_batch(conn, kind, None)?;
    let mut rep = MoveReport { batch_id: batch, ..Default::default() };
    for r in reqs {
        let Some(b) = db::get_book(conn, r.book_id)? else { continue };
        let fail = |rep: &mut MoveReport, msg: String| -> Result<()> {
            db::log_op(conn, batch, "move", Some(b.id), &b.path, None, Some(b.size), "failed", Some(&msg))?;
            rep.failures.push(MoveFailure { book_id: b.id, path: b.path.clone(), message: msg });
            Ok(())
        };
        if !r.target_dir.is_dir() {
            fail(&mut rep, format!("Thư mục đích không tồn tại: {}", r.target_dir.display()))?;
            continue;
        }
        if !r.target_dir.starts_with(lib) {
            fail(&mut rep, "Thư mục đích nằm ngoài thư viện".into())?;
            continue;
        }
        let src = PathBuf::from(&b.path);
        if src.parent() == Some(r.target_dir.as_path()) {
            continue; // đã ở đúng chỗ
        }
        if !src.is_file() {
            fail(&mut rep, "File nguồn không còn tồn tại".into())?;
            continue;
        }
        let dst = unique_dest(&r.target_dir, &b.file_name);
        match move_file(&src, &dst) {
            Ok(()) => {
                let tx = conn.unchecked_transaction()?;
                scan::relocate(&tx, cfg, b.id, &dst)?;
                if let Ok(m) = std::fs::metadata(&dst) {
                    let mt = m.modified().map(mtime_ms).unwrap_or(b.mtime);
                    tx.execute(
                        "UPDATE books SET mtime = ?2, hash_mtime = CASE WHEN hash_mtime = ?3 THEN ?2 ELSE hash_mtime END
                         WHERE id = ?1",
                        params![b.id, mt, b.mtime],
                    )?;
                }
                db::log_op(
                    &tx,
                    batch,
                    "move",
                    Some(b.id),
                    &b.path,
                    Some(&dst.to_string_lossy()),
                    Some(b.size),
                    "done",
                    None,
                )?;
                tx.commit()?;
                rep.moved.push(MoveDone {
                    book_id: b.id,
                    from: b.path.clone(),
                    to: dst.to_string_lossy().into_owned(),
                });
            }
            Err(e) => fail(&mut rep, format!("Không di chuyển được: {e}"))?,
        }
    }
    Ok(rep)
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct UndoReport {
    pub batch_id: i64,
    pub restored: usize,
    pub failures: Vec<MoveFailure>,
}

/// Hoàn tác một lô di chuyển: trả từng file về đúng chỗ cũ, báo rõ file nào không hoàn tác được.
pub fn undo_batch(conn: &Connection, cfg: &Config, batch_id: i64) -> Result<UndoReport> {
    ensure_writable(cfg)?;
    let (kind, undone): (String, Option<i64>) = conn
        .query_row("SELECT kind, undone_at FROM batches WHERE id = ?1", [batch_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|_| Error::msg(format!("Không có lô #{batch_id}")))?;
    if kind == "trash" || kind == "dedupe" {
        return Err(Error::msg(
            "Lô này đã đưa file vào Thùng rác – hãy khôi phục bằng Restore trong Thùng rác Windows, rồi quét lại",
        ));
    }
    if undone.is_some() {
        return Err(Error::msg(format!("Lô #{batch_id} đã được hoàn tác trước đó")));
    }
    let ops: Vec<(i64, Option<i64>, String, String)> = {
        let mut st = conn.prepare(
            "SELECT id, book_id, src, dst FROM operations
             WHERE batch_id = ?1 AND kind = 'move' AND status = 'done' AND dst IS NOT NULL ORDER BY id DESC",
        )?;
        let v = st
            .query_map([batch_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let mut rep = UndoReport { batch_id, ..Default::default() };
    for (op_id, book_id, src, dst) in ops {
        let (srcp, dstp) = (PathBuf::from(&src), PathBuf::from(&dst));
        let fail = |rep: &mut UndoReport, msg: String| -> Result<()> {
            conn.execute("UPDATE operations SET status='undo_failed', message=?2 WHERE id=?1", params![op_id, msg])?;
            rep.failures.push(MoveFailure { book_id: book_id.unwrap_or(0), path: dst.clone(), message: msg });
            Ok(())
        };
        if !dstp.is_file() {
            fail(&mut rep, "File không còn ở vị trí mới".into())?;
            continue;
        }
        if srcp.exists() {
            fail(&mut rep, "Vị trí cũ đã có file khác".into())?;
            continue;
        }
        if let Some(parent) = srcp.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                fail(&mut rep, format!("Không tạo lại được thư mục cũ: {e}"))?;
                continue;
            }
        }
        match move_file(&dstp, &srcp) {
            Ok(()) => {
                let tx = conn.unchecked_transaction()?;
                if let Some(id) = book_id {
                    scan::relocate(&tx, cfg, id, &srcp)?;
                }
                tx.execute("UPDATE operations SET status='undone' WHERE id=?1", [op_id])?;
                tx.commit()?;
                rep.restored += 1;
            }
            Err(e) => fail(&mut rep, format!("Không trả về được: {e}"))?,
        }
    }
    conn.execute("UPDATE batches SET undone_at = ?2 WHERE id = ?1", params![batch_id, now()])?;
    Ok(rep)
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchInfo {
    pub id: i64,
    pub kind: String,
    pub created_at: i64,
    pub undone_at: Option<i64>,
    pub done: i64,
    pub failed: i64,
    pub bytes: i64,
}

/// Màn "Hoạt động" (E5-05, E9-04).
pub fn list_batches(conn: &Connection, limit: usize) -> Result<Vec<BatchInfo>> {
    let mut st = conn.prepare(
        "SELECT b.id, b.kind, b.created_at, b.undone_at,
                COALESCE(SUM(o.status IN ('done','undone')), 0), COALESCE(SUM(o.status = 'failed'), 0),
                COALESCE(SUM(CASE WHEN o.status IN ('done','undone') THEN o.size END), 0)
         FROM batches b LEFT JOIN operations o ON o.batch_id = b.id
         GROUP BY b.id ORDER BY b.id DESC LIMIT ?1",
    )?;
    let v = st
        .query_map([limit as i64], |r| {
            Ok(BatchInfo {
                id: r.get(0)?,
                kind: r.get(1)?,
                created_at: r.get(2)?,
                undone_at: r.get(3)?,
                done: r.get(4)?,
                failed: r.get(5)?,
                bytes: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

#[derive(Debug, Clone, Serialize)]
pub struct Operation {
    pub id: i64,
    pub kind: String,
    pub book_id: Option<i64>,
    pub src: String,
    pub dst: Option<String>,
    pub size: Option<i64>,
    pub status: String,
    pub message: Option<String>,
    pub created_at: i64,
}

pub fn batch_operations(conn: &Connection, batch_id: i64) -> Result<Vec<Operation>> {
    let mut st = conn.prepare(
        "SELECT id, kind, book_id, src, dst, size, status, message, created_at FROM operations
         WHERE batch_id = ?1 ORDER BY id",
    )?;
    let v = st
        .query_map([batch_id], |r| {
            Ok(Operation {
                id: r.get(0)?,
                kind: r.get(1)?,
                book_id: r.get(2)?,
                src: r.get(3)?,
                dst: r.get(4)?,
                size: r.get(5)?,
                status: r.get(6)?,
                message: r.get(7)?,
                created_at: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}
