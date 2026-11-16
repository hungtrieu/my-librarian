//! Quản lý thư mục thể loại ngay trong app – sửa thẳng thư mục thật trên mọi ổ thư viện:
//!
//! - [`create`]: thêm thể loại (thư mục cấp 1) hoặc thư mục con;
//! - [`rename`]: đổi tên thể loại / thư mục con trên mọi ổ có thư mục đó (đổi tên trong cùng ổ: tức thì, không chép
//!   file), cập nhật đường dẫn sách trong DB, danh sách chặn và ánh xạ tag;
//! - [`delete`]: xoá thể loại / thư mục con – sách bên trong được chuyển ra trước (thể loại → `Khác`, thư mục con →
//!   thư mục cha, cùng ổ, hoàn tác được), rồi phần còn lại (ảnh bìa, file rác) vào Thùng rác. Thư mục còn sách chưa
//!   chuyển được (hoặc sách chưa quét tới) thì giữ nguyên.
//!
//! Không đụng được hộp thư đến và `Khác` (cùng các ngăn chữ cái của nó).

use crate::blocked::{self, rel_keys};
use crate::config::Config;
use crate::db::{self, Book};
use crate::error::{Error, Result};
use crate::moves::{self, MoveOptions, MoveReport, MoveRequest};
use crate::trash::Trasher;
use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Lô trong lịch sử khi xoá thư mục (sách được chuyển ra – hoàn tác được).
pub const DELETE_KIND: &str = "folder_delete";
/// Lô ghi lại lần đổi tên thư mục (chỉ để xem lịch sử; đổi lại tên để hoàn tác).
pub const RENAME_KIND: &str = "folder_rename";

fn components(rel: &str) -> Vec<String> {
    rel.split(['/', '\\']).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect()
}

/// Tên thư mục hợp lệ trên Windows / macOS (không ký tự cấm, không tên dành riêng, không kết thúc bằng dấu chấm).
pub fn validate_name(cfg: &Config, name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err(Error::msg("Tên thư mục không được để trống"));
    }
    if n.chars().any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()) {
        return Err(Error::msg("Tên thư mục không được chứa / \\ : * ? \" < > |"));
    }
    if n == "." || n == ".." || n.ends_with('.') || n.starts_with('.') {
        return Err(Error::msg("Tên thư mục không được bắt đầu / kết thúc bằng dấu chấm"));
    }
    let up = n.to_uppercase();
    let stem = up.split('.').next().unwrap_or_default();
    if ["CON", "PRN", "AUX", "NUL"].contains(&stem)
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit())
    {
        return Err(Error::msg(format!("“{n}” là tên dành riêng của Windows")));
    }
    if n.chars().count() > 120 {
        return Err(Error::msg("Tên thư mục quá dài (tối đa 120 ký tự)"));
    }
    if blocked::name_key(n) == blocked::name_key(&cfg.inbox_name) || crate::triage::is_other_dir(n) {
        return Err(Error::msg(format!("“{n}” là tên dành riêng của app (hộp thư đến / Khác)")));
    }
    Ok(n.to_string())
}

/// Kiểm tra thư mục `rel` (`A/B`) sửa / xoá được.
fn check_rel(cfg: &Config, rel: &str) -> Result<Vec<String>> {
    if cfg.read_only {
        return Err(Error::ReadOnly);
    }
    let parts = components(rel);
    if parts.is_empty() || parts.iter().any(|p| p == "." || p == ".." || p.contains(':')) {
        return Err(Error::msg(format!("Đường dẫn không hợp lệ: {rel}")));
    }
    if blocked::name_key(&parts[0]) == blocked::name_key(&cfg.inbox_name) {
        return Err(Error::msg("Không sửa được hộp thư đến"));
    }
    if crate::triage::is_other_dir(&parts[0]) {
        return Err(Error::msg("Không sửa được thư mục “Khác” và các ngăn của nó"));
    }
    Ok(parts)
}

/// Thư mục `rel` trên từng ổ thư viện đang có nó.
fn dirs_of(cfg: &Config, rel: &str) -> Vec<(PathBuf, PathBuf)> {
    cfg.library_roots()
        .into_iter()
        .filter(|r| r.is_dir())
        .filter_map(|r| {
            let d = blocked::find_dir(&r, rel)?;
            Some((r, d))
        })
        .collect()
}

