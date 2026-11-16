//! Ánh xạ tag → thể loại: người dùng duyệt các tag (metadata `subject`) có trong thư viện và gán mỗi tag vào một
//! thư mục thể loại (kể cả thư mục con tự chia, vd. `Tiểu thuyết/Nước ngoài`).
//!
//! - Tag so khớp theo khoá [`tag_key`] (bỏ dấu, chữ thường, ký tự ngăn cách thành khoảng trắng): `Tiểu thuyết`,
//!   `tieu-thuyet`, `TIỂU THUYẾT` là một tag. Nhiều tag khác nhau (`novel`, `fiction`) gán được vào cùng thư mục.
//! - Đổi ánh xạ ([`apply_changes`]) → [`plan`] tìm sách mang tag đó và thư mục mới của chúng, [`apply`] chuyển (lô
//!   `tagmap` trong lịch sử, hoàn tác được). Chuyển được cả từ thư mục con lên thư mục cha và ngược lại.
//! - Sách mang nhiều tag được ánh xạ: các đích nằm trên một nhánh (`Tiểu thuyết` và `Tiểu thuyết/Nước ngoài`) thì
//!   chọn đích sâu nhất; khác nhánh thì là "xung đột" – không chuyển, chỉ báo để người dùng xử lý.
//! - Thư mục con không do ánh xạ quản lý (thư mục bộ / tác giả, vd. `Tiểu thuyết/Harry Potter`) đi theo sách:
//!   `Tiểu thuyết/Harry Potter/x.epub` → `Văn học nước ngoài/Harry Potter/x.epub`; sách đã nằm trong thư mục con như
//!   vậy của đúng đích thì để nguyên. Ngăn chữ cái của `Khác` thì không đi theo.
//! - Sách mới: tự phân loại hộp thư đến dùng ánh xạ trước các luật có sẵn (`triage.rs`), và sau mỗi lần quét, sách
//!   mới xuất hiện ở chỗ khác trong thư viện cũng được xếp theo ánh xạ ([`plan`] với `first_seen_scan`).
//!
//! Không đụng tới hộp thư đến (tự phân loại lo), thư mục bị chặn, sách ở thư mục nguồn phụ.

use crate::blocked::{self, Blocked};
use crate::config::{Config, TagRule};
use crate::db::{self, Book};
use crate::error::{Error, Result};
use crate::moves::{self, MoveReport, MoveRequest};
use crate::normalize::simple_key;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Lô trong lịch sử thao tác.
pub const TAGMAP_KIND: &str = "tagmap";

/// Khoá so khớp của một tag.
pub fn tag_key(tag: &str) -> String {
    simple_key(tag)
}

/// Tách trường `subjects` của sách thành từng tag (`;`, `,` hoặc `|`), bỏ khoảng trắng thừa và tag rỗng.
pub fn split_tags(subjects: &str) -> Vec<String> {
    subjects
        .split([';', ',', '|'])
        .map(str::trim)
        .filter(|t| !t.is_empty() && !tag_key(t).is_empty())
        .map(str::to_string)
        .collect()
}

fn components(rel: &str) -> Vec<String> {
    rel.split(['/', '\\']).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect()
}

/// `keys` nằm trong (hoặc là) thư mục có khoá `prefix`.
fn under(keys: &[String], prefix: &[String]) -> bool {
    keys.len() >= prefix.len() && keys[..prefix.len()] == prefix[..]
}

/// Chuẩn hoá thư mục đích `rel` (`A/B`) và kiểm tra dùng được không: không phải hộp thư đến, `Khác`, thư mục bị chặn.
pub fn normalize_target(cfg: &Config, rel: &str) -> Result<String> {
    let rel = blocked::normalize(cfg, rel).map_err(|e| match e {
        Error::Msg(m) => Error::msg(m.replace("Không chặn được", "Không gán tag vào")),
        e => e,
    })?;
    if Blocked::new(cfg).covers(&blocked::rel_keys(&rel)) {
        return Err(Error::msg(format!("“{rel}” đang bị chặn – không gán tag vào được")));
    }
    Ok(rel)
}

/// Ánh xạ đã chuẩn bị để tra.
#[derive(Debug, Clone, Default)]
pub struct Mapping {
    /// khoá tag → (đích như trong cấu hình, khoá từng cấp của đích, tag như người dùng gõ)
    by_tag: HashMap<String, (String, Vec<String>, String)>,
}

