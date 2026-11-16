//! Thể loại bị chặn: những thư mục thể loại (kể cả thư mục con người dùng tự chia, vd.
//! `Ngôn tình - Đam mỹ/Đam mỹ`) mà người dùng không muốn đọc.
//!
//! - Chặn một thư mục → mọi sách trong đó vào Thùng rác ngay ([`enforce`], lô `block` trong lịch sử, ghi vào sổ sách
//!   đã xoá để tải lại đúng file đó cũng bị loại), rồi cả thư mục vào Thùng rác.
//! - Từ đó, sách tự phân loại vào thư mục bị chặn (hoặc thư mục con của nó) được đưa thẳng vào Thùng rác và thư mục
//!   không bao giờ được tạo lại (xem `triage.rs`). Thư mục bị chặn mà xuất hiện lại trên đĩa (tạo tay, dừng giữa
//!   chừng) thì lần quét sau tự dọn tiếp.
//!
//! Danh sách nằm trong `config.json` (`blocked_categories`): đường dẫn tương đối so với thư mục thư viện, phân cách
//! bằng `/`, so khớp không phân biệt dấu / hoa thường. Không chặn được hộp thư đến và thư mục `Khác`.
//! Luôn dùng Thùng rác (khôi phục được), không bao giờ xoá vĩnh viễn.

use crate::config::Config;
use crate::db::{self, Book};
use crate::error::{Error, Result};
use crate::normalize::simple_key;
use crate::trash::{trash_items_with, TrashFailure, TrashItem, TrashReport, Trasher, TRASH_PHASE};
use crate::{OnStep, Step};
use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Bước đưa sách trong thư mục bị chặn vào Thùng rác (x / y file).
pub const BLOCK_PHASE: &str = "block";
/// Bước đưa chính thư mục bị chặn (đã hết sách) vào Thùng rác.
pub const BLOCK_CLEAN_PHASE: &str = "block_clean";
/// Lô trong lịch sử thao tác.
pub const BLOCK_KIND: &str = "block";

/// Các thành phần của đường dẫn tương đối `rel` (`/` hoặc `\`), đã bỏ khoảng trắng thừa.
fn components(rel: &str) -> Vec<String> {
    rel.split(['/', '\\']).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect()
}

/// Khoá so khớp của một tên thư mục (bỏ dấu, chữ thường, bỏ ký tự đặc biệt).
pub fn name_key(name: &str) -> String {
    simple_key(name)
}

/// Khoá so khớp của một đường dẫn tương đối.
pub fn rel_keys(rel: &str) -> Vec<String> {
    components(rel).iter().map(|c| name_key(c)).collect()
}

/// Khoá của `dir` so với thư mục thư viện `root` (rỗng nếu không nằm trong `root`).
pub fn dir_keys(root: &Path, dir: &Path) -> Vec<String> {
    dir.strip_prefix(root)
        .map(|r| r.components().map(|c| name_key(&c.as_os_str().to_string_lossy())).collect())
        .unwrap_or_default()
}

/// Chuẩn hoá `rel` (`A/B`) và kiểm tra có chặn được không.
pub fn normalize(cfg: &Config, rel: &str) -> Result<String> {
    let parts = components(rel);
    if parts.is_empty() {
        return Err(Error::msg("Chưa chọn thư mục"));
    }
    if parts.iter().any(|p| p == "." || p == ".." || p.contains(':')) {
        return Err(Error::msg(format!("Đường dẫn không hợp lệ: {rel}")));
    }
    let first = name_key(&parts[0]);
    if first == name_key(&cfg.inbox_name) {
        return Err(Error::msg("Không chặn được hộp thư đến"));
    }
    if crate::triage::is_other_dir(&parts[0]) {
        return Err(Error::msg("Không chặn được thư mục “Khác” (nơi chứa sách chưa rõ thể loại)"));
    }
    Ok(parts.join("/"))
}

/// Danh sách thư mục bị chặn đã chuẩn bị để so khớp.
#[derive(Debug, Clone, Default)]
pub struct Blocked {
    /// (đường dẫn như người dùng chặn, khoá từng cấp)
    entries: Vec<(String, Vec<String>)>,
}

