//! Quét thư mục (E1-02..E1-06): đệ quy, tăng dần theo size+mtime, bỏ qua ổ chưa cắm,
//! đánh dấu sách mất, nhận lại sách bị đổi tên/chuyển chỗ ngoài app.

use crate::config::Config;
use crate::db::{self, mtime_ms, now, Book, BOOK_COLS};
use crate::error::Result;
use crate::formats::{self, Format};
use crate::hash;
use crate::normalize::{fold, normalize_author, normalize_title, parse_volume};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use walkdir::{DirEntry, WalkDir};

/// Vị trí của file trong thư viện, suy ra từ đường dẫn (D10, D11, D16).
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub rel_path: String,
    /// Thư mục cấp 1 dưới `books` (thể loại) – chỉ với thư mục thư viện.
    pub category: Option<String>,
    pub in_inbox: bool,
    /// Tên thư mục bộ: file nằm ở `books\<thể loại>\...\<bộ>\file`.
    pub series_dir: Option<String>,
}

pub fn locate(cfg: &Config, root: &Path, root_kind: &str, path: &Path) -> Location {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let comps: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let rel_path = comps.join("\\");
    if root_kind != "library" || comps.len() < 2 {
        return Location { rel_path, category: None, in_inbox: false, series_dir: None };
    }
    let category = comps[0].clone();
    let in_inbox = fold(&category) == fold(&cfg.inbox_name);
    let series_dir = if comps.len() >= 3 { Some(comps[comps.len() - 2].clone()) } else { None };
    Location { rel_path, category: Some(category), in_inbox, series_dir }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ScanError {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Default, Clone, Serialize, serde::Deserialize)]
pub struct ScanSummary {
    pub scan_id: i64,
    pub files_seen: usize,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub missing: usize,
    pub recovered: usize,
    pub errors: Vec<ScanError>,
    /// Thư mục gốc không truy cập được (ổ ngoài chưa cắm) – E1-04, E8-07.
    pub skipped_roots: Vec<String>,
    pub cancelled: bool,
}

#[derive(Debug, Clone)]
pub struct ScanProgress<'a> {
    /// `scan`: đang duyệt file (`files_seen` / `total`) · `recover`: đang nhận lại sách bị đổi tên/chuyển chỗ
    /// ngoài app (`files_seen` = số cặp đã xét, `total` = số cặp cần xét).
    pub phase: &'static str,
    pub files_seen: usize,
    /// Tổng số file sách đếm được trước khi quét (0: đang đếm / chưa biết).
    pub total: usize,
    pub current: &'a Path,
}

/// Báo tiến trình tối thiểu mỗi khoảng này (ổ chậm: một file có thể mất hàng chục giây).
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// Lưu lô đang quét tối đa sau khoảng này, để tắt app giữa chừng không mất nhiều việc đã làm.
const COMMIT_EVERY: Duration = Duration::from_secs(2);

/// Đếm nhanh số file sách (chỉ liệt kê thư mục, không đọc nội dung) để hiện tổng cố định "x / y".
fn count_files(
    cfg: &Config,
    roots: &[(PathBuf, &str)],
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(&ScanProgress),
) -> usize {
    let mut n = 0;
    let mut last = Instant::now();
    for (root, _) in roots {
        if !root.is_dir() {
            continue;
        }
        let walker = WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| !excluded(cfg, e));
        for e in walker.flatten() {
            crate::progress::pulse();
            if cancelled() {
                return n;
            }
            if e.file_type().is_file() && Format::from_path(e.path()).is_some() {
                n += 1;
            }
            if last.elapsed() >= PROGRESS_EVERY {
                progress(&ScanProgress { phase: "count", files_seen: n, total: 0, current: e.path() });
                last = Instant::now();
            }
        }
    }
    n
}

#[derive(Default)]
pub struct ScanOptions<'a> {
    pub cancel: Option<&'a AtomicBool>,
    pub progress: Option<&'a dyn Fn(&ScanProgress)>,
    /// "app" hoặc "background".
    pub kind: Option<&'a str>,
}

fn is_hidden(e: &DirEntry) -> bool {
    if e.file_name().to_string_lossy().starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        if let Ok(m) = e.metadata() {
            if m.file_attributes() & (HIDDEN | SYSTEM) != 0 {
                return true;
            }
        }
    }
    false
}