/// Thêm thư mục `name` vào `parent` (`None` = thể loại mới ở cấp 1). Thư mục cha có ở nhiều ổ thì tạo ở ổ đầu tiên có
/// nó (thư mục chính trước); thể loại mới tạo ở thư mục thư viện chính. Trả về đường dẫn tương đối mới.
pub fn create(cfg: &Config, parent: Option<&str>, name: &str) -> Result<String> {
    if cfg.read_only {
        return Err(Error::ReadOnly);
    }
    let name = validate_name(cfg, name)?;
    let (base, rel) = match parent.filter(|p| !p.trim().is_empty()) {
        Some(p) => {
            let parts = check_rel(cfg, p)?;
            let rel = parts.join("/");
            let (root, d) = dirs_of(cfg, &rel)
                .into_iter()
                .next()
                .ok_or_else(|| Error::msg(format!("Không thấy thư mục “{rel}” trên ổ nào")))?;
            // Tên thật trên ổ (người dùng có thể gõ không dấu).
            let real: Vec<String> = d
                .strip_prefix(&root)
                .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect())
                .unwrap_or(parts);
            (d, format!("{}/{name}", real.join("/")))
        }
        None => {
            let root = cfg.library_roots().into_iter().find(|r| r.is_dir()).ok_or(Error::NoLibrary)?;
            (root, name.clone())
        }
    };
    if dirs_of(cfg, &rel).iter().any(|(_, d)| d.is_dir()) {
        return Err(Error::msg(format!("Đã có thư mục “{}”", rel.replace('/', " › "))));
    }
    std::fs::create_dir(base.join(&name)).map_err(|e| Error::msg(format!("Không tạo được thư mục: {e}")))?;
    Ok(rel)
}

