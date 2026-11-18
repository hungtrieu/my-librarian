//! Thư viện trải trên nhiều ổ: thư mục chính (vd. `D:\books`) + các thư mục thêm (vd. `G:\sach`).
//!
//! - Mỗi thư mục thư viện có cùng cấu trúc: thể loại ở cấp 1, hộp thư đến riêng. Thể loại cùng tên trên các ổ là
//!   một thể loại trong app (`classify::list_categories` gộp lại).
//! - Xếp sách ([`Placer`]): sách **không bao giờ tự chuyển qua ổ khác** – phân loại, ánh xạ tag, chuyển tay đều
//!   dùng thư mục tương ứng trên chính ổ đang chứa sách (tạo nếu chưa có). Chuyển trong cùng ổ chỉ là đổi tên: tức
//!   thì, không chép dữ liệu, không thể làm hỏng file, không tốn thêm chỗ. Ổ sắp đầy (dưới `min_free_gb`) chỉ được
//!   cảnh báo; chuyển sang ổ khác chỉ xảy ra khi người dùng chủ động cân bằng. Sách từ thư mục nguồn phụ ở ổ không có
//!   thư mục thư viện nào thì buộc phải chép: vào ổ đã có thư mục đích và còn chỗ, không thì ổ trống nhất.
//! - Cân bằng ([`balance_plan`]): ổ dưới mức tối thiểu → đề xuất chuyển nguyên thư mục (bộ / tác giả / thể loại
//!   con), thư mục lớn trước, sang ổ còn chỗ, giữ nguyên đường dẫn tương đối. Chép an toàn từng file (file tạm →
//!   so SHA-256 hai bản → đổi tên → mới xoá bản gốc, xem `moves::move_file`); chạy nền, dừng / làm tiếp được,
//!   hoàn tác được.

use crate::config::{same_path, Config};
use crate::error::{Error, Result};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const GB: u64 = 1024 * 1024 * 1024;

/// (tổng dung lượng, còn trống) của ổ chứa `path`.
pub fn space(path: &Path) -> Option<(u64, u64)> {
    Some((fs4::total_space(path).ok()?, fs4::available_space(path).ok()?))
}

/// Mã ổ / phân vùng chứa `path` (đường dẫn chưa có thì xét thư mục cha gần nhất đã có). Hai đường dẫn cùng mã thì
/// chuyển giữa chúng chỉ là đổi tên, không phải chép dữ liệu.
pub fn volume_id(path: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        match path.components().next() {
            Some(std::path::Component::Prefix(p)) => Some(p.as_os_str().to_string_lossy().to_uppercase()),
            _ => None,
        }
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::MetadataExt;
        path.ancestors().find_map(|a| std::fs::metadata(a).ok()).map(|m| m.dev().to_string())
    }
}

/// Hai đường dẫn nằm trên cùng một ổ.
pub fn same_volume(a: &Path, b: &Path) -> bool {
    matches!((volume_id(a), volume_id(b)), (Some(x), Some(y)) if x == y)
}

/// Kiểm tra `root` dùng được làm thư mục thư viện thêm: có thật, chưa có, không lồng trong / chứa thư mục thư viện
/// khác (sách sẽ bị quét hai lần).
pub fn check_new_root(cfg: &Config, root: &Path) -> Result<()> {
    if !root.is_dir() {
        return Err(Error::msg(format!("{} không phải thư mục", root.display())));
    }
    for r in cfg.library_roots() {
        if same_path(&r, root) {
            return Err(Error::msg(format!("{} đã là thư mục thư viện", root.display())));
        }
        if root.starts_with(&r) || r.starts_with(root) {
            return Err(Error::msg(format!(
                "{} lồng với thư mục thư viện {} – hãy chọn thư mục riêng (thường ở ổ khác)",
                root.display(),
                r.display()
            )));
        }
    }
    Ok(())
}

