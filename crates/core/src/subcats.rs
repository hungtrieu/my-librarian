//! Thư mục con của thể loại **do app quản lý** và việc làm phẳng thư mục con lạ.
//!
//! Sách chép vào thư viện thường kéo theo cả cây thư mục của nguồn (`Văn học/Nguyễn Nhật Ánh/Bộ 1/…`,
//! `Truyện tranh/One Piece/Tập 01/…`). Trong app chỉ những thư mục con người dùng tạo bằng app (hoặc làm đích của
//! ánh xạ tag) mới là thư mục con thể loại: hiện ở thanh bên, nhận sách kéo thả, được tự phân loại vào. Mỗi lần quét,
//! [`flatten`] đưa mọi file sách nằm trong thư mục con khác ra thư mục được quản lý gần nhất bên trên (thường chính là
//! thư mục thể loại), rồi đưa các thư mục con đã hết sách vào Thùng rác (ảnh bìa, `.opf`, file rác đi cùng).
//!
//! - Chuyển trong cùng thư mục thư viện: chỉ là đổi tên – tức thì, không chép dữ liệu.
//! - Một lô [`FLATTEN_KIND`] trong Hoạt động, hoàn tác được (file về chỗ cũ, thư mục được tạo lại).
//! - Tên trùng thêm `(1)`; tên file chung chung (`01.epub`, `Tập 3.cbz`) được thêm tên thư mục phía trước
//!   (`One Piece - Tập 3.cbz`) để không mất thông tin bộ.
//! - Không đụng hộp thư đến, `Khác` (có cấu trúc ngăn riêng) và thư mục bị chặn.

use crate::blocked::{self, name_key, rel_keys, Blocked};
use crate::config::Config;
use crate::db;
use crate::error::Result;
use crate::moves::{self, MoveOptions, MoveRequest};
use crate::normalize::simple_key;
use crate::trash::Trasher;
use crate::OnStep;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Lô trong lịch sử khi làm phẳng thư mục con (hoàn tác được).
pub const FLATTEN_KIND: &str = "flatten";
/// Bước tiến trình: chuyển file ra thư mục thể loại (x / y file).
pub const FLATTEN_PHASE: &str = "flatten";
/// Bước tiến trình: dọn thư mục con đã hết sách.
pub const FLATTEN_CLEAN_PHASE: &str = "flatten_clean";

fn components(rel: &str) -> Vec<String> {
    rel.split(['/', '\\']).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect()
}

/// Tập thư mục con được quản lý (khoá từng cấp): danh sách trong cấu hình + đích của ánh xạ tag, kèm mọi thư mục cha
/// của chúng.
#[derive(Debug, Clone, Default)]
pub struct SubCats {
    set: HashSet<Vec<String>>,
}

impl SubCats {
    pub fn new(cfg: &Config) -> Self {
        let mut set = HashSet::new();
        for rel in cfg.subcategories.iter().chain(cfg.tag_map.iter().map(|t| &t.target)) {
            let k = rel_keys(rel);
            for n in 2..=k.len() {
                set.insert(k[..n].to_vec());
            }
        }
        SubCats { set }
    }

    /// Thư mục `keys` (khoá từng cấp, tính từ thư mục thư viện) là thư mục thể loại (cấp 1) hoặc thư mục con được
    /// quản lý.
    pub fn contains(&self, keys: &[String]) -> bool {
        keys.len() == 1 || self.set.contains(keys)
    }

    /// Số cấp của thư mục được quản lý sâu nhất chứa `keys` (≥ 1: thư mục thể loại).
    pub fn keep_len(&self, keys: &[String]) -> usize {
        (2..=keys.len()).rev().find(|&n| self.set.contains(&keys[..n])).unwrap_or(1)
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }
}

/// Ghi `rel` (`A/B`, đã kiểm tra) vào danh sách thư mục con được quản lý. Người gọi lưu cấu hình.
pub fn register(cfg: &mut Config, rel: &str) {
    let k = rel_keys(rel);
    if k.len() < 2 || cfg.subcategories.iter().any(|r| rel_keys(r) == k) {
        return;
    }
    cfg.subcategories.push(components(rel).join("/"));
    cfg.subcategories.sort_by_key(|r| rel_keys(r));
}

/// Đã được quản lý (`rel` có ít nhất 2 cấp).
pub fn is_registered(cfg: &Config, rel: &str) -> bool {
    SubCats::new(cfg).set.contains(&rel_keys(rel))
}

/// Từ chỉ số tập / chương… – tên file chỉ gồm những từ này và số là "chung chung".
const GENERIC_WORDS: &[&str] = &[
    "tap", "chuong", "phan", "quyen", "cuon", "hoi", "so", "vol", "volume", "part", "book", "chapter", "ch", "ep",
    "episode", "issue", "q", "t", "p", "no", "ky", "full", "end", "ebook", "epub", "pdf", "mobi", "prc", "azw3",
];