/// Đổi tiền tố `old` (khoá từng cấp) của đường dẫn tương đối `entry` thành `new_parts`; `None` nếu không nằm trong.
fn rebase(entry: &str, old: &[String], new_parts: &[String]) -> Option<String> {
    let parts = components(entry);
    let keys: Vec<String> = parts.iter().map(|p| blocked::name_key(p)).collect();
    (keys.len() >= old.len() && keys[..old.len()] == old[..]).then(|| {
        let mut v = new_parts.to_vec();
        v.extend(parts[old.len()..].iter().cloned());
        v.join("/")
    })
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RenameReport {
    /// Đường dẫn tương đối mới.
    pub rel: String,
    /// Thư mục đã đổi tên trên đĩa (mỗi ổ một).
    pub dirs: Vec<String>,
    /// Số sách đã cập nhật đường dẫn trong DB.
    pub books: usize,
    /// Ổ không đổi được (lỗi), kèm lý do.
    pub failures: Vec<String>,
}

/// Đổi tên thư mục `rel` thành `new_name` trên mọi ổ có nó (xem mô tả module). `cfg` được cập nhật (danh sách chặn,
/// ánh xạ tag) – người gọi lưu cấu hình.
pub fn rename(conn: &Connection, cfg: &mut Config, rel: &str, new_name: &str) -> Result<RenameReport> {
    let parts = check_rel(cfg, rel)?;
    let name = validate_name(cfg, new_name)?;
    let old_keys: Vec<String> = parts.iter().map(|p| blocked::name_key(p)).collect();
    let mut new_parts = parts.clone();
    *new_parts.last_mut().expect("không rỗng") = name.clone();
    let same_key = blocked::name_key(&name) == *old_keys.last().expect("không rỗng");
    let dirs = dirs_of(cfg, &parts.join("/"));
    if dirs.is_empty() {
        return Err(Error::msg(format!("Không thấy thư mục “{}” trên ổ nào", parts.join(" › "))));
    }
    // Trùng tên với thư mục khác (không phân biệt dấu / hoa thường) ở bất kỳ ổ nào thì thôi – không gộp ngầm.
    if !same_key && !dirs_of(cfg, &new_parts.join("/")).is_empty() {
        return Err(Error::msg(format!("Đã có thư mục “{}”", new_parts.join(" › "))));
    }
    let mut rep = RenameReport { rel: new_parts.join("/"), ..Default::default() };
    let mut renamed: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (_, d) in &dirs {
        let to = d.with_file_name(&name);
        if to == *d {
            continue;
        }
        crate::progress::working_on(d);
        match std::fs::rename(d, &to) {
            Ok(()) => {
                rep.dirs.push(to.to_string_lossy().into_owned());
                renamed.push((d.clone(), to));
            }
            Err(e) => rep
                .failures
                .push(format!("{}: {e} (có thể một file trong thư mục đang mở – đóng lại rồi thử lại)", d.display())),
        }
    }
    if !renamed.is_empty() {
        let tx = db::write_tx(conn)?;
        let batch = db::new_batch(&tx, RENAME_KIND, Some(&format!("{} → {}", parts.join("/"), rep.rel)))?;
        let books = db::books_where(&tx, "status = 'present'", [])?;
        for (from, to) in &renamed {
            db::log_op(
                &tx,
                batch,
                "rename_dir",
                None,
                &from.to_string_lossy(),
                Some(&to.to_string_lossy()),
                None,
                "done",
                None,
            )?;
            for b in books.iter().filter(|b| Path::new(&b.path).starts_with(from)) {
                crate::progress::pulse();
                let rest = Path::new(&b.path).strip_prefix(from).expect("đã kiểm tra");
                crate::scan::relocate(&tx, cfg, b.id, &to.join(rest))?;
                rep.books += 1;
            }
        }
        tx.commit()?;
    }
    // Cấu hình trỏ tới thư mục cũ (hoặc thư mục con của nó): đổi theo, kể cả khi chỉ một phần ổ đổi được.
    if rep.failures.is_empty() || !renamed.is_empty() {
        for r in cfg.blocked_categories.iter_mut() {
            if let Some(n) = rebase(r, &old_keys, &new_parts) {
                *r = n;
            }
        }
        for t in cfg.tag_map.iter_mut() {
            if let Some(n) = rebase(&t.target, &old_keys, &new_parts) {
                t.target = n;
            }
        }
    }
    Ok(rep)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DeleteReport {
    /// Lô di chuyển sách ra khỏi thư mục (hoàn tác được).
    pub moves: MoveReport,
    /// Thư mục đã vào Thùng rác.
    pub removed: Vec<String>,
    /// Thư mục giữ lại (còn sách chưa chuyển được / chưa quét tới), kèm lý do.
    pub kept: Vec<String>,
    pub cancelled: bool,
}

/// Bỏ khỏi cấu hình các mục chặn / ánh xạ tag nằm trong thư mục `rel` sắp xoá. Người gọi lưu cấu hình.
pub fn forget(cfg: &mut Config, rel: &str) -> Result<()> {
    let parts = check_rel(cfg, rel)?;
    let keys: Vec<String> = parts.iter().map(|p| blocked::name_key(p)).collect();
    let inside = |r: &str| {
        let k = rel_keys(r);
        k.len() >= keys.len() && k[..keys.len()] == keys[..]
    };
    cfg.blocked_categories.retain(|r| !inside(r));
    cfg.tag_map.retain(|t| !inside(&t.target));
    Ok(())
}

/// Xoá thư mục `rel` trên mọi ổ (xem mô tả module). Báo tiến trình `move` (x / y sách); dừng được trước mỗi sách.
pub fn delete(
    conn: &Connection,
    cfg: &Config,
    trasher: &dyn Trasher,
    rel: &str,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on: Option<crate::OnStep>,
) -> Result<DeleteReport> {
    let parts = check_rel(cfg, rel)?;
    let dirs = dirs_of(cfg, &parts.join("/"));
    let mut rep = DeleteReport::default();
    if dirs.is_empty() {
        return Ok(rep);
    }
    // Sách trong thư mục (kể cả thư mục con) → thể loại: `Khác` cùng ổ; thư mục con: thư mục cha.
    let books: Vec<Book> = db::books_where(conn, "status = 'present' ORDER BY path", [])?;
    let mut reqs = Vec::new();
    for (root, d) in &dirs {
        let target = if parts.len() == 1 {
            root.join(crate::triage::OTHER_DIR)
        } else {
            d.parent().unwrap_or(root).to_path_buf()
        };
        for b in books.iter().filter(|b| Path::new(&b.path).starts_with(d)) {
            reqs.push(MoveRequest { book_id: b.id, target_dir: target.clone() });
        }
    }
    if !reqs.is_empty() {
        let opts = MoveOptions { exact: true, cancel, progress: on };
        rep.moves = moves::apply_moves_with(conn, cfg, &reqs, DELETE_KIND, &opts)?;
        rep.cancelled = rep.moves.cancelled;
    }
    if rep.cancelled {
        return Ok(rep);
    }
    for (_, d) in &dirs {
        // Còn file sách (chuyển lỗi, hoặc chưa quét tới) → giữ thư mục, không bao giờ xoá sách.
        let left = walkdir::WalkDir::new(d)
            .into_iter()
            .flatten()
            .filter(|e| e.file_type().is_file() && crate::triage::is_book(e.path()))
            .count();
        if left > 0 {
            rep.kept.push(format!("{}: còn {left} file sách chưa chuyển ra được – thư mục được giữ lại", d.display()));
            continue;
        }
        crate::progress::working_on(d);
        match trasher.trash(d) {
            Ok(()) => rep.removed.push(d.to_string_lossy().into_owned()),
            Err(e) => rep.kept.push(format!("{}: {e}", d.display())),
        }
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        let c = Config::default();
        assert_eq!(validate_name(&c, "  Văn học  ").unwrap(), "Văn học");
        for bad in ["", "a/b", "a\\b", "x:y", "con", "COM1", "nul.txt", "abc.", ".an", "Chưa phân loại", "khac"] {
            assert!(validate_name(&c, bad).is_err(), "{bad}");
        }
        assert!(check_rel(&c, "Khác/T").is_err());
        assert!(check_rel(&c, "Chưa phân loại").is_err());
    }

    #[test]
    fn rebase_keeps_the_tail() {
        let old = rel_keys("Van hoc");
        let new = vec!["Văn học Việt Nam".to_string()];
        assert_eq!(rebase("Văn học/Nguyễn Nhật Ánh", &old, &new).as_deref(), Some("Văn học Việt Nam/Nguyễn Nhật Ánh"));
        assert_eq!(rebase("Văn học", &old, &new).as_deref(), Some("Văn học Việt Nam"));
        assert_eq!(rebase("Văn học nước ngoài", &old, &new), None);
    }
}