/// Một ổ / thư mục thư viện.
#[derive(Debug, Clone, Serialize)]
pub struct Volume {
    pub path: String,
    /// Thư mục thư viện chính.
    pub primary: bool,
    /// Đang truy cập được (ổ rời có thể chưa cắm).
    pub online: bool,
    pub total_bytes: u64,
    pub free_bytes: u64,
    /// Dưới mức trống tối thiểu.
    pub low: bool,
    /// Số sách / dung lượng sách của thư viện trên ổ này.
    pub books: i64,
    pub book_bytes: i64,
    /// Sách đang chờ trong hộp thư đến của ổ này.
    pub inbox: i64,
}

/// Các thư mục thư viện kèm dung lượng ổ và số sách.
pub fn list(conn: &Connection, cfg: &Config) -> Result<Vec<Volume>> {
    let reserve = cfg.min_free_gb.saturating_mul(GB);
    let mut out = Vec::new();
    for (i, root) in cfg.library_roots().into_iter().enumerate() {
        let path = root.to_string_lossy().into_owned();
        let (books, book_bytes, inbox): (i64, i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(b.size), 0), COALESCE(SUM(b.in_inbox), 0) FROM books b
                 JOIN roots r ON r.id = b.root_id WHERE r.path = ?1 AND b.status = 'present'",
                [&path],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap_or((0, 0, 0));
        let online = root.is_dir();
        let (total_bytes, free_bytes) = if online { space(&root).unwrap_or((0, 0)) } else { (0, 0) };
        out.push(Volume {
            path,
            primary: i == 0 && cfg.library_root.is_some(),
            online,
            total_bytes,
            free_bytes,
            low: online && total_bytes > 0 && free_bytes < reserve,
            books,
            book_bytes,
            inbox,
        });
    }
    Ok(out)
}