impl Blocked {
    pub fn new(cfg: &Config) -> Self {
        let entries = cfg
            .blocked_categories
            .iter()
            .map(|r| (components(r).join("/"), rel_keys(r)))
            .filter(|(_, k)| !k.is_empty())
            .collect();
        Blocked { entries }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Thư mục có khoá `keys` là (hoặc nằm trong) một thư mục bị chặn.
    pub fn covers(&self, keys: &[String]) -> bool {
        self.entries.iter().any(|(_, k)| keys.len() >= k.len() && keys[..k.len()] == k[..])
    }

    /// Đường dẫn bị chặn chứa thư mục có khoá `keys` (để ghi nhật ký).
    pub fn entry_for(&self, keys: &[String]) -> Option<&str> {
        self.entries.iter().find(|(_, k)| keys.len() >= k.len() && keys[..k.len()] == k[..]).map(|(r, _)| r.as_str())
    }

    /// Tên các thư mục bị chặn nằm ngay dưới thư mục có khoá `parent` (tên đúng như lúc chặn) – dùng được cả khi
    /// thư mục đó đã không còn trên đĩa.
    pub fn children(&self, parent: &[String]) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(_, k)| k.len() == parent.len() + 1 && k[..parent.len()] == parent[..])
            .filter_map(|(r, _)| components(r).pop())
            .collect()
    }

    /// Các thư mục cấp 1 của mọi mục bị chặn (tên như lúc chặn).
    pub fn top_levels(&self) -> Vec<String> {
        let mut v: Vec<String> = self.entries.iter().filter_map(|(r, _)| components(r).into_iter().next()).collect();
        v.sort();
        v.dedup_by(|a, b| name_key(a) == name_key(b));
        v
    }

    /// Mọi đường dẫn bị chặn.
    pub fn rels(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(r, _)| r.as_str())
    }
}

/// Thêm `rel` vào danh sách chặn (đã có thì thôi). Trả về đường dẫn đã chuẩn hoá. Người gọi lưu cấu hình.
pub fn add(cfg: &mut Config, rel: &str) -> Result<String> {
    if cfg.read_only {
        return Err(Error::ReadOnly);
    }
    let rel = normalize(cfg, rel)?;
    let keys = rel_keys(&rel);
    if !cfg.blocked_categories.iter().any(|r| rel_keys(r) == keys) {
        cfg.blocked_categories.push(rel.clone());
        cfg.blocked_categories.sort();
    }
    Ok(rel)
}

/// Bỏ chặn `rel`. Trả về `true` nếu có mục bị bỏ. Người gọi lưu cấu hình.
pub fn remove(cfg: &mut Config, rel: &str) -> bool {
    let keys = rel_keys(rel);
    let before = cfg.blocked_categories.len();
    cfg.blocked_categories.retain(|r| rel_keys(r) != keys);
    cfg.blocked_categories.len() != before
}

/// Một thư mục trong cây thể loại (để chọn thư mục cần chặn).
#[derive(Debug, Clone, Serialize)]
pub struct FolderNode {
    /// Đường dẫn tương đối `A/B`.
    pub rel: String,
    pub name: String,
    /// 1 = thư mục thể loại cấp 1.
    pub depth: usize,
    /// Số sách đang có trong thư mục (kể cả thư mục con).
    pub books: i64,
    pub blocked: bool,
    /// Còn trên đĩa không (mục bị chặn đã dọn thì không).
    pub exists: bool,
}