/// Tên file (không đuôi) không tự nói là sách gì: `01`, `Tập 3`, `Chapter 12`, `Q.2`…
pub fn is_generic_stem(stem: &str) -> bool {
    let k = simple_key(stem);
    let letters: usize = k
        .split(' ')
        .filter(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit()) && !GENERIC_WORDS.contains(w))
        .map(|w| w.chars().count())
        .sum();
    letters < 3
}

/// Tên file khi đưa `file_name` ra khỏi các thư mục `dirs` (từ ngoài vào trong): tên chung chung thì thêm tên thư
/// mục gần nhất có nghĩa phía trước (`One Piece - Tập 3.cbz`); tên đã chứa tên thư mục đó thì giữ nguyên.
pub fn flat_name(file_name: &str, dirs: &[String]) -> String {
    let (stem, ext) = match file_name.rfind('.') {
        Some(i) if i > 0 => (&file_name[..i], &file_name[i..]),
        _ => (file_name, ""),
    };
    if !is_generic_stem(stem) {
        return file_name.to_string();
    }
    // Từ thư mục gần nhất có nghĩa trở vào (`Harry Potter/Tập 1/01.epub` → `Harry Potter - Tập 1 - 01.epub`).
    let i = dirs.iter().rposition(|d| !is_generic_stem(d)).unwrap_or(0);
    let used: Vec<&str> = dirs[i.min(dirs.len())..].iter().map(|d| d.trim()).filter(|d| !d.is_empty()).collect();
    let Some(first) = used.first() else { return file_name.to_string() };
    let (sk, dk) = (simple_key(stem), simple_key(first));
    if !dk.is_empty() && format!(" {sk} ").contains(&format!(" {dk} ")) {
        return file_name.to_string();
    }
    let name = format!("{} - {}", used.join(" - "), stem.trim());
    let name: String = name.chars().take(180).collect();
    format!("{}{ext}", name.trim_end_matches(['.', ' ']))
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FlattenReport {
    /// Lô di chuyển (0 = không có gì để chuyển).
    pub batch_id: i64,
    /// Số file sách đã đưa ra thư mục thể loại.
    pub moved: usize,
    /// Thư mục con đã hết sách, đã vào Thùng rác.
    pub dirs_trashed: usize,
    pub failures: Vec<String>,
    pub cancelled: bool,
}

/// Làm phẳng thư mục con không được quản lý ở mọi thư mục thư viện (xem mô tả module). Sách phải đã được quét
/// (dùng DB để biết sách nào nằm ở đâu); file sách mà DB chưa biết cũng được đưa ra (chỉ chuyển file).
pub fn flatten(
    conn: &Connection,
    cfg: &Config,
    trasher: &dyn Trasher,
    cancel: Option<&AtomicBool>,
    on: Option<OnStep>,
) -> Result<FlattenReport> {
    let mut rep = FlattenReport::default();
    if cfg.read_only || !cfg.flatten_subfolders {
        return Ok(rep);
    }
    let subs = SubCats::new(cfg);
    let blocked = Blocked::new(cfg);
    let inbox = name_key(&cfg.inbox_name);
    let skip_top = |n: &str| name_key(n) == inbox || crate::triage::is_other_dir(n);
    let roots: Vec<PathBuf> = cfg.library_roots().into_iter().filter(|r| r.is_dir()).collect();
    let lib_ids = crate::dedupe::library_root_ids(conn)?;

    // 1) Sách đã biết nằm trong thư mục con lạ → thư mục được quản lý gần nhất bên trên.
    let books = db::books_where(conn, "status = 'present' AND in_inbox = 0 AND category IS NOT NULL", [])?;
    let mut reqs = Vec::new();
    let mut names: HashMap<i64, String> = HashMap::new();
    for b in books.iter().filter(|b| lib_ids.contains(&b.root_id)) {
        let path = Path::new(&b.path);
        let Some(dir) = path.parent() else { continue };
        let Some(root) = roots.iter().filter(|r| dir.starts_with(r)).max_by_key(|r| r.components().count()) else {
            continue;
        };
        let Ok(rel) = dir.strip_prefix(root) else { continue };
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if parts.len() < 2 {
            continue;
        }
        let keys: Vec<String> = parts.iter().map(|p| name_key(p)).collect();
        if skip_top(&parts[0]) || blocked.covers(&keys) {
            continue;
        }
        let keep = subs.keep_len(&keys);
        if keep >= keys.len() {
            continue;
        }
        let target = parts[..keep].iter().fold(root.clone(), |p, c| p.join(c));
        let new_name = flat_name(&b.file_name, &parts[keep..]);
        if new_name != b.file_name {
            names.insert(b.id, new_name);
        }
        reqs.push(MoveRequest { book_id: b.id, target_dir: target });
    }
    if !reqs.is_empty() {
        let step = |s: crate::Step| {
            if let Some(f) = on {
                f(crate::Step { phase: FLATTEN_PHASE, ..s });
            }
        };
        let opts = MoveOptions { exact: true, cancel, progress: Some(&step), names: Some(&names) };
        let mr = moves::apply_moves_with(conn, cfg, &reqs, FLATTEN_KIND, &opts)?;
        rep.batch_id = mr.batch_id;
        rep.moved = mr.moved.len();
        rep.failures.extend(mr.failures.iter().map(|f| format!("{}: {}", f.path, f.message)));
        if mr.cancelled {
            rep.cancelled = true;
            return Ok(rep);
        }
    }

    // 2) Thư mục con lạ còn lại: file sách DB chưa biết thì đưa ra; hết sách thì vào Thùng rác.
    let known: HashSet<String> = {
        let mut st = conn.prepare("SELECT path FROM books WHERE status = 'present'")?;
        let v = st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
        v
    };
    let mut candidates: Vec<(PathBuf, PathBuf, Vec<String>)> = Vec::new();
    for root in &roots {
        for top in list_dirs(root) {
            let n = top.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if skip_top(&n) || blocked.covers(&[name_key(&n)]) {
                continue;
            }
            find_unmanaged(root, &top, &subs, &blocked, &mut candidates);
        }
    }
    let total = candidates.len();
    for (i, (dir, keep_dir, _)) in candidates.iter().enumerate() {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            rep.cancelled = true;
            break;
        }
        if let Some(f) = on {
            f(crate::Step { phase: FLATTEN_CLEAN_PHASE, done: i, total });
        }
        crate::progress::working_on(dir);
        let mut left = 0usize;
        for e in walkdir::WalkDir::new(dir).into_iter().flatten() {
            crate::progress::pulse();
            if !e.file_type().is_file() || !crate::triage::is_book(e.path()) {
                continue;
            }
            let p = e.path();
            if known.contains(p.to_string_lossy().as_ref()) {
                left += 1; // sách đã biết mà chuyển lỗi ở bước 1 – giữ nguyên
                continue;
            }
            let rel_dirs: Vec<String> = p
                .parent()
                .and_then(|d| d.strip_prefix(keep_dir).ok())
                .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect())
                .unwrap_or_default();
            let fname = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let dst = moves::unique_dest(keep_dir, &flat_name(&fname, &rel_dirs));
            if let Err(e) = moves::move_file(p, &dst) {
                left += 1;
                rep.failures.push(format!("{}: {e}", p.display()));
            } else {
                rep.moved += 1;
            }
        }
        if left > 0 {
            continue;
        }
        match trasher.trash(dir) {
            Ok(()) => rep.dirs_trashed += 1,
            Err(e) => rep.failures.push(format!("{}: không đưa được vào Thùng rác: {e}", dir.display())),
        }
    }
    if let Some(f) = on {
        f(crate::Step { phase: FLATTEN_CLEAN_PHASE, done: total, total });
    }
    Ok(rep)
}

