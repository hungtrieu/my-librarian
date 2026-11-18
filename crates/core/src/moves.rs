//! Di chuyển file thật + nhật ký + hoàn tác theo lô (E6-06, E6-07, E6-08). BRD 3.5:
//! không ghi đè; trùng tên thêm `(1)`; khác ổ thì copy → kiểm tra dung lượng → mới xoá nguồn.

use crate::config::Config;
use crate::db::{self, mtime_ms, now};
use crate::error::{Error, Result};
use crate::scan;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Ghi DB sau mỗi chừng này file đã chuyển…
const COMMIT_EVERY_FILES: usize = 64;
/// …hoặc sau chừng này thời gian.
const COMMIT_EVERY: Duration = Duration::from_millis(300);

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
    /// Dừng giữa chừng (phần đã chuyển giữ nguyên, hoàn tác được).
    pub cancelled: bool,
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
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => copy_verify_replace(src, dst),
        Err(e) => Err(e),
    }
}

/// Chép từng khúc 4 MB, vừa chép vừa băm SHA-256; mỗi khúc một nhịp cho bộ giám sát treo (chép file vài GB sang ổ
/// USB có thể mất vài phút).
fn copy_pulsing(from: &mut impl io::Read, to: &mut impl io::Write) -> io::Result<(u64, [u8; 32])> {
    use sha2::Digest;
    let mut buf = vec![0u8; 4 << 20];
    let mut n = 0u64;
    let mut h = sha2::Sha256::new();
    loop {
        let k = match from.read(&mut buf) {
            Ok(0) => return Ok((n, h.finalize().into())),
            Ok(k) => k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        to.write_all(&buf[..k])?;
        h.update(&buf[..k]);
        n += k as u64;
        crate::progress::pulse();
    }
}

/// Tên file tạm (ẩn – quét bỏ qua) khi chép `dst` sang ổ khác.
pub fn temp_copy_path(dst: &Path) -> PathBuf {
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dst.with_file_name(format!(".mylib-dang-chep-{name}"))
}

/// Ghi chú "đang chép dở" (đường dẫn file tạm) trong thư mục dữ liệu app: mất điện / app bị tắt giữa lúc chép thì
/// lần sau [`recover_interrupted_copy`] dọn file tạm còn sót. Mỗi lúc chỉ chép một file.
fn inflight_marker() -> PathBuf {
    crate::config::default_home().join("copy-inflight.txt")
}

/// Dọn file tạm của lần chép sang ổ khác bị ngắt giữa chừng (bản gốc chưa bị xoá nên không mất gì). Gọi trước mỗi
/// lần chép và khi bắt đầu quét.
pub fn recover_interrupted_copy() {
    let m = inflight_marker();
    let Ok(tmp) = std::fs::read_to_string(&m) else { return };
    let tmp = PathBuf::from(tmp.trim());
    if tmp.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".mylib-dang-chep-")) && tmp.is_file() {
        match std::fs::remove_file(&tmp) {
            Ok(()) => tracing::warn!("dọn file chép dở từ lần trước: {}", tmp.display()),
            Err(e) => {
                tracing::warn!("không dọn được file chép dở {}: {e}", tmp.display());
                return;
            }
        }
    }
    let _ = std::fs::remove_file(m);
}