/// Kết quả tra đích của một cuốn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolve {
    /// Không tag nào được ánh xạ.
    None,
    /// Đích (đường dẫn tương đối) và tag quyết định.
    Target { rel: String, tag: String },
    /// Các tag chỉ vào những thư mục khác nhánh.
    Conflict(Vec<String>),
}

impl Mapping {
    pub fn new(cfg: &Config) -> Self {
        let by_tag = cfg
            .tag_map
            .iter()
            .filter_map(|r| {
                let k = tag_key(&r.tag);
                let t = components(&r.target).join("/");
                let tk = blocked::rel_keys(&t);
                (!k.is_empty() && !tk.is_empty()).then_some((k, (t, tk, r.tag.clone())))
            })
            .collect();
        Mapping { by_tag }
    }

    pub fn is_empty(&self) -> bool {
        self.by_tag.is_empty()
    }

    /// Khoá từng cấp của mọi thư mục đích.
    pub fn target_keys(&self) -> Vec<Vec<String>> {
        let mut v: Vec<Vec<String>> = self.by_tag.values().map(|(_, k, _)| k.clone()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Đích của một cuốn theo trường `subjects`.
    pub fn resolve(&self, subjects: &str) -> Resolve {
        // (đích, khoá đích, tag); mỗi đích một lần.
        let mut hits: Vec<(&str, &[String], &str)> = Vec::new();
        for t in split_tags(subjects) {
            if let Some((rel, keys, tag)) = self.by_tag.get(&tag_key(&t)) {
                if !hits.iter().any(|(_, k, _)| *k == keys.as_slice()) {
                    hits.push((rel, keys, tag));
                }
            }
        }
        let Some(deepest) = hits.iter().max_by_key(|(_, k, _)| k.len()).copied() else { return Resolve::None };
        if hits.iter().all(|(_, k, _)| under(deepest.1, k)) {
            Resolve::Target { rel: deepest.0.to_string(), tag: deepest.2.to_string() }
        } else {
            let mut v: Vec<String> = hits.iter().map(|(r, _, _)| r.to_string()).collect();
            v.sort();
            Resolve::Conflict(v)
        }
    }
}

/// Một thay đổi ánh xạ: `target = None` là bỏ ánh xạ tag đó.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub tag: String,
    pub target: Option<String>,
}

/// Áp các thay đổi vào cấu hình (người gọi lưu). Trả về các đích cũ của những tag bị đổi / bỏ (để [`plan`] biết thư
/// mục nào vừa thôi được quản lý – sách trong đó cần chuyển đi).
pub fn apply_changes(cfg: &mut Config, changes: &[Change]) -> Result<Vec<String>> {
    if cfg.read_only {
        return Err(Error::ReadOnly);
    }
    let mut olds = Vec::new();
    let mut rules = cfg.tag_map.clone();
    for c in changes {
        let tag = c.tag.trim();
        let k = tag_key(tag);
        if k.is_empty() {
            return Err(Error::msg("Tag rỗng"));
        }
        let target =
            c.target.as_deref().filter(|t| !t.trim().is_empty()).map(|t| normalize_target(cfg, t)).transpose()?;
        rules.retain(|r| {
            let same = tag_key(&r.tag) == k;
            if same && target.as_deref().is_none_or(|t| blocked::rel_keys(t) != blocked::rel_keys(&r.target)) {
                olds.push(r.target.clone());
            }
            !same
        });
        if let Some(target) = target {
            rules.push(TagRule { tag: tag.to_string(), target });
        }
    }
    rules.sort_by_key(|r| tag_key(&r.tag));
    cfg.tag_map = rules;
    Ok(olds)
}

/// Một tag có trong thư viện (hoặc đã được ánh xạ).
#[derive(Debug, Clone, Serialize)]
pub struct TagInfo {
    pub key: String,
    /// Cách viết phổ biến nhất (hoặc cách viết trong ánh xạ).
    pub label: String,
    /// Mọi cách viết gặp được (tối đa 8, phổ biến trước).
    pub variants: Vec<String>,
    /// Số sách đang mang tag.
    pub books: i64,
    /// Thư mục đang được gán.
    pub target: Option<String>,
    /// Sách mang tag đang nằm ở đâu (thư mục 2 cấp đầu, nhiều nhất trước, tối đa 4).
    pub folders: Vec<(String, i64)>,
}

/// Mọi tag của sách trong thư viện (kể cả hộp thư đến), nhiều sách trước; tag đã ánh xạ mà không còn sách nào vẫn có.
pub fn list_tags(conn: &Connection, cfg: &Config) -> Result<Vec<TagInfo>> {
    if cfg.library_roots().is_empty() {
        return Err(Error::NoLibrary);
    }
    #[derive(Default)]
    struct Acc {
        books: i64,
        variants: HashMap<String, i64>,
        folders: HashMap<String, i64>,
    }
    let mut acc: HashMap<String, Acc> = HashMap::new();
    let mut st = conn.prepare(
        "SELECT b.subjects, b.path FROM books b JOIN roots r ON r.id = b.root_id
         WHERE b.status = 'present' AND b.subjects <> '' AND r.kind = 'library'",
    )?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (subjects, path) = row?;
        crate::progress::pulse();
        let dir: Vec<String> = Path::new(&path)
            .parent()
            .and_then(|p| p.strip_prefix(cfg.library_root_of(p)?).ok().map(Path::to_path_buf))
            .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        let folder = if dir.first().is_some_and(|d| crate::triage::is_other_dir(d)) {
            crate::triage::OTHER_DIR.to_string()
        } else {
            dir.iter().take(2).cloned().collect::<Vec<_>>().join("/")
        };
        let mut seen = HashSet::new();
        for t in split_tags(&subjects) {
            let k = tag_key(&t);
            if !seen.insert(k.clone()) {
                continue;
            }
            let a = acc.entry(k).or_default();
            a.books += 1;
            *a.variants.entry(t).or_default() += 1;
            *a.folders.entry(folder.clone()).or_default() += 1;
        }
    }
    let mapping: HashMap<String, &TagRule> = cfg.tag_map.iter().map(|r| (tag_key(&r.tag), r)).collect();
    for (k, r) in &mapping {
        acc.entry(k.clone()).or_default().variants.entry(r.tag.clone()).or_default();
    }
    let top = |m: HashMap<String, i64>, n: usize| -> Vec<(String, i64)> {
        let mut v: Vec<(String, i64)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.truncate(n);
        v
    };
    let mut out: Vec<TagInfo> = acc
        .into_iter()
        .map(|(key, a)| {
            let rule = mapping.get(&key);
            let variants: Vec<String> = top(a.variants, 8).into_iter().map(|(v, _)| v).collect();
            TagInfo {
                label: rule.map(|r| r.tag.clone()).unwrap_or_else(|| variants[0].clone()),
                variants,
                books: a.books,
                target: rule.map(|r| r.target.clone()),
                folders: top(a.folders, 4),
                key,
            }
        })
        .collect();
    out.sort_by(|a, b| b.books.cmp(&a.books).then_with(|| a.key.cmp(&b.key)));
    Ok(out)
}