fn list_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            !n.starts_with('.') && !n.starts_with('$') && n != "System Volume Information"
        })
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

/// Thư mục con lạ cao nhất bên trong thư mục được quản lý `dir`: (thư mục lạ, thư mục được quản lý chứa nó, khoá).
fn find_unmanaged(
    root: &Path,
    dir: &Path,
    subs: &SubCats,
    blocked: &Blocked,
    out: &mut Vec<(PathBuf, PathBuf, Vec<String>)>,
) {
    for d in list_dirs(dir) {
        crate::progress::pulse();
        let keys = blocked::dir_keys(root, &d);
        if blocked.covers(&keys) {
            continue;
        }
        if subs.contains(&keys) {
            find_unmanaged(root, &d, subs, blocked, out);
        } else {
            out.push((d, dir.to_path_buf(), keys));
        }
    }
}

/// Thư mục con được quản lý của mọi thể loại, kèm số sách – cho thanh bên.
#[derive(Debug, Clone, Serialize)]
pub struct SubFolder {
    /// `A/B` (tên thật trên ổ đầu tiên có thư mục).
    pub rel: String,
    pub name: String,
    /// 2 = thư mục con trực tiếp của thể loại.
    pub depth: usize,
    /// Thư mục trên ổ đầu tiên có nó (chưa có trên ổ nào: nơi sẽ tạo trên thư mục chính).
    pub path: String,
    /// Số sách nằm trong thư mục (kể cả thư mục con của nó).
    pub books: i64,
}