/// Chép sang ổ khác một cách an toàn rồi mới xoá bản gốc:
/// 1. chép vào file tạm ẩn cạnh đích (vừa chép vừa băm SHA-256), `fsync`;
/// 2. đọc lại file tạm, so SHA-256 và dung lượng với bản gốc – lệch (ổ USB lỗi, cáp lỏng) thì bỏ file tạm, bản gốc
///    giữ nguyên;
/// 3. đổi tên file tạm thành tên thật (trong cùng thư mục: tức thì, không bao giờ để lại file đích dở dang);
/// 4. xoá bản gốc.
///
/// Ngắt ở bất kỳ bước nào cũng không mất / hỏng sách: trước bước 3 chỉ có file tạm (bị bỏ qua khi quét, được dọn ở
/// lần sau); sau bước 3 có hai bản y hệt (hiện ở "Trùng chắc chắn").
pub fn copy_verify_replace(src: &Path, dst: &Path) -> io::Result<()> {
    recover_interrupted_copy();
    let meta = std::fs::metadata(src)?;
    let tmp = temp_copy_path(dst);
    if tmp.exists() {
        std::fs::remove_file(&tmp)?;
    }
    let marker = inflight_marker();
    if let Some(d) = marker.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&marker, tmp.to_string_lossy().as_bytes());
    let cleanup = |e: io::Error| -> io::Error {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&marker);
        e
    };
    let src_hash = {
        let mut from = std::fs::File::open(src)?;
        let mut to = std::fs::File::create_new(&tmp).map_err(cleanup)?;
        let r = copy_pulsing(&mut from, &mut to).and_then(|(n, h)| to.sync_all().map(|_| (n, h)));
        let (n, h) = match r {
            Ok(x) => x,
            Err(e) => {
                drop(to);
                return Err(cleanup(e));
            }
        };
        if let Ok(m) = meta.modified() {
            let _ = to.set_modified(m);
        }
        if n != meta.len() {
            drop(to);
            return Err(cleanup(io::Error::other("đọc bản gốc bị thiếu dữ liệu – đã giữ nguyên file gốc")));
        }
        h
    };
    let copy_hash = {
        let mut f = std::fs::File::open(&tmp).map_err(cleanup)?;
        match copy_pulsing(&mut f, &mut io::sink()) {
            Ok((n, h)) if n == meta.len() => h,
            Ok(_) => return Err(cleanup(io::Error::other("bản chép sai dung lượng – đã giữ nguyên file gốc"))),
            Err(e) => return Err(cleanup(e)),
        }
    };
    if copy_hash != src_hash {
        return Err(cleanup(io::Error::other("bản chép khác bản gốc (lỗi ổ / cáp?) – đã giữ nguyên file gốc")));
    }
    if dst.exists() {
        return Err(cleanup(io::Error::new(io::ErrorKind::AlreadyExists, "đích đã tồn tại")));
    }
    std::fs::rename(&tmp, dst).map_err(cleanup)?;
    let _ = std::fs::remove_file(&marker);
    std::fs::remove_file(src)
}

fn ensure_writable(cfg: &Config) -> Result<()> {
    if cfg.read_only {
        Err(Error::ReadOnly)
    } else {
        Ok(())
    }
}

/// Tuỳ chọn cho [`apply_moves_with`].
#[derive(Default)]
pub struct MoveOptions<'a> {
    /// Chuyển đúng vào thư mục yêu cầu (cân bằng ổ). Mặc định: chọn ổ tiết kiệm bằng [`crate::volumes::Placer`] –
    /// sách ở lại ổ đang nằm (đổi tên tức thì), trừ khi ổ đó sắp đầy.
    pub exact: bool,
    /// Dừng trước file tiếp theo.
    pub cancel: Option<&'a std::sync::atomic::AtomicBool>,
    /// Tiến trình (`phase = "move"`, x / y file).
    pub progress: Option<crate::OnStep<'a>>,
    /// Tên file mới cho một số cuốn (mã sách → tên), vd. khi làm phẳng thư mục con (`subcats.rs`).
    pub names: Option<&'a HashMap<i64, String>>,
}

/// Áp dụng một lô di chuyển (sau khi người dùng bấm "Áp dụng" ở màn xem trước).
/// Thư mục đích phải nằm trong thư viện (một trong các ổ thư viện).
pub fn apply_moves(conn: &Connection, cfg: &Config, reqs: &[MoveRequest], kind: &str) -> Result<MoveReport> {
    apply_moves_with(conn, cfg, reqs, kind, &MoveOptions::default())
}

