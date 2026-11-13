//! Băm nhiều tầng (E4-01): lọc theo dung lượng → băm nhanh đầu/giữa/cuối → SHA-256 đầy đủ chỉ cho ứng viên.

use crate::db::now;
use crate::error::Result;
use crate::{OnStep, Step};
use rayon::prelude::*;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const CHUNK: u64 = 64 * 1024;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Băm nhanh: dung lượng + 64 KB đầu + 64 KB giữa + 64 KB cuối.
pub fn quick_hash(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    let mut h = Sha256::new();
    h.update(size.to_le_bytes());
    let mut buf = vec![0u8; CHUNK as usize];
    if size <= CHUNK * 3 {
        let mut all = Vec::with_capacity(size as usize);
        f.read_to_end(&mut all)?;
        h.update(&all);
    } else {
        for off in [0, size / 2 - CHUNK / 2, size - CHUNK] {
            f.seek(SeekFrom::Start(off))?;
            f.read_exact(&mut buf)?;
            h.update(&buf);
        }
    }
    Ok(hex(&h.finalize()[..16]))
}

/// SHA-256 toàn bộ file (BRD R4).
pub fn full_hash(path: &Path, cancel: Option<&AtomicBool>) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "đã huỷ"));
        }
        let n = f.read(&mut buf)?;
        crate::progress::pulse(); // file lớn trên ổ chậm: mỗi MB một nhịp
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct HashStats {
    pub size_candidates: usize,
    pub quick_hashed: usize,
    pub full_candidates: usize,
    pub full_hashed: usize,
    pub bytes_full_hashed: u64,
    pub errors: Vec<(String, String)>,
    /// Dừng giữa chừng vì bị huỷ (phần đã băm vẫn được lưu).
    pub cancelled: bool,
}

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(threads.max(1)).build().expect("thread pool")
}

/// Đảm bảo mọi file "có khả năng trùng" đã được băm. Kết quả lưu vào DB, kèm size/mtime lúc băm.
pub fn ensure_hashes(conn: &Connection, threads: usize, cancel: Option<&AtomicBool>) -> Result<HashStats> {
    ensure_hashes_with(conn, threads, cancel, None)
}

