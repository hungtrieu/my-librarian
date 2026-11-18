//! Đưa sách ra khỏi `Khác` khi sách **không có metadata** (không tag, không tác giả trong file).
//!
//! Hai cách, đều chạy nền, hoàn tác được, sách ở lại ổ của nó:
//!
//! 1. [`suggestions`] – sau khi người dùng chuyển tay vài cuốn từ `Khác` vào một thư mục: tìm trong `Khác` các cuốn
//!    "giống" chúng theo nhiều dấu hiệu, không chỉ tag:
//!    - **tag** (metadata `subject`), như trước;
//!    - **tác giả**: từ metadata, hoặc đoán từ tên file (`Mắt biếc - Nguyễn Nhật Ánh.epub`: phần tên trùng một tác
//!      giả đã biết trong thư viện);
//!    - **bộ / tên sách**: cùng tên bộ suy từ tên file (`Conan - Tập 03` ≡ `Conan 45`);
//!    - **cụm tên file**: một phần tên file (tách bằng ` - `, `_`, ngoặc…) xuất hiện ở nhiều file khác (thường là tên
//!      tác giả, tên bộ, nhà phát hành, người dịch).
//! 2. [`other_plan`] – màn "Phân loại Khác": mô hình học từ chính các thư mục thể loại (TF-IDF tên sách / tag, tác
//!    giả – kể cả tác giả đoán từ tên file; xem `classify.rs`) đoán thể loại cho từng cuốn trong `Khác`; người dùng
//!    chọn thể loại và ngưỡng tin cậy rồi chuyển cả loạt.

use crate::blocked;
use crate::classify::Model;
use crate::config::Config;
use crate::db::{self, Book};
use crate::error::{Error, Result};
use crate::normalize::{normalize_author, normalize_title, series_from_name, simple_key};
use crate::tagmap::{self, Plan, PlannedMove};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Lô trong Hoạt động khi chuyển sách tương tự / theo đề xuất ra khỏi `Khác`.
pub const OTHER_KIND: &str = "other_out";

/// Ký tự / cụm ngăn các phần của tên file.
const SEPARATORS: &[&str] = &[" - ", " – ", " — ", "_-_", " _ ", "__", " | ", "(", ")", "[", "]", "{", "}", "﹣"];