/// Danh sách thư mục con được quản lý (theo thứ tự cây). Thư mục chưa có trên ổ nào vẫn hiện (đích ánh xạ tag sẽ được
/// tạo khi có sách chuyển tới).
pub fn list(conn: &Connection, cfg: &Config) -> Result<Vec<SubFolder>> {
    let subs = SubCats::new(cfg);
    if subs.is_empty() {
        return Ok(Vec::new());
    }
    let roots = cfg.library_roots();
    let mut keys: Vec<&Vec<String>> = subs.set.iter().collect();
    keys.sort();
    // Tên hiển thị: theo cấu hình (người dùng gõ), ưu tiên tên thật trên ổ.
    let mut named: HashMap<Vec<String>, Vec<String>> = HashMap::new();
    for rel in cfg.subcategories.iter().chain(cfg.tag_map.iter().map(|t| &t.target)) {
        let parts = components(rel);
        for n in 2..=parts.len() {
            let k: Vec<String> = parts[..n].iter().map(|p| name_key(p)).collect();
            named.entry(k).or_insert_with(|| parts[..n].to_vec());
        }
    }
    let mut counts: HashMap<Vec<String>, i64> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT path FROM books WHERE status = 'present' AND in_inbox = 0 AND category IS NOT NULL
             AND (rel_path LIKE '%\\%\\%' OR rel_path LIKE '%/%/%')",
        )?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        for p in rows.flatten() {
            let p = PathBuf::from(p);
            let Some(root) = roots.iter().filter(|r| p.starts_with(r)).max_by_key(|r| r.components().count()) else {
                continue;
            };
            let Some(dir) = p.parent() else { continue };
            let k = blocked::dir_keys(root, dir);
            if k.first().is_some_and(|c| crate::triage::is_other_dir(c)) {
                continue;
            }
            for n in 2..=k.len() {
                if subs.set.contains(&k[..n]) {
                    *counts.entry(k[..n].to_vec()).or_default() += 1;
                }
            }
        }
    }
    let mut out = Vec::new();
    for k in keys {
        let rel = k.join("/");
        let found = roots.iter().filter(|r| r.is_dir()).find_map(|r| {
            let d = blocked::find_dir(r, &rel)?;
            let names: Vec<String> =
                d.strip_prefix(r).ok()?.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            Some((d, names))
        });
        let (path, parts) = match found {
            Some((d, names)) => (d.to_string_lossy().into_owned(), names),
            None => {
                let parts = named.get(k).cloned().unwrap_or_else(|| k.clone());
                let base = roots.first().cloned().unwrap_or_default();
                (crate::volumes::dir_on(&base, &parts.join("/")).to_string_lossy().into_owned(), parts)
            }
        };
        out.push(SubFolder {
            rel: parts.join("/"),
            name: parts.last().cloned().unwrap_or_default(),
            depth: parts.len(),
            path,
            books: counts.get(k).copied().unwrap_or(0),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_names_get_their_folder() {
        assert!(is_generic_stem("01"));
        assert!(is_generic_stem("Tập 3"));
        assert!(is_generic_stem("Chapter 12"));
        assert!(is_generic_stem("Q.2"));
        assert!(!is_generic_stem("Mắt biếc"));
        let d = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(flat_name("Tập 3.cbz", &d(&["One Piece"])), "One Piece - Tập 3.cbz");
        assert_eq!(flat_name("01.epub", &d(&["Harry Potter", "Tập 1"])), "Harry Potter - Tập 1 - 01.epub");
        assert_eq!(flat_name("01.epub", &d(&["Doraemon", "Harry Potter"])), "Harry Potter - 01.epub");
        assert_eq!(flat_name("Mắt biếc.epub", &d(&["Nguyễn Nhật Ánh"])), "Mắt biếc.epub");
        assert_eq!(flat_name("01.epub", &d(&["Quyển 2"])), "Quyển 2 - 01.epub");
        assert_eq!(flat_name("01.epub", &d(&[])), "01.epub");
    }

    #[test]
    fn keep_len_finds_the_deepest_managed_folder() {
        let cfg = Config {
            subcategories: vec!["Văn học/Nước ngoài".into()],
            tag_map: vec![crate::config::TagRule { tag: "x".into(), target: "Ngôn tình/Đam mỹ/Hiện đại".into() }],
            ..Config::default()
        };
        let s = SubCats::new(&cfg);
        assert_eq!(s.keep_len(&rel_keys("Van hoc/Nuoc ngoai/Murakami")), 2);
        assert_eq!(s.keep_len(&rel_keys("Văn học/NNA")), 1);
        assert_eq!(s.keep_len(&rel_keys("Ngôn tình/Đam mỹ/x")), 2, "thư mục cha của đích cũng được quản lý");
        assert!(s.contains(&rel_keys("Ngôn tình/Đam mỹ/Hiện đại")));
        assert!(!s.contains(&rel_keys("Ngôn tình/Khác lạ")));
    }
}