/// Như [`ensure_hashes`], báo tiến trình qua `on`. Băm theo từng lô nhỏ và **ghi DB sau mỗi lô**: tắt máy / huỷ
/// giữa chừng thì phần đã băm vẫn còn, lần sau chỉ băm phần còn lại (file đã có băm khớp size/mtime được bỏ qua).
pub fn ensure_hashes_with(
    conn: &Connection,
    threads: usize,
    cancel: Option<&AtomicBool>,
    on: Option<OnStep>,
) -> Result<HashStats> {
    let mut stats = HashStats::default();
    let pool = pool(threads);
    let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    let report = |phase: &'static str, done: usize, total: usize| {
        if let Some(f) = on {
            f(Step { phase, done, total });
        }
    };

    // Tầng 1: cùng dung lượng.
    let need_quick: Vec<(i64, String, i64, i64)> = {
        let mut st = conn.prepare(
            "SELECT id, path, size, mtime FROM books
             WHERE status = 'present' AND size > 0
               AND size IN (SELECT size FROM books WHERE status = 'present' AND size > 0
                            GROUP BY size HAVING COUNT(*) > 1)
               AND (quick_hash IS NULL OR hash_size IS NOT size OR hash_mtime IS NOT mtime)",
        )?;
        let v = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    stats.size_candidates = conn.query_row(
        "SELECT COUNT(*) FROM books WHERE status='present' AND size > 0 AND size IN
         (SELECT size FROM books WHERE status='present' AND size > 0 GROUP BY size HAVING COUNT(*) > 1)",
        [],
        |r| r.get::<_, i64>(0),
    )? as usize;

    if !quick_batches(conn, &pool, &need_quick, threads, cancel, "hash_quick", on, &mut stats)? {
        stats.cancelled = true;
        return Ok(stats);
    }

    // Tầng 2: cùng (dung lượng, băm nhanh) → SHA-256 đầy đủ.
    let need_full: Vec<(i64, String, i64)> = {
        let mut st = conn.prepare(
            "SELECT b.id, b.path, b.size FROM books b
             JOIN (SELECT size, quick_hash FROM books
                   WHERE status = 'present' AND quick_hash IS NOT NULL AND hash_size = size AND hash_mtime = mtime
                   GROUP BY size, quick_hash HAVING COUNT(*) > 1) g
               ON g.size = b.size AND g.quick_hash = b.quick_hash
             WHERE b.status = 'present' AND b.hash_size = b.size AND b.hash_mtime = b.mtime",
        )?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    stats.full_candidates = need_full.len();
    let todo: Vec<_> = {
        let mut st = conn.prepare("SELECT full_hash IS NOT NULL FROM books WHERE id = ?1")?;
        need_full
            .into_iter()
            .filter(|(id, _, _)| !st.query_row([id], |r| r.get::<_, bool>(0)).unwrap_or(false))
            .collect()
    };
    let total = todo.len();
    report("hash_full", 0, total);
    let mut done = 0;
    // File lớn → lô nhỏ để lưu thường xuyên.
    for chunk in todo.chunks(threads.max(1) * 2) {
        if cancelled() {
            stats.cancelled = true;
            return Ok(stats);
        }
        let results: Vec<_> = pool.install(|| {
            chunk
                .par_iter()
                .map(|(id, p, size)| {
                    crate::progress::working_on(Path::new(p));
                    (*id, p, *size, full_hash(Path::new(p), cancel))
                })
                .collect()
        });
        let tx = crate::db::write_tx(conn)?;
        for (id, p, size, r) in results {
            match r {
                Ok(h) => {
                    tx.execute(
                        "UPDATE books SET full_hash = ?2, updated_at = ?3 WHERE id = ?1",
                        params![id, h, now()],
                    )?;
                    stats.full_hashed += 1;
                    stats.bytes_full_hashed += size as u64;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => stats.errors.push((p.clone(), e.to_string())),
            }
        }
        tx.commit()?;
        done += chunk.len();
        report("hash_full", done, total);
    }
    if cancelled() {
        stats.cancelled = true;
    }
    Ok(stats)
}

/// Băm nhanh `items` (id, đường dẫn, dung lượng, mtime) song song theo lô, **ghi DB sau mỗi lô** và báo tiến
/// trình `phase` x / y. Trả về `false` nếu bị huỷ giữa chừng (các lô đã xong vẫn được lưu).
#[allow(clippy::too_many_arguments)]
fn quick_batches(
    conn: &Connection,
    pool: &rayon::ThreadPool,
    items: &[(i64, String, i64, i64)],
    threads: usize,
    cancel: Option<&AtomicBool>,
    phase: &'static str,
    on: Option<OnStep>,
    stats: &mut HashStats,
) -> Result<bool> {
    let report = |done: usize, total: usize| {
        if let Some(f) = on {
            f(Step { phase, done, total });
        }
    };
    let total = items.len();
    report(0, total);
    let mut done = 0;
    for chunk in items.chunks(threads.max(1) * 4) {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Ok(false);
        }
        let results: Vec<_> = pool.install(|| {
            chunk
                .par_iter()
                .map(|(id, p, size, mtime)| {
                    crate::progress::working_on(Path::new(p));
                    let r = quick_hash(Path::new(p)).map_err(|e| e.to_string());
                    crate::progress::pulse();
                    (*id, p, *size, *mtime, r)
                })
                .collect()
        });
        let tx = crate::db::write_tx(conn)?;
        for (id, p, size, mtime, r) in results {
            match r {
                Ok(q) => {
                    tx.execute(
                        "UPDATE books SET quick_hash = ?2, full_hash = NULL, hash_size = ?3, hash_mtime = ?4,
                         updated_at = ?5 WHERE id = ?1",
                        params![id, q, size, mtime, now()],
                    )?;
                    stats.quick_hashed += 1;
                }
                Err(e) => stats.errors.push((p.clone(), e)),
            }
        }
        tx.commit()?;
        done += chunk.len();
        report(done, total);
    }
    Ok(true)
}

/// Đảm bảo các sách `ids` có băm nhanh còn hợp lệ (khớp size/mtime): chỉ băm phần còn thiếu – song song, ghi DB
/// theo lô, báo tiến trình `phase`. Huỷ giữa chừng thì phần đã băm vẫn được lưu, lần sau chỉ băm phần còn lại.
/// Trả về `false` nếu bị huỷ.
pub fn ensure_quick_many(
    conn: &Connection,
    ids: &[i64],
    threads: usize,
    cancel: Option<&AtomicBool>,
    phase: &'static str,
    on: Option<OnStep>,
) -> Result<bool> {
    let mut need: Vec<(i64, String, i64, i64)> = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT path, size, mtime FROM books
             WHERE id = ?1 AND (quick_hash IS NULL OR hash_size IS NOT size OR hash_mtime IS NOT mtime)",
        )?;
        for id in ids {
            crate::progress::pulse();
            if let Some((p, s, m)) = st
                .query_row([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))
                .optional()?
            {
                need.push((*id, p, s, m));
            }
        }
    }
    let mut stats = HashStats::default();
    quick_batches(conn, &pool(threads), &need, threads, cancel, phase, on, &mut stats)
}

/// Băm nhanh theo yêu cầu (vd. cho thành viên nhóm nghi trùng chưa có băm), lưu lại DB.
pub fn ensure_quick(conn: &Connection, id: i64) -> Result<Option<String>> {
    let (path, size, mtime, q, hs, hm): (String, i64, i64, Option<String>, Option<i64>, Option<i64>) = conn.query_row(
        "SELECT path, size, mtime, quick_hash, hash_size, hash_mtime FROM books WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
    )?;
    if let Some(q) = q {
        if hs == Some(size) && hm == Some(mtime) {
            return Ok(Some(q));
        }
    }
    match quick_hash(Path::new(&path)) {
        Ok(q) => {
            conn.execute(
                "UPDATE books SET quick_hash = ?2, full_hash = NULL, hash_size = ?3, hash_mtime = ?4 WHERE id = ?1",
                params![id, q, size, mtime],
            )?;
            Ok(Some(q))
        }
        Err(_) => Ok(None),
    }
}