fn excluded(cfg: &Config, e: &DirEntry) -> bool {
    if e.depth() == 0 {
        return false;
    }
    let name = e.file_name().to_string_lossy();
    cfg.exclude_names.iter().any(|x| x.eq_ignore_ascii_case(&name)) || is_hidden(e)
}

struct Record {
    title: String,
    title_from_meta: bool,
    authors: String,
    language: Option<String>,
    subjects: String,
    isbn: Option<String>,
    series: Option<String>,
    volume: Option<u32>,
    page_count: Option<u32>,
    drm: bool,
    norm_title: String,
    norm_author: String,
    error: Option<String>,
}

fn build_record(path: &Path, format: Format, loc: &Location) -> Record {
    let (md, error) = match formats::extract(path, format) {
        Ok(m) => (m, None),
        Err(e) => {
            tracing::warn!("metadata {}: {e}", path.display());
            (Default::default(), Some(e))
        }
    };
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let title_from_meta = md.title.is_some();
    let title = md.title.clone().unwrap_or_else(|| stem.clone());
    let series = loc.series_dir.clone().or(md.series.clone());
    let volume = if loc.series_dir.is_some() {
        parse_volume(&stem, loc.series_dir.as_deref()).or(md.volume)
    } else {
        md.volume.or_else(|| parse_volume(&stem, md.series.as_deref()))
    };
    let mut norm_title = normalize_title(&title);
    if norm_title.is_empty() {
        norm_title = normalize_title(&stem);
    }
    let authors = md.authors.join("; ");
    Record {
        norm_author: normalize_author(&md.authors.join(" ")),
        title,
        title_from_meta,
        authors,
        language: md.language,
        subjects: md.subjects.join("; "),
        isbn: md.isbn,
        series,
        volume,
        page_count: md.page_count,
        drm: md.drm,
        norm_title,
        error,
    }
}

/// Ghi (thêm mới hoặc cập nhật) một file vào DB. Trả về id.
#[allow(clippy::too_many_arguments)]
pub(crate) fn upsert_file(
    conn: &Connection,
    cfg: &Config,
    root_id: i64,
    root: &Path,
    root_kind: &str,
    path: &Path,
    size: i64,
    mtime: i64,
    scan_id: Option<i64>,
    existing: Option<i64>,
) -> Result<i64> {
    let format = Format::from_path(path).expect("đã lọc định dạng");
    let loc = locate(cfg, root, root_kind, path);
    let rec = build_record(path, format, &loc);
    let p = path.to_string_lossy();
    let file_name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let t = now();
    let id = if let Some(id) = existing {
        conn.execute(
            "UPDATE books SET root_id=?2, path=?3, rel_path=?4, file_name=?5, format=?6, size=?7, mtime=?8,
               title=?9, title_from_meta=?10, authors=?11, language=?12, subjects=?13, isbn=?14, series=?15,
               volume=?16, page_count=?17, drm=?18, norm_title=?19, norm_author=?20, category=?21, in_inbox=?22,
               quick_hash=NULL, full_hash=NULL, hash_size=NULL, hash_mtime=NULL, status='present', error=?23,
               last_seen_scan=COALESCE(?24, last_seen_scan), updated_at=?25
             WHERE id=?1",
            params![
                id,
                root_id,
                p,
                loc.rel_path,
                file_name,
                format.as_str(),
                size,
                mtime,
                rec.title,
                rec.title_from_meta,
                rec.authors,
                rec.language,
                rec.subjects,
                rec.isbn,
                rec.series,
                rec.volume,
                rec.page_count,
                rec.drm,
                rec.norm_title,
                rec.norm_author,
                loc.category,
                loc.in_inbox,
                rec.error,
                scan_id,
                t
            ],
        )?;
        id
    } else {
        conn.execute(
            "INSERT INTO books(root_id, path, rel_path, file_name, format, size, mtime, title, title_from_meta,
               authors, language, subjects, isbn, series, volume, page_count, drm, norm_title, norm_author,
               category, in_inbox, status, error, first_seen_scan, last_seen_scan, added_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,'present',?22,?23,?23,?24,?24)",
            params![
                root_id, p, loc.rel_path, file_name, format.as_str(), size, mtime, rec.title,
                rec.title_from_meta, rec.authors, rec.language, rec.subjects, rec.isbn, rec.series,
                rec.volume, rec.page_count, rec.drm, rec.norm_title, rec.norm_author, loc.category,
                loc.in_inbox, rec.error, scan_id, t
            ],
        )?;
        conn.last_insert_rowid()
    };
    db::fts_upsert(conn, id)?;
    Ok(id)
}