/// Các phần của tên file (không đuôi), giữ nguyên chữ: `Mắt biếc - Nguyễn Nhật Ánh (2019)` → `Mắt biếc`,
/// `Nguyễn Nhật Ánh`, `2019`.
pub fn name_parts(stem: &str) -> Vec<String> {
    let mut parts = vec![stem.to_string()];
    for sep in SEPARATORS {
        parts = parts.iter().flat_map(|p| p.split(sep).map(str::to_string).collect::<Vec<_>>()).collect();
    }
    parts
        .into_iter()
        .map(|p| p.trim().trim_matches(['-', '_', '.', ',']).trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Một phần tên file đủ "riêng" để so (≥ 2 từ có chữ, hoặc một từ ≥ 6 chữ cái; không phải số / tập).
fn distinctive(key: &str) -> bool {
    distinctive_min(key, 6)
}

/// Như [`distinctive`], một từ đơn cần ít nhất `min` chữ (tên bộ một từ như `Conan`, `Naruto` dùng 4).
fn distinctive_min(key: &str, min: usize) -> bool {
    let words: Vec<&str> = key.split(' ').filter(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit())).collect();
    !crate::subcats::is_generic_stem(key)
        && (words.len() >= 2 || words.first().is_some_and(|w| w.chars().count() >= min))
}

/// Tác giả đoán từ tên file: phần tên trùng (sau chuẩn hoá) một tác giả đã biết. `known`: khoá `normalize_author`.
pub fn author_from_name(stem: &str, known: &HashSet<String>) -> Option<String> {
    let parts = name_parts(stem);
    if parts.len() < 2 {
        return None;
    }
    parts.iter().map(|p| normalize_author(p)).find(|k| k.contains(' ') && known.contains(k))
}

/// Tác giả của sách (khoá `normalize_author`): metadata, không có thì đoán từ tên file.
pub fn author_key(b: &Book, known: &HashSet<String>) -> Option<String> {
    if !b.norm_author.is_empty() {
        return Some(b.norm_author.clone());
    }
    author_from_name(b.stem(), known)
}

/// Mọi tác giả (khoá) có trong thư viện – để nhận ra tên tác giả trong tên file.
pub fn known_authors(conn: &rusqlite::Connection) -> Result<HashSet<String>> {
    let mut st =
        conn.prepare("SELECT DISTINCT norm_author FROM books WHERE status = 'present' AND norm_author <> ''")?;
    let v = st
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .filter(|a| a.contains(' '))
        .collect::<HashSet<_>>();
    Ok(v)
}

/// Khoá bộ / tên sách để so: tên bộ trong metadata, không có thì suy từ tên file (bỏ số tập).
fn series_key(b: &Book) -> Option<String> {
    let k = match &b.series {
        Some(s) if !s.trim().is_empty() => normalize_title(s),
        _ => series_from_name(b.stem()),
    };
    distinctive_min(&k, 4).then_some(k)
}

/// Sách trong `Khác` kèm các dấu hiệu đã tính sẵn.
struct OtherBook {
    book: Book,
    root: PathBuf,
    tags: HashSet<String>,
    author: Option<String>,
    series: Option<String>,
    parts: HashSet<String>,
}

fn part_keys(stem: &str) -> HashSet<String> {
    name_parts(stem).iter().map(|p| simple_key(p)).filter(|k| distinctive(k)).collect()
}

fn other_books(conn: &rusqlite::Connection, cfg: &Config, known: &HashSet<String>) -> Result<Vec<OtherBook>> {
    let libs = crate::dedupe::library_root_ids(conn)?;
    Ok(db::books_where(conn, "status = 'present' AND in_inbox = 0 ORDER BY path", [])?
        .into_iter()
        .filter(|b| libs.contains(&b.root_id) && b.category.as_deref().is_some_and(crate::triage::is_other_dir))
        .filter_map(|b| {
            let root = cfg.library_root_of(Path::new(&b.path))?;
            Some(OtherBook {
                tags: tagmap::split_tags(&b.subjects).iter().map(|t| tagmap::tag_key(t)).collect(),
                author: author_key(&b, known),
                series: series_key(&b),
                parts: part_keys(b.stem()),
                root,
                book: b,
            })
        })
        .collect())
}

/// Một dấu hiệu "giống nhau".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Criterion {
    /// `tag` · `author` · `series` · `part`.
    pub kind: String,
    /// Khoá so khớp.
    pub key: String,
    /// Chữ hiện cho người dùng.
    #[serde(default)]
    pub label: String,
}