/// Phạm vi lập kế hoạch.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    /// Chỉ sách mang một trong các tag này (khoá hoặc cách viết bất kỳ); `None` = mọi tag đã ánh xạ.
    pub tags: Option<Vec<String>>,
    /// Đích cũ của các tag vừa đổi (xem [`apply_changes`]).
    pub old_targets: Vec<String>,
    /// Chỉ sách lần đầu thấy ở lần quét này.
    pub first_seen_scan: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedMove {
    /// Thư mục thư viện (ổ) đang chứa sách; `from` / `to` tương đối so với nó. Khi chuyển, sách ở lại ổ này trừ khi
    /// ổ sắp đầy (xem `volumes::Placer`).
    pub root: String,
    pub book_id: i64,
    pub title: String,
    pub file_name: String,
    /// Thư mục hiện tại (tương đối, `/`).
    pub from: String,
    /// Thư mục đích (tương đối, `/`).
    pub to: String,
    /// Tag quyết định.
    pub tag: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanConflict {
    pub book_id: i64,
    pub title: String,
    pub file_name: String,
    pub from: String,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Plan {
    pub moves: Vec<PlannedMove>,
    pub conflicts: Vec<PlanConflict>,
}

/// Các sách cần chuyển theo ánh xạ hiện tại của `cfg` (xem mô tả module). Không đụng tới đĩa.
pub fn plan(conn: &Connection, cfg: &Config, opts: &PlanOptions) -> Result<Plan> {
    if cfg.library_roots().is_empty() {
        return Err(Error::NoLibrary);
    }
    let mapping = Mapping::new(cfg);
    let mut out = Plan::default();
    if mapping.is_empty() {
        return Ok(out);
    }
    let blocked = Blocked::new(cfg);
    let wanted: Option<HashSet<String>> = opts.tags.as_ref().map(|v| v.iter().map(|t| tag_key(t)).collect());
    // Thư mục do ánh xạ quản lý: đích hiện tại + đích cũ của các tag vừa đổi.
    let mut managed = mapping.target_keys();
    managed.extend(opts.old_targets.iter().map(|t| blocked::rel_keys(t)).filter(|k| !k.is_empty()));
    let libs = crate::dedupe::library_root_ids(conn)?;
    if libs.is_empty() {
        return Ok(out);
    }
    let books: Vec<Book> = match opts.first_seen_scan {
        Some(s) => db::books_where(
            conn,
            "status = 'present' AND in_inbox = 0 AND subjects <> '' AND first_seen_scan = ?1 ORDER BY path",
            [s],
        )?,
        None => db::books_where(conn, "status = 'present' AND in_inbox = 0 AND subjects <> '' ORDER BY path", [])?,
    };
    for b in books.into_iter().filter(|b| libs.contains(&b.root_id)) {
        crate::progress::pulse();
        if let Some(w) = &wanted {
            if !split_tags(&b.subjects).iter().any(|t| w.contains(&tag_key(t))) {
                continue;
            }
        }
        let Some(dir) = Path::new(&b.path).parent() else { continue };
        let Some(root) = cfg.library_root_of(dir) else { continue };
        let names: Vec<String> = match dir.strip_prefix(&root) {
            Ok(r) => r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect(),
            Err(_) => continue,
        };
        let keys: Vec<String> = names.iter().map(|n| blocked::name_key(n)).collect();
        if blocked.covers(&keys) && !keys.is_empty() {
            continue; // sắp vào Thùng rác
        }
        let from = names.join("/");
        let (rel, tag) = match mapping.resolve(&b.subjects) {
            Resolve::None => continue,
            Resolve::Conflict(targets) => {
                if !targets.iter().any(|t| blocked::rel_keys(t) == keys) {
                    out.conflicts.push(PlanConflict {
                        book_id: b.id,
                        title: b.title.clone(),
                        file_name: b.file_name.clone(),
                        from,
                        targets,
                    });
                }
                continue;
            }
            Resolve::Target { rel, tag } => (rel, tag),
        };
        // Phần đường dẫn đi theo sách: dưới thư mục quản lý sâu nhất chứa sách (không có thì dưới thể loại cấp 1).
        let suffix: &[String] = if names.first().is_none_or(|n| crate::triage::is_other_dir(n)) {
            &[]
        } else {
            let base = managed.iter().filter(|m| under(&keys, m)).map(Vec::len).max().unwrap_or(1).min(names.len());
            &names[base..]
        };
        let mut to_names = components(&rel);
        to_names.extend(suffix.iter().cloned());
        let to_keys: Vec<String> = to_names.iter().map(|n| blocked::name_key(n)).collect();
        if to_keys == keys || blocked.covers(&to_keys) {
            continue;
        }
        out.moves.push(PlannedMove {
            root: root.to_string_lossy().into_owned(),
            book_id: b.id,
            title: b.title.clone(),
            file_name: b.file_name.clone(),
            from,
            to: to_names.join("/"),
            tag,
        });
    }
    Ok(out)
}

/// Chuyển sách theo kế hoạch (lô [`TAGMAP_KIND`], hoàn tác được), tạo thư mục đích nếu chưa có, rồi bỏ các thư mục
/// con không do ánh xạ quản lý đã rỗng hẳn (thư mục bộ / tác giả đã đi theo sách).
pub fn apply(conn: &Connection, cfg: &Config, plan: &Plan) -> Result<MoveReport> {
    if cfg.read_only {
        return Err(Error::ReadOnly);
    }
    if plan.moves.is_empty() {
        return Ok(MoveReport::default());
    }
    let mut dirs: BTreeMap<(String, String), PathBuf> = BTreeMap::new();
    let mut reqs = Vec::new();
    for m in &plan.moves {
        let k = (m.root.clone(), blocked::rel_keys(&m.to).join("/"));
        let dir = dirs.entry(k).or_insert_with(|| crate::volumes::dir_on(Path::new(&m.root), &m.to)).clone();
        reqs.push(MoveRequest { book_id: m.book_id, target_dir: dir });
    }
    let rep = moves::apply_moves(conn, cfg, &reqs, TAGMAP_KIND)?;
    // Dọn thư mục con đã rỗng (không bao giờ thư mục cấp 1 hay thư mục do ánh xạ quản lý).
    let managed: HashSet<Vec<String>> = Mapping::new(cfg).target_keys().into_iter().collect();
    moves::prune_empty_dirs(cfg, &rep.moved, &|keys| managed.contains(keys));
    Ok(rep)
}

/// Đích theo ánh xạ cho tác phẩm có `subjects` trong hộp thư đến (dùng khi tự phân loại).
pub fn triage_target(mapping: &Mapping, root: &Path, subjects: &str) -> Option<PathBuf> {
    match mapping.resolve(subjects) {
        Resolve::Target { rel, .. } => Some(crate::volumes::dir_on(root, &rel)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(rules: &[(&str, &str)]) -> Config {
        Config {
            tag_map: rules.iter().map(|(t, d)| TagRule { tag: t.to_string(), target: d.to_string() }).collect(),
            ..Config::default()
        }
    }

    #[test]
    fn tag_variants_share_a_key() {
        assert_eq!(tag_key("Tiểu thuyết"), tag_key("tieu-thuyet"));
        assert_eq!(tag_key("TIỂU  THUYẾT"), tag_key("tiểu_thuyết"));
        assert_ne!(tag_key("Tiểu thuyết"), tag_key("novel"));
        assert_eq!(split_tags("Tiểu thuyết; novel, Văn học | ; "), vec!["Tiểu thuyết", "novel", "Văn học"]);
    }

    #[test]
    fn resolve_prefers_deepest_on_one_branch_and_reports_conflicts() {
        let m = Mapping::new(&cfg(&[
            ("Tiểu thuyết", "Tiểu thuyết"),
            ("novel", "Tiểu thuyết"),
            ("foreign", "Tiểu thuyết/Nước ngoài"),
            ("mystery", "Trinh thám"),
        ]));
        assert_eq!(m.resolve("tieu-thuyet"), Resolve::Target { rel: "Tiểu thuyết".into(), tag: "Tiểu thuyết".into() });
        assert_eq!(
            m.resolve("Novel; Foreign"),
            Resolve::Target { rel: "Tiểu thuyết/Nước ngoài".into(), tag: "foreign".into() }
        );
        assert_eq!(m.resolve("Romance"), Resolve::None);
        assert_eq!(m.resolve("novel; mystery"), Resolve::Conflict(vec!["Tiểu thuyết".into(), "Trinh thám".into()]));
    }

    #[test]
    fn apply_changes_replaces_by_key_and_reports_old_targets() {
        let mut c = cfg(&[("Tiểu thuyết", "Tiểu thuyết")]);
        let olds = apply_changes(
            &mut c,
            &[
                Change { tag: "tieu-thuyet".into(), target: Some("Tiểu thuyết / Việt Nam".into()) },
                Change { tag: "novel".into(), target: Some("Tiểu thuyết".into()) },
            ],
        )
        .unwrap();
        assert_eq!(olds, vec!["Tiểu thuyết".to_string()]);
        assert_eq!(c.tag_map.len(), 2);
        assert!(c.tag_map.iter().any(|r| r.tag == "tieu-thuyet" && r.target == "Tiểu thuyết/Việt Nam"));
        let olds = apply_changes(&mut c, &[Change { tag: "NOVEL".into(), target: None }]).unwrap();
        assert_eq!(olds, vec!["Tiểu thuyết".to_string()]);
        assert_eq!(c.tag_map.len(), 1);
        assert!(apply_changes(&mut c, &[Change { tag: "x".into(), target: Some("Chưa phân loại".into()) }]).is_err());
        assert!(apply_changes(&mut c, &[Change { tag: "x".into(), target: Some("Khác".into()) }]).is_err());
        c.blocked_categories = vec!["Ngôn tình".into()];
        assert!(apply_changes(&mut c, &[Change { tag: "x".into(), target: Some("Ngôn tình/Đam mỹ".into()) }]).is_err());
    }
}