/// Cây thư mục thể loại tới độ sâu `max_depth` (trừ hộp thư đến và `Khác`), kèm số sách và các mục bị chặn
/// (kể cả đã không còn trên đĩa).
pub fn folder_tree(conn: &Connection, cfg: &Config, max_depth: usize) -> Result<Vec<FolderNode>> {
    let roots = cfg.library_roots();
    if roots.is_empty() {
        return Err(Error::NoLibrary);
    }
    let blocked = Blocked::new(cfg);
    let mut out = Vec::new();
    let inbox = name_key(&cfg.inbox_name);
    // Cấp 1 bỏ hộp thư đến và `Khác` (hàng trăm ngăn chữ cái) ngay từ đầu.
    let skip = |depth: usize, n: &str| depth == 1 && (name_key(n) == inbox || crate::triage::is_other_dir(n));
    fn walk(
        dir: &Path,
        rel: &[String],
        depth: usize,
        max: usize,
        skip: &dyn Fn(usize, &str) -> bool,
        out: &mut Vec<(Vec<String>, String)>,
    ) {
        if depth > max {
            return;
        }
        let mut subs: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .filter(|(n, _)| !n.starts_with('.') && !n.starts_with('$') && n != "System Volume Information")
            .filter(|(n, _)| !skip(depth, n))
            .collect();
        subs.sort_by_key(|(n, _)| name_key(n));
        for (n, p) in subs {
            crate::progress::pulse();
            let mut r = rel.to_vec();
            r.push(n.clone());
            out.push((r.clone(), n));
            walk(&p, &r, depth + 1, max, skip, out);
        }
    }
    // Mọi ổ thư viện; thư mục cùng tên trên các ổ là một mục (giữ tên ở ổ đầu tiên).
    let mut found: Vec<(Vec<String>, String)> = Vec::new();
    for root in &roots {
        let mut f = Vec::new();
        walk(root, &[], 1, max_depth.max(1), &skip, &mut f);
        for (rel, name) in f {
            let k: Vec<String> = rel.iter().map(|c| name_key(c)).collect();
            if !found.iter().any(|(r, _)| r.len() == k.len() && r.iter().zip(&k).all(|(a, b)| name_key(a) == *b)) {
                found.push((rel, name));
            }
        }
    }
    // Số sách theo từng cấp thư mục (một lần đọc DB).
    let mut counts: std::collections::HashMap<Vec<String>, i64> = std::collections::HashMap::new();
    let mut st = conn.prepare("SELECT path FROM books WHERE status = 'present'")?;
    let paths = st.query_map([], |r| r.get::<_, String>(0))?;
    for p in paths.flatten() {
        let Some(root) = cfg.library_root_of(Path::new(&p)) else { continue };
        let keys = dir_keys(&root, Path::new(&p));
        // bỏ thành phần cuối (tên file)
        for d in 1..keys.len().min(max_depth + 1) {
            *counts.entry(keys[..d].to_vec()).or_default() += 1;
        }
    }
    for (rel, name) in &found {
        let keys: Vec<String> = rel.iter().map(|c| name_key(c)).collect();
        out.push(FolderNode {
            rel: rel.join("/"),
            name: name.clone(),
            depth: rel.len(),
            books: counts.get(&keys).copied().unwrap_or(0),
            blocked: blocked.covers(&keys),
            exists: true,
        });
    }
    // Mục bị chặn không còn trên đĩa: vẫn hiện để bỏ chặn được.
    for r in blocked.rels() {
        let keys = rel_keys(r);
        if !out.iter().any(|n| rel_keys(&n.rel) == keys) {
            out.push(FolderNode {
                rel: r.to_string(),
                name: components(r).pop().unwrap_or_default(),
                depth: keys.len(),
                books: 0,
                blocked: true,
                exists: false,
            });
        }
    }
    // Thứ tự cây: theo khoá từng cấp.
    out.sort_by_key(|n| rel_keys(&n.rel));
    Ok(out)
}

/// Thư mục trên đĩa ứng với `rel`, nếu còn: tìm từng cấp theo tên thật trong thư mục (không phân biệt dấu / hoa
/// thường – thư mục tạo lại bằng tay có thể khác kiểu chữ), để đường dẫn trùng đúng với đường dẫn sách khi quét.
pub(crate) fn find_dir(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut cur = root.to_path_buf();
    for part in components(rel) {
        let want = name_key(&part);
        let subs: Vec<(String, PathBuf)> = std::fs::read_dir(&cur)
            .ok()?
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .collect();
        cur = subs
            .iter()
            .find(|(n, _)| *n == part)
            .or_else(|| subs.iter().find(|(n, _)| name_key(n) == want))
            .map(|(_, p)| p.clone())?;
    }
    (cur != root).then_some(cur)
}