/// Như [`apply_moves`], kèm [`MoveOptions`].
pub fn apply_moves_with(
    conn: &Connection,
    cfg: &Config,
    reqs: &[MoveRequest],
    kind: &str,
    opts: &MoveOptions,
) -> Result<MoveReport> {
    ensure_writable(cfg)?;
    if !cfg.library_online() {
        return Err(Error::NoLibrary);
    }
    let batch = db::new_batch(conn, kind, None)?;
    let mut rep = MoveReport { batch_id: batch, ..Default::default() };
    let mut placer = crate::volumes::Placer::new(cfg);
    let step = |done: usize| {
        if let Some(f) = opts.progress {
            f(crate::Step { phase: "move", done, total: reqs.len() });
        }
    };
    // Mỗi cuốn trước đây phải đọc lại thư mục thư viện / thư mục đích (tìm đúng tên thật trên ổ) và mọi ngăn của
    // `Khác`: với ổ USB / ổ mạng (vài ms mỗi lần hỏi đĩa) và hàng nghìn cuốn là rất chậm. Kết quả giống nhau cho mọi
    // cuốn cùng ổ, cùng đích → nhớ lại.
    let mut placed: HashMap<(Option<usize>, PathBuf), PathBuf> = HashMap::new();
    let mut made_dirs: HashSet<PathBuf> = HashSet::new();
    let mut shelves: HashMap<PathBuf, crate::triage::Shelves> = HashMap::new();
    let roots: Vec<(i64, PathBuf, String)> = scan::all_roots(conn)?;
    // Ghi DB theo lô nhỏ (một giao dịch cho vài chục file) thay vì một giao dịch mỗi file. File đã chuyển mà chưa kịp
    // ghi (mất điện đúng lúc) được lần quét sau nhận lại theo băm, không mất sách.
    let mut tx: Option<rusqlite::Transaction> = None;
    let mut pending = 0usize;
    let mut since = std::time::Instant::now();
    step(0);
    for (i, r) in reqs.iter().enumerate() {
        if opts.cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            rep.cancelled = true;
            break;
        }
        if i > 0 {
            step(i);
        }
        if pending > 0 && (pending >= COMMIT_EVERY_FILES || since.elapsed() >= COMMIT_EVERY) {
            if let Some(t) = tx.take() {
                t.commit()?;
            }
            pending = 0;
        }
        let Some(b) = db::get_book(conn, r.book_id)? else { continue };
        let fail = |rep: &mut MoveReport, msg: String| -> Result<()> {
            db::log_op(conn, batch, "move", Some(b.id), &b.path, None, Some(b.size), "failed", Some(&msg))?;
            rep.failures.push(MoveFailure { book_id: b.id, path: b.path.clone(), message: msg });
            Ok(())
        };
        let Some(lib) = cfg.library_root_of(&r.target_dir) else {
            fail(&mut rep, "Thư mục đích nằm ngoài thư viện".into())?;
            continue;
        };
        let src = PathBuf::from(&b.path);
        let target_dir = if opts.exact {
            r.target_dir.clone()
        } else {
            // Sách nằm trong một thư mục thư viện: đích chỉ phụ thuộc thư mục đó (xem `Placer::place_rel`).
            match placer.root_index(&src) {
                Some(h) => placed
                    .entry((Some(h), r.target_dir.clone()))
                    .or_insert_with(|| placer.place(&src, b.size.max(0) as u64, &r.target_dir))
                    .clone(),
                None => placer.place(&src, b.size.max(0) as u64, &r.target_dir),
            }
        };
        let lib = cfg.library_root_of(&target_dir).unwrap_or(lib);
        // Chuyển vào `Khác` → vào thẳng ngăn của chữ cái đầu (`Khác\T\T-01\…`), không làm phẳng lại `Khác`.
        let into_other = target_dir.parent() == Some(lib.as_path())
            && target_dir.file_name().is_some_and(|n| crate::triage::is_other_dir(&n.to_string_lossy()));
        if into_other {
            let letter = crate::triage::other_bucket(&b.file_name);
            let in_shelf = src.parent().is_some_and(|shelf| {
                shelf.parent() == Some(target_dir.join(&letter).as_path())
                    && shelf
                        .file_name()
                        .is_some_and(|n| crate::triage::other_shelf_number(&letter, &n.to_string_lossy()).is_some())
            });
            if in_shelf {
                continue; // đã ở đúng chỗ
            }
        } else if src.parent() == Some(target_dir.as_path()) {
            continue; // đã ở đúng chỗ
        }
        if !made_dirs.contains(&target_dir) {
            if !lib.is_dir() {
                fail(&mut rep, format!("Ổ thư viện chưa kết nối: {}", lib.display()))?;
                continue;
            }
            if let Err(e) = std::fs::create_dir_all(&target_dir) {
                fail(&mut rep, format!("Không tạo được thư mục {}: {e}", target_dir.display()))?;
                continue;
            }
            made_dirs.insert(target_dir.clone());
        }
        crate::progress::working_on(&src);
        let target = if into_other {
            let sh =
                shelves.entry(target_dir.clone()).or_insert_with(|| crate::triage::Shelves::new(&target_dir, false));
            match sh.dir_for(&b.file_name) {
                Ok(d) => d,
                Err(e) => {
                    fail(&mut rep, format!("Không tạo được ngăn trong {}: {e}", target_dir.display()))?;
                    continue;
                }
            }
        } else {
            target_dir.clone()
        };
        let name = opts.names.and_then(|m| m.get(&b.id)).unwrap_or(&b.file_name);
        let dst = unique_dest(&target, name);
        match move_file(&src, &dst) {
            Ok(()) => {
                if tx.is_none() {
                    tx = Some(crate::db::write_tx(conn)?);
                    since = std::time::Instant::now();
                }
                let t = tx.as_ref().expect("vừa mở");
                scan::relocate_in(t, cfg, &roots, b.id, &dst)?;
                if let Ok(m) = std::fs::metadata(&dst) {
                    let mt = m.modified().map(mtime_ms).unwrap_or(b.mtime);
                    if mt != b.mtime {
                        t.prepare_cached(
                            "UPDATE books SET mtime = ?2, hash_mtime = CASE WHEN hash_mtime = ?3 THEN ?2 ELSE hash_mtime END
                             WHERE id = ?1",
                        )?
                        .execute(params![b.id, mt, b.mtime])?;
                    }
                }
                db::log_op(
                    t,
                    batch,
                    "move",
                    Some(b.id),
                    &b.path,
                    Some(&dst.to_string_lossy()),
                    Some(b.size),
                    "done",
                    None,
                )?;
                pending += 1;
                rep.moved.push(MoveDone {
                    book_id: b.id,
                    from: b.path.clone(),
                    to: dst.to_string_lossy().into_owned(),
                });
            }
            Err(e) => fail(&mut rep, format!("Không di chuyển được: {e}"))?,
        }
    }
    if let Some(t) = tx.take() {
        t.commit()?;
    }
    if !rep.cancelled {
        step(reqs.len());
    }
    Ok(rep)
}