/// Cập nhật vị trí (sau khi di chuyển trong app) mà không đọc lại metadata.
pub(crate) fn relocate(conn: &Connection, cfg: &Config, id: i64, new_path: &Path) -> Result<()> {
    let (root_id, root, kind) = root_for(conn, new_path)?
        .ok_or_else(|| crate::Error::msg(format!("{} không nằm trong thư mục gốc nào", new_path.display())))?;
    let loc = locate(cfg, &root, &kind, new_path);
    let file_name = new_path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = new_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let (old_series, old_volume): (Option<String>, Option<i64>) =
        conn.query_row("SELECT series, volume FROM books WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let series = loc.series_dir.clone().or(old_series);
    let volume = match &loc.series_dir {
        Some(s) => parse_volume(&stem, Some(s)).map(|v| v as i64).or(old_volume),
        None => old_volume,
    };
    conn.execute(
        "UPDATE books SET root_id=?2, path=?3, rel_path=?4, file_name=?5, category=?6, in_inbox=?7,
           series=?8, volume=?9, status='present', updated_at=?10 WHERE id=?1",
        params![
            id,
            root_id,
            new_path.to_string_lossy(),
            loc.rel_path,
            file_name,
            loc.category,
            loc.in_inbox,
            series,
            volume,
            now()
        ],
    )?;
    db::fts_upsert(conn, id)
}

/// Thư mục gốc (đã đăng ký) chứa `path`.
pub(crate) fn root_for(conn: &Connection, path: &Path) -> Result<Option<(i64, PathBuf, String)>> {
    let mut st = conn.prepare("SELECT id, path, kind FROM roots")?;
    let roots = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, PathBuf::from(r.get::<_, String>(1)?), r.get::<_, String>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(roots.into_iter().filter(|(_, p, _)| path.starts_with(p)).max_by_key(|(_, p, _)| p.components().count()))
}

pub fn scan(conn: &Connection, cfg: &Config, opts: &ScanOptions) -> Result<ScanSummary> {
    let roots = cfg.roots();
    if roots.is_empty() {
        return Err(crate::Error::NoLibrary);
    }
    let kind = opts.kind.unwrap_or("app");
    conn.execute(
        "INSERT INTO scan_reports(kind, started_at, status) VALUES (?1, ?2, 'running')",
        params![kind, now()],
    )?;
    let scan_id = conn.last_insert_rowid();
    let mut sum = ScanSummary { scan_id, ..Default::default() };
    let cancelled = || opts.cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    let total = if let Some(cb) = opts.progress {
        cb(&ScanProgress { phase: "count", files_seen: 0, total: 0, current: Path::new("") });
        let n = count_files(cfg, &roots, &cancelled, cb);
        cb(&ScanProgress { phase: "scan", files_seen: 0, total: n, current: Path::new("") });
        n
    } else {
        0
    };
    let mut last_progress = Instant::now();

    'roots: for (root, root_kind) in roots {
        let root_str = root.to_string_lossy().into_owned();
        if !root.is_dir() {
            tracing::warn!("bỏ qua {root_str}: ổ/thư mục chưa kết nối");
            sum.skipped_roots.push(root_str);
            continue;
        }
        let root_id = db::ensure_root(conn, &root_str, root_kind)?;
        let mut tx = Some(crate::db::write_tx(conn)?);
        let mut in_tx = 0usize;
        let mut tx_started = Instant::now();
        let walker = WalkDir::new(&root).follow_links(false).into_iter().filter_entry(|e| !excluded(cfg, e));
        for entry in walker {
            crate::progress::pulse();
            if cancelled() {
                sum.cancelled = true;
                tx.take().unwrap().commit()?;
                break 'roots;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    let path = e.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                    sum.errors.push(ScanError { path, message: e.to_string() });
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Some(_format) = Format::from_path(path) else { continue };
            crate::progress::working_on(path);
            sum.files_seen += 1;
            if let Some(cb) = opts.progress {
                if sum.files_seen % 200 == 1 || last_progress.elapsed() >= PROGRESS_EVERY {
                    cb(&ScanProgress { phase: "scan", files_seen: sum.files_seen, total, current: path });
                    last_progress = Instant::now();
                }
            }
            if path.to_str().is_none() {
                sum.errors.push(ScanError {
                    path: path.to_string_lossy().into_owned(),
                    message: "tên file không phải Unicode hợp lệ".into(),
                });
                continue;
            }
            let md = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    sum.errors.push(ScanError { path: path.to_string_lossy().into_owned(), message: e.to_string() });
                    continue;
                }
            };
            let size = md.len() as i64;
            let mtime = md.modified().map(mtime_ms).unwrap_or(0);
            let c: &Connection = tx.as_ref().unwrap();
            let existing: Option<(i64, i64, i64, String)> = c
                .query_row("SELECT id, size, mtime, status FROM books WHERE path = ?1", [path.to_string_lossy()], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .optional()?;
            match existing {
                Some((id, s, m, st)) if s == size && m == mtime && st == "present" => {
                    c.execute("UPDATE books SET last_seen_scan = ?2 WHERE id = ?1", params![id, scan_id])?;
                    sum.unchanged += 1;
                }
                Some((id, _, _, _)) => {
                    upsert_file(c, cfg, root_id, &root, root_kind, path, size, mtime, Some(scan_id), Some(id))?;
                    sum.updated += 1;
                }
                None => {
                    upsert_file(c, cfg, root_id, &root, root_kind, path, size, mtime, Some(scan_id), None)?;
                    sum.added += 1;
                }
            }
            in_tx += 1;
            if in_tx >= 500 || tx_started.elapsed() >= COMMIT_EVERY {
                tx.take().unwrap().commit()?;
                tx = Some(crate::db::write_tx(conn)?);
                in_tx = 0;
                tx_started = Instant::now();
            }
        }
        tx.take().unwrap().commit()?;

        // Ổ bị rút giữa chừng → không đánh dấu mất (E1-04).
        if !root.is_dir() {
            sum.skipped_roots.push(root_str);
            continue;
        }
        let gone: Vec<i64> = {
            let mut st = conn.prepare(
                "SELECT id FROM books WHERE root_id = ?1 AND status = 'present'
                 AND (last_seen_scan IS NULL OR last_seen_scan < ?2)",
            )?;
            let v = st.query_map(params![root_id, scan_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            v
        };
        let tx = crate::db::write_tx(conn)?;
        for id in &gone {
            crate::progress::pulse();
            db::set_status(&tx, *id, "missing")?;
        }
        tx.commit()?;
        sum.missing += gone.len();
    }

    if !sum.cancelled {
        let (n, stopped) = recover_moved(conn, scan_id, opts.cancel, opts.progress)?;
        sum.recovered = n;
        sum.cancelled = stopped;
        sum.missing = sum.missing.saturating_sub(sum.recovered);
    }
    let status = if sum.cancelled { "cancelled" } else { "done" };
    conn.execute(
        "UPDATE scan_reports SET finished_at = ?2, status = ?3, stats = ?4 WHERE id = ?1",
        params![scan_id, now(), status, serde_json::to_string(&sum)?],
    )?;
    Ok(sum)
}

/// E1-05: sách "mất" + sách "mới" cùng nội dung → là cùng một cuốn bị đổi tên/chuyển chỗ.
/// Giữ bản ghi cũ (tiến độ đọc, quyết định duyệt), chuyển sang đường dẫn mới.
/// Sách "mới" tính từ lần quét hoàn tất gần nhất (kể cả lần quét bị tắt/huỷ giữa chừng), để huỷ ở bước này
/// thì lần sau vẫn nhận lại tiếp được. Trả về (số sách nhận lại, có bị huỷ không).
fn recover_moved(
    conn: &Connection,
    scan_id: i64,
    cancel: Option<&AtomicBool>,
    progress: Option<&dyn Fn(&ScanProgress)>,
) -> Result<(usize, bool)> {
    let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    let pairs: Vec<(i64, i64)> = {
        let mut st = conn.prepare(
            "SELECT n.id, m.id FROM books n JOIN books m ON m.size = n.size AND m.status = 'missing'
             WHERE n.first_seen_scan > (SELECT COALESCE(MAX(id), 0) FROM scan_reports WHERE status = 'done' AND id < ?1)
               AND n.first_seen_scan <= ?1 AND n.status = 'present'
             ORDER BY n.id, m.id",
        )?;
        let v = st.query_map([scan_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let mut used_new = HashSet::new();
    let mut used_old = HashSet::new();
    let mut n_recovered = 0;
    let total = pairs.len();
    let mut last_progress: Option<Instant> = None;
    for (i, (nid, mid)) in pairs.into_iter().enumerate() {
        if cancelled() {
            return Ok((n_recovered, true));
        }
        if let Some(cb) = progress {
            if last_progress.is_none_or(|t| t.elapsed() >= PROGRESS_EVERY) {
                cb(&ScanProgress { phase: "recover", files_seen: i, total, current: Path::new("") });
                last_progress = Some(Instant::now());
            }
        }
        if used_new.contains(&nid) || used_old.contains(&mid) {
            continue;
        }
        let (Some(n), Some(m)) = (db::get_book(conn, nid)?, db::get_book(conn, mid)?) else { continue };
        crate::progress::working_on(Path::new(&n.path));
        let same = if let Some(mh) = &m.full_hash {
            hash::full_hash(Path::new(&n.path), cancel).map(|h| &h == mh).unwrap_or(false)
        } else if let Some(mq) = m.quick_hash.as_ref().filter(|_| m.hash_size == Some(m.size)) {
            n.mtime == m.mtime && hash::quick_hash(Path::new(&n.path)).map(|h| &h == mq).unwrap_or(false)
        } else {
            n.mtime == m.mtime && (n.file_name == m.file_name || n.norm_title == m.norm_title)
        };
        if !same {
            continue;
        }
        merge_into(conn, &n, &m)?;
        used_new.insert(nid);
        used_old.insert(mid);
        n_recovered += 1;
    }
    Ok((n_recovered, false))
}

fn merge_into(conn: &Connection, new: &Book, old: &Book) -> Result<()> {
    let tx = crate::db::write_tx(conn)?;
    db::fts_delete(&tx, new.id)?;
    let first_seen: Option<i64> =
        tx.query_row("SELECT last_seen_scan FROM books WHERE id=?1", [new.id], |r| r.get(0))?;
    tx.execute("DELETE FROM books WHERE id = ?1", [new.id])?;
    tx.execute(
        "UPDATE books SET root_id=?2, path=?3, rel_path=?4, file_name=?5, mtime=?6, category=?7, in_inbox=?8,
           series=?9, volume=?10, status='present', last_seen_scan=?11, updated_at=?12 WHERE id=?1",
        params![
            old.id,
            new.root_id,
            new.path,
            new.rel_path,
            new.file_name,
            new.mtime,
            new.category,
            new.in_inbox,
            new.series,
            new.volume,
            first_seen,
            now()
        ],
    )?;
    db::fts_upsert(&tx, old.id)?;
    tx.commit()?;
    tracing::info!("nhận lại sách đổi tên: {} → {}", old.path, new.path);
    Ok(())
}

/// Sách mọi trạng thái theo đường dẫn (tiện cho test/CLI).
pub fn book_by_path(conn: &Connection, path: &Path) -> Result<Option<Book>> {
    Ok(conn
        .query_row(&format!("SELECT {BOOK_COLS} FROM books WHERE path = ?1"), [path.to_string_lossy()], Book::from_row)
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locate_rules() {
        let cfg = Config { library_root: Some(PathBuf::from("/b")), ..Default::default() };
        let root = Path::new("/b");
        let l = locate(&cfg, root, "library", Path::new("/b/truyen tranh/7 seeds/7 Seeds 4.mobi"));
        assert_eq!(l.category.as_deref(), Some("truyen tranh"));
        assert_eq!(l.series_dir.as_deref(), Some("7 seeds"));
        assert!(!l.in_inbox);
        let l = locate(&cfg, root, "library", Path::new("/b/Chua phan loai/x.epub"));
        assert!(l.in_inbox, "so khớp inbox bỏ dấu");
        let l = locate(&cfg, root, "library", Path::new("/b/x.epub"));
        assert_eq!(l.category, None);
        let l = locate(&cfg, root, "source", Path::new("/b/a/x.epub"));
        assert_eq!(l.category, None);
    }
}