/// Đưa sách trong các thư mục bị chặn (hoặc chỉ `only`) còn trên đĩa vào Thùng rác, rồi chính thư mục đó.
///
/// Báo tiến trình [`BLOCK_PHASE`] x / y file và [`BLOCK_CLEAN_PHASE`]; dừng được bằng `cancel` (trước mỗi file).
/// Thư mục chỉ bị dọn khi mọi sách trong đó đã vào Thùng rác (không lỗi, không dừng giữa chừng); file mà DB chưa
/// biết (chưa quét tới) đi cùng thư mục. Không có gì để làm thì không tạo lô trong lịch sử.
pub fn enforce(
    conn: &Connection,
    cfg: &Config,
    trasher: &dyn Trasher,
    only: Option<&str>,
    cancel: Option<&AtomicBool>,
    on: Option<OnStep>,
) -> Result<TrashReport> {
    if cfg.read_only {
        return Err(Error::ReadOnly);
    }
    let roots = cfg.library_roots();
    if roots.is_empty() {
        return Err(Error::NoLibrary);
    }
    let blocked = Blocked::new(cfg);
    let rels: Vec<String> = match only {
        Some(r) => vec![normalize(cfg, r)?],
        None => blocked.rels().map(str::to_string).collect(),
    };
    let step = |phase: &'static str, done: usize, total: usize| {
        if let Some(f) = on {
            f(Step { phase, done, total });
        }
    };
    // Thư mục bị chặn trên mọi ổ thư viện.
    let dirs: Vec<(PathBuf, PathBuf)> = roots
        .iter()
        .flat_map(|root| rels.iter().filter_map(move |r| Some((root.clone(), find_dir(root, r)?))))
        .collect();
    let dir_keys_list: Vec<(PathBuf, Vec<String>)> =
        dirs.iter().map(|(root, d)| (root.clone(), dir_keys(root, d))).collect();
    let dirs: Vec<PathBuf> = dirs.into_iter().map(|(_, d)| d).collect();
    let mut rep = TrashReport::default();
    if dirs.is_empty() {
        return Ok(rep);
    }
    step(BLOCK_PHASE, 0, 0);
    let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    let mut books: Vec<Book> = Vec::new();
    for b in db::books_where(conn, "status = 'present' ORDER BY path", [])? {
        crate::progress::pulse();
        let p = Path::new(&b.path);
        if dir_keys_list.iter().any(|(root, k)| {
            let keys = dir_keys(root, p);
            p.starts_with(root) && !k.is_empty() && keys.len() > k.len() && keys[..k.len()] == k[..]
        }) {
            books.push(b);
        }
    }
    if cancelled() {
        rep.cancelled = true;
        return Ok(rep);
    }
    if !books.is_empty() {
        let items: Vec<TrashItem> = books
            .iter()
            .map(|b| TrashItem { book: b, expect: None, keeper: None, note: Some("thể loại bị chặn".into()) })
            .collect();
        // Bước con của `trash_items_with` mang tên riêng: giao diện / giám sát treo biết đây là xoá do chặn.
        let remap = |s: Step| step(if s.phase == TRASH_PHASE { BLOCK_PHASE } else { s.phase }, s.done, s.total);
        rep = trash_items_with(conn, cfg, trasher, BLOCK_KIND, &items, cancel, Some(&remap))?;
    }
    if rep.cancelled || rep.aborted.is_some() || !rep.failures.is_empty() {
        return Ok(rep); // thư mục còn sách: lần quét sau (hoặc lần chặn lại) dọn tiếp
    }
    for (i, d) in dirs.iter().enumerate() {
        if cancelled() {
            rep.cancelled = true;
            break;
        }
        step(BLOCK_CLEAN_PHASE, i, dirs.len());
        crate::progress::working_on(d);
        if let Err(e) = trasher.trash(d) {
            rep.failures.push(TrashFailure { book_id: 0, path: d.to_string_lossy().into_owned(), message: e });
        }
    }
    if !rep.cancelled {
        step(BLOCK_CLEAN_PHASE, dirs.len(), dirs.len());
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(blocked: &[&str]) -> Config {
        Config { blocked_categories: blocked.iter().map(|s| s.to_string()).collect(), ..Config::default() }
    }

    #[test]
    fn covers_and_children_ignore_case_and_diacritics() {
        let b = Blocked::new(&cfg(&["Ngôn tình - Đam mỹ/Đam Mỹ"]));
        assert!(b.covers(&rel_keys("ngon tinh - dam my/dam my")));
        assert!(b.covers(&rel_keys("Ngôn Tình - Đam Mỹ\\Đam mỹ\\Tác giả A")), "thư mục con của mục bị chặn");
        assert!(!b.covers(&rel_keys("Ngôn tình - Đam mỹ")), "thư mục cha không bị chặn");
        assert!(!b.covers(&rel_keys("Ngôn tình - Đam mỹ/Ngôn tình")));
        assert_eq!(b.children(&rel_keys("ngôn tình - đam mỹ")), vec!["Đam Mỹ".to_string()]);
        assert!(b.children(&[]).is_empty());
        assert_eq!(b.top_levels(), vec!["Ngôn tình - Đam mỹ".to_string()]);
    }

    #[test]
    fn add_remove_and_reject_inbox_or_other() {
        let mut c = cfg(&[]);
        assert_eq!(add(&mut c, " Ngôn tình - Đam mỹ \\ Đam mỹ ").unwrap(), "Ngôn tình - Đam mỹ/Đam mỹ");
        add(&mut c, "ngôn tình - đam mỹ/đam mỹ").unwrap();
        assert_eq!(c.blocked_categories.len(), 1, "không thêm trùng");
        assert!(add(&mut c, "Chưa phân loại").is_err());
        assert!(add(&mut c, "Khác/T").is_err());
        assert!(add(&mut c, "../x").is_err());
        assert!(remove(&mut c, "NGÔN TÌNH - ĐAM MỸ/ĐAM MỸ"));
        assert!(c.blocked_categories.is_empty());
        c.read_only = true;
        assert!(add(&mut c, "Thơ").is_err());
    }
}