/// Sau một lô di chuyển: bỏ các thư mục nguồn đã rỗng hẳn (không còn file / thư mục nào), đi ngược lên dần.
/// Không bao giờ bỏ thư mục gốc hay thư mục thể loại cấp 1, hoặc thư mục mà `keep` (khoá từng cấp, tương đối so với
/// thư mục thư viện) giữ lại. Chỉ trong thư viện.
pub fn prune_empty_dirs(cfg: &Config, moved: &[MoveDone], keep: &dyn Fn(&[String]) -> bool) {
    let mut tried = std::collections::HashSet::new();
    for d in moved {
        let mut dir = Path::new(&d.from).parent().map(Path::to_path_buf);
        let Some(root) = dir.as_deref().and_then(|p| cfg.library_root_of(p)) else { continue };
        while let Some(p) = dir {
            let keys = crate::blocked::dir_keys(&root, &p);
            if keys.len() < 2 || keep(&keys) || !tried.insert(p.clone()) {
                break;
            }
            if std::fs::read_dir(&p).map(|mut r| r.next().is_some()).unwrap_or(true) || std::fs::remove_dir(&p).is_err()
            {
                break;
            }
            dir = p.parent().map(Path::to_path_buf);
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct UndoReport {
    pub batch_id: i64,
    pub restored: usize,
    pub failures: Vec<MoveFailure>,
    /// Dừng giữa chừng: lô chưa hoàn tác xong, hoàn tác lại sẽ làm tiếp.
    pub cancelled: bool,
}

/// Hoàn tác một lô di chuyển: trả từng file về đúng chỗ cũ, báo rõ file nào không hoàn tác được.
pub fn undo_batch(conn: &Connection, cfg: &Config, batch_id: i64) -> Result<UndoReport> {
    undo_batch_with(conn, cfg, batch_id, None, None)
}

/// Lô có chuyển file qua ổ khác (cân bằng ổ): hoàn tác cũng phải chép ngược lại – chạy nền, không chạy trong lệnh
/// giao diện.
pub fn batch_crosses_drives(conn: &Connection, batch_id: i64) -> Result<bool> {
    let mut st = conn.prepare(
        "SELECT src, dst FROM operations WHERE batch_id = ?1 AND kind = 'move' AND status = 'done' AND dst IS NOT NULL",
    )?;
    let rows = st.query_map([batch_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (src, dst) = row?;
        let (a, b) = (Path::new(&src), Path::new(&dst));
        let pa = a.parent().unwrap_or(a);
        let pb = b.parent().unwrap_or(b);
        if !crate::volumes::same_volume(pa, pb) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Như [`undo_batch`], dừng được (`cancel`, trước mỗi file) và báo tiến trình (`phase = "move"`). Dừng giữa chừng thì
/// lô chưa tính là đã hoàn tác: gọi lại sẽ làm tiếp phần còn lại.
pub fn undo_batch_with(
    conn: &Connection,
    cfg: &Config,
    batch_id: i64,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on: Option<crate::OnStep>,
) -> Result<UndoReport> {
    ensure_writable(cfg)?;
    let (kind, undone): (String, Option<i64>) = conn
        .query_row("SELECT kind, undone_at FROM batches WHERE id = ?1", [batch_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|_| Error::msg(format!("Không có lô #{batch_id}")))?;
    if kind == crate::metaedit::KIND {
        return Err(Error::msg(
            "Sửa thông tin sách không hoàn tác theo lô – hãy sửa lại (nếu đã ghi vào file, bản gốc nằm trong Thùng rác)",
        ));
    }
    if kind == "retitle" {
        return Err(Error::msg(
            "Sửa tiêu đề không hoàn tác theo lô – hãy sửa lại tiêu đề cũ (nếu đã ghi vào nội dung sách, bản gốc nằm trong Thùng rác Windows)",
        ));
    }
    if matches!(kind.as_str(), "trash" | "dedupe" | "dedupe_suspect" | crate::deleted::REJECT_KIND) {
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
    let total = ops.len();
    let step = |done: usize| {
        if let Some(f) = on {
            f(crate::Step { phase: "move", done, total });
        }
    };
    let roots = scan::all_roots(conn)?;
    let mut tx: Option<rusqlite::Transaction> = None;
    let mut pending = 0usize;
    let mut since = std::time::Instant::now();
    step(0);
    for (i, (op_id, book_id, src, dst)) in ops.into_iter().enumerate() {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            rep.cancelled = true;
            if let Some(t) = tx.take() {
                t.commit()?;
            }
            return Ok(rep);
        }
        if i > 0 {
            step(i);
        }
        if pending > 0 && (pending >= COMMIT_EVERY_FILES || since.elapsed() >= COMMIT_EVERY) {
            if let Some(t) = tx.take() {
                t.commit()?;
            }
            pending = 0;
        }
        crate::progress::working_on(Path::new(&dst));
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
                if tx.is_none() {
                    tx = Some(crate::db::write_tx(conn)?);
                    since = std::time::Instant::now();
                }
                let t = tx.as_ref().expect("vừa mở");
                if let Some(id) = book_id {
                    scan::relocate_in(t, cfg, &roots, id, &srcp)?;
                }
                t.prepare_cached("UPDATE operations SET status='undone' WHERE id=?1")?.execute([op_id])?;
                pending += 1;
                rep.restored += 1;
            }
            Err(e) => fail(&mut rep, format!("Không trả về được: {e}"))?,
        }
    }
    if let Some(t) = tx.take() {
        t.commit()?;
    }
    step(total);
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