/// Đường dẫn tương đối dạng `A/B` của `dir` dưới `root`.
fn rel_string(root: &Path, dir: &Path) -> Option<String> {
    let r = dir.strip_prefix(root).ok()?;
    Some(r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
}

/// Thư mục `rel` trên `root`: thư mục có sẵn (không phân biệt dấu / hoa thường) cho phần đã có, phần còn lại theo
/// tên trong `rel`.
pub fn dir_on(root: &Path, rel: &str) -> PathBuf {
    let parts: Vec<&str> = rel.split(['/', '\\']).filter(|c| !c.is_empty()).collect();
    for n in (1..=parts.len()).rev() {
        if let Some(d) = crate::blocked::find_dir(root, &parts[..n].join("/")) {
            return parts[n..].iter().fold(d, |p, c| p.join(c));
        }
    }
    parts.iter().fold(root.to_path_buf(), |p, c| p.join(c))
}

/// Chọn ổ cho sách sắp chuyển (xem mô tả module). Đọc dung lượng trống một lần rồi tự trừ / cộng theo các lần chọn,
/// để một lô lớn không dồn hết vào một ổ.
pub struct Placer {
    roots: Vec<PathBuf>,
    /// Ổ của từng thư mục thư viện (đọc khi cần).
    vols: Vec<Option<Option<String>>>,
    /// Còn trống (byte); `None` = không biết (ổ chưa cắm / không đọc được) – không bao giờ chọn sang ổ đó.
    free: Vec<Option<u64>>,
    reserve: u64,
}

impl Placer {
    pub fn new(cfg: &Config) -> Self {
        let roots = cfg.library_roots();
        let free = roots.iter().map(|r| r.is_dir().then(|| space(r).map(|s| s.1)).flatten()).collect();
        let vols = vec![None; roots.len()];
        Placer { roots, vols, free, reserve: cfg.min_free_gb.saturating_mul(GB) }
    }

    /// Dùng trong test: dung lượng trống cho sẵn.
    pub fn with_free(roots: Vec<PathBuf>, free: Vec<Option<u64>>, reserve: u64) -> Self {
        let vols = vec![None; roots.len()];
        Placer { roots, vols, free, reserve }
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Chỉ số thư mục thư viện chứa `path`.
    pub fn root_index(&self, path: &Path) -> Option<usize> {
        (0..self.roots.len())
            .filter(|&i| path.starts_with(&self.roots[i]))
            .max_by_key(|&i| self.roots[i].components().count())
    }

    /// Ổ (phân vùng) của thư mục thư viện `i`.
    fn vol(&mut self, i: usize) -> Option<String> {
        if self.vols[i].is_none() {
            self.vols[i] = Some(volume_id(&self.roots[i]));
        }
        self.vols[i].clone().flatten()
    }

    /// Thư mục thật cho file `src` (`size` byte) sắp chuyển vào thư mục (logic) `rel` của thư viện.
    ///
    /// **Không bao giờ chọn ổ khác với ổ đang chứa file** khi có thư mục thư viện trên cùng ổ: chuyển trong cùng ổ chỉ
    /// là đổi tên – tức thì, không chép dữ liệu, không thể làm hỏng file. Ổ sắp đầy cũng vậy (chỉ cảnh báo ở màn "Ổ
    /// lưu sách"; chuyển bớt sang ổ khác là việc người dùng tự chọn). Chỉ file nằm ở ổ không có thư mục thư viện nào
    /// (thư mục nguồn phụ) mới phải chép: vào ổ đã có thư mục đích và còn chỗ, không thì ổ trống nhất.
    pub fn place_rel(&mut self, src: &Path, size: u64, rel: &str) -> PathBuf {
        let pick = match self.root_index(src) {
            Some(h) => h,
            None => {
                let sv = volume_id(src);
                let same: Vec<usize> = (0..self.roots.len()).filter(|&i| sv.is_some() && self.vol(i) == sv).collect();
                let has = |i: &usize| !rel.is_empty() && crate::blocked::find_dir(&self.roots[*i], rel).is_some();
                if let Some(i) = same.iter().copied().find(has).or_else(|| same.first().copied()) {
                    i
                } else {
                    let ok: Vec<usize> = (0..self.roots.len())
                        .filter(|&i| self.free[i].is_some_and(|f| f >= self.reserve.saturating_add(size)))
                        .collect();
                    let i = ok
                        .iter()
                        .copied()
                        .filter(has)
                        .max_by_key(|&i| self.free[i])
                        .or_else(|| ok.iter().copied().max_by_key(|&i| self.free[i]))
                        .or_else(|| {
                            (0..self.roots.len()).filter(|&i| self.free[i].is_some()).max_by_key(|&i| self.free[i])
                        })
                        .unwrap_or(0);
                    if let Some(f) = self.free[i].as_mut() {
                        *f = f.saturating_sub(size);
                    }
                    i
                }
            }
        };
        dir_on(&self.roots[pick], rel)
    }

    /// Như [`Placer::place_rel`] với thư mục đích `target_dir` (đường dẫn thật trên một ổ thư viện bất kỳ): đổi sang
    /// thư mục tương ứng trên ổ nên dùng. Thư mục ngoài thư viện thì giữ nguyên.
    pub fn place(&mut self, src: &Path, size: u64, target_dir: &Path) -> PathBuf {
        match self.root_index(target_dir).and_then(|i| rel_string(&self.roots[i], target_dir)) {
            Some(rel) => self.place_rel(src, size, &rel),
            None => target_dir.to_path_buf(),
        }
    }
}

/// Một thư mục đề xuất chuyển sang ổ khác.
#[derive(Debug, Clone, Serialize)]
pub struct BalanceGroup {
    /// Thư mục (tương đối, `/`).
    pub rel: String,
    pub from_root: String,
    pub to_root: String,
    pub books: usize,
    pub bytes: u64,
    pub book_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BalancePlan {
    pub groups: Vec<BalanceGroup>,
    pub bytes: u64,
    /// Số byte cần chuyển để mọi ổ về trên mức tối thiểu (0 = không ổ nào thiếu).
    pub needed: u64,
}

/// Đề xuất chuyển bớt sách từ ổ dưới mức tối thiểu sang ổ còn chỗ. `want` (byte, tuỳ chọn): chuyển ít nhất chừng
/// này khỏi ổ `from` (đường dẫn thư mục thư viện) dù ổ chưa thiếu chỗ.
///
/// Đơn vị chuyển là cả thư mục chứa sách (mọi định dạng, mọi tập của một bộ đi cùng nhau); thư mục lớn trước, bỏ
/// qua thư mục không vừa ổ đích. Không đụng hộp thư đến và thư mục bị chặn.
pub fn balance_plan(conn: &Connection, cfg: &Config, from: Option<&Path>, want: Option<u64>) -> Result<BalancePlan> {
    let roots = cfg.library_roots();
    if roots.len() < 2 {
        return Ok(BalancePlan::default());
    }
    let reserve = cfg.min_free_gb.saturating_mul(GB);
    let mut free: Vec<Option<u64>> =
        roots.iter().map(|r| r.is_dir().then(|| space(r).map(|s| s.1)).flatten()).collect();
    let blocked = crate::blocked::Blocked::new(cfg);
    let inbox = crate::blocked::name_key(&cfg.inbox_name);
    let mut plan = BalancePlan::default();
    for (si, src) in roots.iter().enumerate() {
        let Some(sfree) = free[si] else { continue };
        let need = match (from, want) {
            (Some(f), Some(w)) if same_path(f, src) => w,
            (Some(_), _) => continue,
            _ => reserve.saturating_sub(sfree),
        };
        if need == 0 {
            continue;
        }
        plan.needed += reserve.saturating_sub(sfree);
        // Sách theo thư mục chứa.
        let src_s = src.to_string_lossy().into_owned();
        let mut st = conn.prepare(
            "SELECT b.id, b.path, b.size FROM books b JOIN roots r ON r.id = b.root_id
             WHERE r.path = ?1 AND b.status = 'present' AND b.in_inbox = 0",
        )?;
        let rows =
            st.query_map([&src_s], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
        let mut groups: BTreeMap<String, (Vec<i64>, u64)> = BTreeMap::new();
        for row in rows {
            let (id, path, size) = row?;
            let Some(rel) = Path::new(&path).parent().and_then(|d| rel_string(src, d)) else { continue };
            let keys = crate::blocked::rel_keys(&rel);
            if keys.is_empty() || keys[0] == inbox || blocked.covers(&keys) {
                continue;
            }
            let g = groups.entry(rel).or_default();
            g.0.push(id);
            g.1 += size.max(0) as u64;
        }
        let mut list: Vec<(String, (Vec<i64>, u64))> = groups.into_iter().collect();
        list.sort_by(|a, b| b.1 .1.cmp(&a.1 .1).then_with(|| a.0.cmp(&b.0)));
        let mut moved = 0u64;
        for (rel, (ids, bytes)) in list {
            if moved >= need {
                break;
            }
            // Ổ đích: còn chỗ cho cả thư mục mà vẫn trên mức tối thiểu; ưu tiên ổ đã có thư mục cha, rồi ổ trống nhất.
            let parent_top = rel.split('/').next().unwrap_or_default().to_string();
            let cands: Vec<usize> = (0..roots.len())
                .filter(|&i| i != si && free[i].is_some_and(|f| f >= reserve.saturating_add(bytes)))
                .collect();
            let has = |i: &usize| crate::blocked::find_dir(&roots[*i], &parent_top).is_some();
            let Some(di) = cands
                .iter()
                .copied()
                .filter(has)
                .max_by_key(|&i| free[i])
                .or_else(|| cands.iter().copied().max_by_key(|&i| free[i]))
            else {
                continue;
            };
            if let Some(f) = free[di].as_mut() {
                *f -= bytes;
            }
            moved += bytes;
            plan.bytes += bytes;
            plan.groups.push(BalanceGroup {
                rel,
                from_root: src_s.clone(),
                to_root: roots[di].to_string_lossy().into_owned(),
                books: ids.len(),
                bytes,
                book_ids: ids,
            });
        }
    }
    Ok(plan)
}

/// Yêu cầu chuyển (đích chính xác, không chọn lại ổ) cho kế hoạch cân bằng.
pub fn balance_requests(plan: &BalancePlan) -> Vec<crate::moves::MoveRequest> {
    plan.groups
        .iter()
        .flat_map(|g| {
            let dir = dir_on(Path::new(&g.to_root), &g.rel);
            g.book_ids.iter().map(move |&id| crate::moves::MoveRequest { book_id: id, target_dir: dir.clone() })
        })
        .collect()
}

/// Lô trong lịch sử thao tác.
pub const BALANCE_KIND: &str = "balance";

/// Thực hiện kế hoạch cân bằng: chuyển đúng vào ổ đã chọn (chép → kiểm tra dung lượng → xoá bản cũ), dừng được
/// giữa chừng, rồi bỏ các thư mục rỗng ở ổ cũ. Lô [`BALANCE_KIND`], hoàn tác được.
pub fn apply_balance(
    conn: &Connection,
    cfg: &Config,
    plan: &BalancePlan,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on: Option<crate::OnStep>,
) -> Result<crate::moves::MoveReport> {
    let reqs = balance_requests(plan);
    if reqs.is_empty() {
        return Ok(Default::default());
    }
    let opts = crate::moves::MoveOptions { exact: true, cancel, progress: on, names: None };
    let rep = crate::moves::apply_moves_with(conn, cfg, &reqs, BALANCE_KIND, &opts)?;
    crate::moves::prune_empty_dirs(cfg, &rep.moved, &|_| false);
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placer_never_leaves_the_drive_of_a_library_book() {
        let t = tempfile::tempdir().unwrap();
        let (d, g) = (t.path().join("d"), t.path().join("g"));
        std::fs::create_dir_all(d.join("Văn học")).unwrap();
        std::fs::create_dir_all(g.join("Văn học").join("Nguyễn Nhật Ánh")).unwrap();
        for free in [Some(100 * GB), Some(GB)] {
            // Kể cả khi D sắp đầy (dưới mức 20 GB) và G còn rất nhiều chỗ: sách trên D vẫn ở lại D.
            let mut p = Placer::with_free(vec![d.clone(), g.clone()], vec![free, Some(900 * GB)], 20 * GB);
            assert_eq!(p.place(&d.join("Chưa phân loại/x.epub"), GB, &g.join("Văn học")), d.join("Văn học"));
            assert_eq!(
                p.place_rel(&d.join("Chưa phân loại/x.epub"), GB, "van hoc/Nguyễn Nhật Ánh"),
                d.join("Văn học").join("Nguyễn Nhật Ánh"),
                "thư mục con chỉ có ở G được tạo ở D"
            );
            assert_eq!(p.place_rel(&g.join("Chưa phân loại/y.epub"), GB, "Văn học"), g.join("Văn học"));
        }
    }

    #[test]
    fn placer_prefers_a_library_on_the_same_drive_for_outside_books() {
        let t = tempfile::tempdir().unwrap();
        let (d, g) = (t.path().join("d"), t.path().join("g"));
        std::fs::create_dir_all(g.join("Văn học").join("Nguyễn Nhật Ánh")).unwrap();
        std::fs::create_dir_all(&d).unwrap();
        // Thư mục nguồn phụ cùng ổ (test: cùng một đĩa) → không chép, dùng thư viện đã có thư mục đích.
        let mut p = Placer::with_free(vec![d.clone(), g.clone()], vec![Some(100 * GB), Some(50 * GB)], 20 * GB);
        assert_eq!(
            p.place_rel(&t.path().join("nguon/x.epub"), GB, "Văn học/Nguyễn Nhật Ánh"),
            g.join("Văn học/Nguyễn Nhật Ánh")
        );
        assert!(same_volume(&d, &t.path().join("nguon/chua-co")));
    }
}