impl Criterion {
    fn matches(&self, o: &OtherBook) -> bool {
        match self.kind.as_str() {
            "tag" => o.tags.contains(&self.key),
            "author" => o.author.as_deref() == Some(self.key.as_str()),
            "series" => o.series.as_deref() == Some(self.key.as_str()),
            "part" => o.parts.contains(&self.key),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CriterionCount {
    #[serde(flatten)]
    pub criterion: Criterion,
    /// Số sách khác trong `Khác` khớp dấu hiệu này.
    pub books: usize,
    /// Vài tên sách khớp (để người dùng xem trước).
    pub samples: Vec<String>,
}

/// Gợi ý cho một thư mục đích.
#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    /// Thư mục đích (tương đối, `/`).
    pub target: String,
    pub criteria: Vec<CriterionCount>,
    /// Số sách khác nhau trong `Khác` khớp ít nhất một dấu hiệu.
    pub books: usize,
}

/// Số dấu hiệu tối đa đưa ra cho mỗi thư mục.
const MAX_CRITERIA: usize = 12;

/// Gợi ý sau khi người dùng chuyển tay sách: `moved` = (mã sách, đường dẫn cũ). Chỉ xét cuốn chuyển ra từ `Khác`.
pub fn suggestions(conn: &rusqlite::Connection, cfg: &Config, moved: &[(i64, String)]) -> Result<Vec<Suggestion>> {
    let known = known_authors(conn)?;
    // thư mục đích → (dấu hiệu, mã sách vừa chuyển)
    let mut by_target: Vec<(String, Vec<Criterion>, HashSet<i64>)> = Vec::new();
    for (id, from) in moved {
        let from = Path::new(from);
        let Some(root) = cfg.library_root_of(from) else { continue };
        let top = from.strip_prefix(&root).ok().and_then(|r| r.components().next());
        if !top.is_some_and(|c| crate::triage::is_other_dir(&c.as_os_str().to_string_lossy())) {
            continue;
        }
        let Some(b) = db::get_book(conn, *id)? else { continue };
        let Some(now_root) = cfg.library_root_of(Path::new(&b.path)) else { continue };
        let Some(target) = Path::new(&b.path)
            .parent()
            .and_then(|d| d.strip_prefix(&now_root).ok())
            .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
            .filter(|t| !t.is_empty())
        else {
            continue;
        };
        let tk = blocked::rel_keys(&target);
        let idx = match by_target.iter().position(|(t, _, _)| blocked::rel_keys(t) == tk) {
            Some(i) => i,
            None => {
                by_target.push((target, Vec::new(), HashSet::new()));
                by_target.len() - 1
            }
        };
        let entry = &mut by_target[idx];
        entry.2.insert(*id);
        let mut add = |c: Criterion| {
            if !entry.1.iter().any(|x| x.kind == c.kind && x.key == c.key) {
                entry.1.push(c);
            }
        };
        for t in tagmap::split_tags(&b.subjects) {
            add(Criterion { kind: "tag".into(), key: tagmap::tag_key(&t), label: t });
        }
        let author = author_key(&b, &known);
        if let Some(a) = &author {
            let label = if b.authors.trim().is_empty() {
                name_parts(b.stem()).into_iter().find(|p| normalize_author(p) == *a).unwrap_or_else(|| a.clone())
            } else {
                b.authors.clone()
            };
            add(Criterion { kind: "author".into(), key: a.clone(), label });
        }
        let series = series_key(&b);
        if let Some(s) = &series {
            let label = b
                .series
                .clone()
                .unwrap_or_else(|| name_parts(b.stem()).into_iter().next().unwrap_or_else(|| b.title.clone()));
            add(Criterion { kind: "series".into(), key: s.clone(), label });
        }
        for p in name_parts(b.stem()) {
            let k = simple_key(&p);
            if !distinctive(&k)
                || series.as_deref() == Some(k.as_str())
                || author.as_deref() == Some(normalize_author(&p).as_str())
            {
                continue;
            }
            add(Criterion { kind: "part".into(), key: k, label: p });
        }
    }
    if by_target.is_empty() {
        return Ok(Vec::new());
    }
    let other = other_books(conn, cfg, &known)?;
    let mut out = Vec::new();
    for (target, crits, ids) in by_target {
        let mut list: Vec<CriterionCount> = crits
            .into_iter()
            .map(|c| {
                let hits: Vec<&OtherBook> =
                    other.iter().filter(|o| !ids.contains(&o.book.id) && c.matches(o)).collect();
                CriterionCount {
                    books: hits.len(),
                    samples: hits.iter().take(5).map(|o| o.book.title.clone()).collect(),
                    criterion: c,
                }
            })
            .filter(|c| c.books > 0)
            .collect();
        if list.is_empty() {
            continue;
        }
        // Dấu hiệu mạnh trước (tác giả, bộ, tag), rồi nhiều sách trước.
        let rank = |k: &str| match k {
            "author" => 0,
            "series" => 1,
            "tag" => 2,
            _ => 3,
        };
        list.sort_by(|a, b| rank(&a.criterion.kind).cmp(&rank(&b.criterion.kind)).then_with(|| b.books.cmp(&a.books)));
        list.truncate(MAX_CRITERIA);
        let books =
            other.iter().filter(|o| !ids.contains(&o.book.id) && list.iter().any(|c| c.criterion.matches(o))).count();
        out.push(Suggestion { target, criteria: list, books });
    }
    Ok(out)
}

fn rel_of(root: &Path, path: &Path) -> String {
    path.parent()
        .and_then(|d| d.strip_prefix(root).ok())
        .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
        .unwrap_or_default()
}

/// Kế hoạch chuyển mọi sách trong `Khác` khớp ít nhất một dấu hiệu vào `target` (tương đối).
pub fn plan_similar(conn: &rusqlite::Connection, cfg: &Config, target: &str, criteria: &[Criterion]) -> Result<Plan> {
    let target = tagmap::normalize_target(cfg, target)?;
    let known = known_authors(conn)?;
    let mut out = Plan::default();
    for o in other_books(conn, cfg, &known)? {
        crate::progress::pulse();
        let Some(c) = criteria.iter().find(|c| c.matches(&o)) else { continue };
        out.moves.push(PlannedMove {
            from: rel_of(&o.root, Path::new(&o.book.path)),
            root: o.root.to_string_lossy().into_owned(),
            book_id: o.book.id,
            title: o.book.title.clone(),
            file_name: o.book.file_name.clone(),
            to: target.clone(),
            tag: if c.label.is_empty() { c.key.clone() } else { c.label.clone() },
        });
    }
    Ok(out)
}

// ------------------------------------------------------------------ màn "Phân loại Khác"

/// Một cuốn trong `Khác` được mô hình đoán thể loại.
#[derive(Debug, Clone, Serialize)]
pub struct OtherGuess {
    pub book_id: i64,
    pub title: String,
    pub file_name: String,
    pub confidence: u8,
    pub reason: String,
}

/// Các cuốn đoán thuộc một thể loại.
#[derive(Debug, Clone, Serialize)]
pub struct OtherGroup {
    /// Thể loại (tên thư mục cấp 1).
    pub target: String,
    pub books: usize,
    /// Số cuốn tin cậy cao (≥ `classify_high`).
    pub high: usize,
    /// Vài cuốn tin cậy nhất (xem trước).
    pub samples: Vec<OtherGuess>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OtherView {
    /// Tổng số sách trong `Khác`.
    pub total: usize,
    /// Số cuốn đoán được thể loại (≥ ngưỡng).
    pub guessed: usize,
    /// Số sách đã nằm trong thư mục thể loại (dữ liệu học).
    pub training: usize,
    pub groups: Vec<OtherGroup>,
}

/// Một dự đoán: (chỉ số trong `other`, chỉ số thể loại, độ tin cậy, lý do).
type Guess = (usize, usize, u8, String);

/// Đoán thể loại cho mọi sách trong `Khác` (không đụng tới đĩa). Chỉ giữ dự đoán ≥ `min_conf`.
fn guesses(conn: &rusqlite::Connection, cfg: &Config, min_conf: u8) -> Result<(Model, Vec<OtherBook>, Vec<Guess>)> {
    let known = known_authors(conn)?;
    let model = Model::build_excluding_other(conn, cfg, &known)?;
    let other = other_books(conn, cfg, &known)?;
    let mut v = Vec::new();
    for (i, o) in other.iter().enumerate() {
        crate::progress::pulse();
        let p = model.predict_with_author(&o.book, o.author.as_deref());
        if let Some(c) = p.category {
            if p.confidence >= min_conf {
                v.push((i, c, p.confidence, p.reason));
            }
        }
    }
    Ok((model, other, v))
}

/// Xem trước màn "Phân loại Khác": mỗi thể loại bao nhiêu cuốn được đoán (≥ `min_conf`), kèm ví dụ.
pub fn other_view(conn: &rusqlite::Connection, cfg: &Config, min_conf: u8) -> Result<OtherView> {
    let (model, other, gs) = guesses(conn, cfg, min_conf)?;
    let mut by: HashMap<usize, Vec<(usize, u8, String)>> = HashMap::new();
    for (i, c, conf, reason) in gs.iter().cloned() {
        by.entry(c).or_default().push((i, conf, reason));
    }
    let mut groups: Vec<OtherGroup> = by
        .into_iter()
        .map(|(c, mut v)| {
            v.sort_by_key(|a| std::cmp::Reverse(a.1));
            OtherGroup {
                target: model.categories[c].name.clone(),
                books: v.len(),
                high: v.iter().filter(|x| x.1 >= cfg.classify_high).count(),
                samples: v
                    .iter()
                    .take(30)
                    .map(|(i, conf, reason)| OtherGuess {
                        book_id: other[*i].book.id,
                        title: other[*i].book.title.clone(),
                        file_name: other[*i].book.file_name.clone(),
                        confidence: *conf,
                        reason: reason.clone(),
                    })
                    .collect(),
            }
        })
        .collect();
    groups.sort_by(|a, b| b.books.cmp(&a.books).then_with(|| a.target.cmp(&b.target)));
    Ok(OtherView { total: other.len(), guessed: gs.len(), training: model.training_docs, groups })
}

/// Kế hoạch chuyển các cuốn trong `Khác` được đoán thuộc một trong `targets` (tên thể loại) với độ tin cậy ≥ `min_conf`.
/// `exclude`: mã sách người dùng bỏ tick ở phần xem trước.
pub fn other_plan(
    conn: &rusqlite::Connection,
    cfg: &Config,
    targets: &[String],
    min_conf: u8,
    exclude: &[i64],
) -> Result<Plan> {
    if targets.is_empty() {
        return Err(Error::msg("Chưa chọn thể loại nào"));
    }
    let want: HashSet<String> = targets.iter().map(|t| crate::normalize::fold(t)).collect();
    let exclude: HashSet<i64> = exclude.iter().copied().collect();
    let (model, other, gs) = guesses(conn, cfg, min_conf)?;
    let mut out = Plan::default();
    for (i, c, conf, _) in gs {
        let cat = &model.categories[c];
        let o = &other[i];
        if !want.contains(&cat.key) || exclude.contains(&o.book.id) {
            continue;
        }
        out.moves.push(PlannedMove {
            from: rel_of(&o.root, Path::new(&o.book.path)),
            root: o.root.to_string_lossy().into_owned(),
            book_id: o.book.id,
            title: o.book.title.clone(),
            file_name: o.book.file_name.clone(),
            to: cat.name.clone(),
            tag: format!("đoán {conf}%"),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_parts_and_authors() {
        assert_eq!(name_parts("Mắt biếc - Nguyễn Nhật Ánh (2019)"), vec!["Mắt biếc", "Nguyễn Nhật Ánh", "2019"]);
        assert_eq!(name_parts("Conan_-_Tap_03"), vec!["Conan", "Tap_03"]);
        let known: HashSet<String> = [normalize_author("Nguyễn Nhật Ánh")].into_iter().collect();
        assert_eq!(author_from_name("Mắt biếc - Nguyen Nhat Anh", &known), Some(normalize_author("Nguyễn Nhật Ánh")));
        assert_eq!(author_from_name("Nguyễn Nhật Ánh", &known), None, "cả tên file là một phần: không đoán");
        assert!(distinctive("nguyen nhat anh"));
        assert!(distinctive("doraemon"));
        assert!(!distinctive("2019"));
        assert!(!distinctive("tap 3"));
        assert!(!distinctive("abc"));
    }
}
