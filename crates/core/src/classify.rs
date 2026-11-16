//! Phân loại theo thư mục có sẵn (E6-01..E6-04, E6-10).
//! - Danh mục = thư mục con cấp 1 của `books`, trừ hộp thư đến. Không bao giờ tạo thư mục mới.
//! - Chỉ đề xuất cho sách trong hộp thư đến; không đụng tới sách đã ở thư mục thể loại.
//! - Học từ chính kho: hồ sơ TF-IDF mỗi thư mục + tác giả trùng + từ khoá trong tên thư mục.

use crate::config::Config;
use crate::db::{self, Book};
use crate::error::{Error, Result};
use crate::normalize::{fold, normalize_title, series_from_name, simple_key};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct SeriesDir {
    pub name: String,
    pub key: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct Category {
    /// Tên thật của thư mục (hiển thị đúng như trên đĩa, có thể không dấu).
    pub name: String,
    pub key: String,
    pub path: PathBuf,
    pub series_dirs: Vec<SeriesDir>,
}

fn subdirs(p: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(p)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .filter(|(n, _)| !n.starts_with('.') && !n.starts_with('$') && n != "System Volume Information")
        .collect();
    v.sort();
    v
}

/// Danh mục đọc trực tiếp từ đĩa (E6-01).
pub fn list_categories(cfg: &Config) -> Result<Vec<Category>> {
    let root = cfg.library_root.as_ref().ok_or(Error::NoLibrary)?;
    let inbox = fold(&cfg.inbox_name);
    Ok(subdirs(root)
        .into_iter()
        .filter(|(n, _)| fold(n) != inbox)
        .map(|(name, path)| Category {
            key: fold(&name),
            series_dirs: subdirs(&path)
                .into_iter()
                // thư mục chia nhỏ `Khác` (`A`…`Z`, `0-9`, `#`) không phải thư mục bộ
                .filter(|(n, _)| !(crate::triage::is_other_dir(&name) && crate::triage::is_other_bucket(n)))
                .map(|(n, p)| SeriesDir { key: normalize_title(&n), name: n, path: p })
                .filter(|s| s.key.chars().count() >= 2)
                .collect(),
            name,
            path,
        })
        .collect())
}

const STOPWORDS: &[&str] = &[
    "va", "cua", "cac", "nhung", "mot", "la", "cho", "voi", "trong", "tren", "duoi", "ve", "tu", "den", "khi", "nay",
    "do", "the", "and", "of", "a", "an", "to", "in", "for", "on", "by", "with", "from", "at", "is", "tap", "vol",
    "volume", "book", "edition", "ebook", "phan", "quyen", "so", "ban", "full",
];

fn term_tokens(s: &str) -> impl Iterator<Item = String> + '_ {
    simple_key(s)
        .split(' ')
        .filter(|t| t.chars().count() >= 2 && !t.chars().all(|c| c.is_ascii_digit()) && !STOPWORDS.contains(t))
        .map(str::to_string)
        .collect::<Vec<_>>()
        .into_iter()
}

/// Trọng số theo BRD mục 5: chủ đề ×3, tiêu đề ×2, tên file ×1.
fn raw_terms(b: &Book) -> HashMap<String, f64> {
    let mut m: HashMap<String, f64> = HashMap::new();
    for s in b.subjects.split(';') {
        for t in term_tokens(s) {
            *m.entry(t).or_default() += 3.0;
        }
    }
    for t in term_tokens(&b.title) {
        *m.entry(t).or_default() += 2.0;
    }
    if b.title_from_meta {
        for t in term_tokens(b.stem()) {
            *m.entry(t).or_default() += 1.0;
        }
    }
    m
}

fn unit(mut v: HashMap<String, f64>) -> HashMap<String, f64> {
    let n = v.values().map(|x| x * x).sum::<f64>().sqrt();
    if n > 0.0 {
        for x in v.values_mut() {
            *x /= n;
        }
    }
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Bucket {
    /// Tin cậy cao – đề xuất sẵn tick.
    High,
    /// Trung bình – đề xuất nhưng không tick.
    Medium,
    /// Thấp / không xác định – "Cần phân loại tay".
    Manual,
}

#[derive(Debug, Clone, Serialize)]
pub struct Prediction {
    pub category: Option<usize>,
    pub series_dir: Option<PathBuf>,
    pub confidence: u8,
    pub reason: String,
}

pub struct Model {
    pub categories: Vec<Category>,
    idf: HashMap<String, f64>,
    sums: Vec<HashMap<String, f64>>,
    sum_norm2: Vec<f64>,
    authors: Vec<HashMap<String, u32>>,
    pub training_docs: usize,
}

impl Model {
    /// Dựng hồ sơ từ sách đã nằm trong các thư mục thể loại (kể cả sách bạn đã phân loại tay – E6-08).
    pub fn build(conn: &rusqlite::Connection, cfg: &Config) -> Result<Model> {
        let categories = list_categories(cfg)?;
        let by_key: HashMap<&str, usize> = categories.iter().enumerate().map(|(i, c)| (c.key.as_str(), i)).collect();
        let lib = crate::dedupe::library_root_id(conn)?;
        let books = db::books_where(conn, "status = 'present' AND category IS NOT NULL AND in_inbox = 0", [])?;
        let docs: Vec<(usize, &Book, HashMap<String, f64>)> = books
            .iter()
            .filter(|b| Some(b.root_id) == lib)
            .filter_map(|b| {
                let c = *by_key.get(fold(b.category.as_deref()?).as_str())?;
                Some((c, b, raw_terms(b)))
            })
            .collect();
        // Thư mục cũng là một "tài liệu": từ khoá trong tên thư mục (vd. "Tiểu sử - Hồi ký").
        let name_docs: Vec<HashMap<String, f64>> =
            categories.iter().map(|c| term_tokens(&c.name).map(|t| (t, 3.0)).collect()).collect();
        let n_docs = (docs.len() + name_docs.len()) as f64;
        let mut df: HashMap<String, f64> = HashMap::new();
        for v in docs.iter().map(|d| &d.2).chain(name_docs.iter()) {
            for t in v.keys() {
                *df.entry(t.clone()).or_default() += 1.0;
            }
        }
        let idf: HashMap<String, f64> =
            df.into_iter().map(|(t, d)| (t, ((n_docs + 1.0) / (d + 1.0)).ln() + 1.0)).collect();
        let mut model = Model {
            sums: vec![HashMap::new(); categories.len()],
            sum_norm2: vec![0.0; categories.len()],
            authors: vec![HashMap::new(); categories.len()],
            training_docs: docs.len(),
            categories,
            idf,
        };
        for (c, v) in name_docs.into_iter().enumerate() {
            let u = model.weigh(v);
            add(&mut model.sums[c], &u, 1.0);
        }
        for (c, b, v) in &docs {
            let u = model.weigh(v.clone());
            add(&mut model.sums[*c], &u, 1.0);
            if !b.norm_author.is_empty() {
                *model.authors[*c].entry(b.norm_author.clone()).or_default() += 1;
            }
        }
        for (i, s) in model.sums.iter().enumerate() {
            model.sum_norm2[i] = s.values().map(|x| x * x).sum();
        }
        Ok(model)
    }

    fn weigh(&self, raw: HashMap<String, f64>) -> HashMap<String, f64> {
        unit(
            raw.into_iter()
                .map(|(t, tf)| {
                    let idf = self.idf.get(&t).copied().unwrap_or(1.0);
                    (t, tf * idf)
                })
                .collect(),
        )
    }

    pub fn category_index(&self, name: &str) -> Option<usize> {
        let k = fold(name);
        self.categories.iter().position(|c| c.key == k)
    }

    /// Khớp tên bộ truyện với thư mục bộ đã có (E6-10). Không phân biệt dấu/hoa thường.
    pub fn match_series(&self, b: &Book) -> Option<(usize, &SeriesDir)> {
        let stem_key = normalize_title(b.stem());
        let mut names = vec![series_from_name(b.stem())];
        if let Some(s) = &b.series {
            names.push(normalize_title(s));
        }
        let mut best: Option<(usize, &SeriesDir)> = None;
        for (ci, c) in self.categories.iter().enumerate() {
            for s in &c.series_dirs {
                if s.key.chars().count() < 3 {
                    continue;
                }
                let prefix_ok =
                    stem_key.strip_prefix(&s.key).is_some_and(|rest| rest.is_empty() || rest.starts_with(' '));
                if (names.iter().any(|n| n == &s.key) || prefix_ok)
                    && best.is_none_or(|(_, cur)| s.key.len() > cur.key.len())
                {
                    best = Some((ci, s));
                }
            }
        }
        best
    }

    /// Dự đoán thư mục cho một cuốn. `exclude_own`: bỏ đóng góp của chính cuốn này (đánh giá leave-one-out).
    pub fn predict(&self, b: &Book, exclude_own: Option<usize>) -> Prediction {
        let none = |reason: &str| Prediction { category: None, series_dir: None, confidence: 0, reason: reason.into() };
        if self.categories.is_empty() {
            return none("Thư viện chưa có thư mục thể loại nào");
        }
        let v = self.weigh(raw_terms(b));

        // 1) Tác giả đã có sách ở một mục.
        let mut author_pick: Option<(usize, u32, f64)> = None;
        if !b.norm_author.is_empty() {
            let counts: Vec<u32> = (0..self.categories.len())
                .map(|c| {
                    let n = self.authors[c].get(&b.norm_author).copied().unwrap_or(0);
                    if exclude_own == Some(c) {
                        n.saturating_sub(1)
                    } else {
                        n
                    }
                })
                .collect();
            let total: u32 = counts.iter().sum();
            if total > 0 {
                let (c, n) = counts.iter().copied().enumerate().max_by_key(|(_, n)| *n).unwrap();
                author_pick = Some((c, n, n as f64 / total as f64));
            }
        }

        // 2) Độ tương đồng văn bản với hồ sơ thư mục.
        let mut sims: Vec<(usize, f64)> = (0..self.categories.len())
            .map(|c| {
                let s = &self.sums[c];
                let dot: f64 = v.iter().map(|(t, w)| w * s.get(t).copied().unwrap_or(0.0)).sum();
                let (dot, n2) = if exclude_own == Some(c) {
                    let vv: f64 = v.values().map(|x| x * x).sum();
                    (dot - vv, self.sum_norm2[c] - 2.0 * dot + vv)
                } else {
                    (dot, self.sum_norm2[c])
                };
                let sim = if n2 > 1e-12 { (dot / n2.sqrt()).max(0.0) } else { 0.0 };
                (c, sim)
            })
            .collect();
        sims.sort_by(|a, b| b.1.total_cmp(&a.1));
        let (top, s1) = sims[0];
        let s2 = sims.get(1).map_or(0.0, |x| x.1);

        if let Some((c, n, share)) = author_pick {
            if n >= 1 && share >= 0.75 {
                let mut conf = (70.0 + 8.0 * n as f64).min(95.0) * share;
                if c == top && s1 > 0.05 {
                    conf = (conf + 5.0).min(97.0);
                }
                return Prediction {
                    category: Some(c),
                    series_dir: None,
                    confidence: conf.round() as u8,
                    reason: format!("{n} sách cùng tác giả đang ở mục này"),
                };
            }
        }
        if s1 < 0.05 {
            return none("Không đủ thông tin để đoán thể loại");
        }
        let margin = (s1 - s2) / s1;
        let strength = (s1 / 0.35).min(1.0);
        let conf = (100.0 * (0.55 * strength + 0.45 * margin)).min(90.0);
        let s = &self.sums[top];
        let mut contrib: Vec<(&String, f64)> =
            v.iter().map(|(t, w)| (t, w * s.get(t).copied().unwrap_or(0.0))).filter(|x| x.1 > 0.0).collect();
        contrib.sort_by(|a, b| b.1.total_cmp(&a.1));
        let words: Vec<&str> = contrib.iter().take(3).map(|(t, _)| t.as_str()).collect();
        Prediction {
            category: Some(top),
            series_dir: None,
            confidence: conf.round() as u8,
            reason: format!("Từ khoá khớp: {}", words.join(", ")),
        }
    }
}

fn add(sum: &mut HashMap<String, f64>, v: &HashMap<String, f64>, k: f64) {
    for (t, w) in v {
        *sum.entry(t.clone()).or_default() += w * k;
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub book: Book,
    /// Thư mục đích (luôn là thư mục đã tồn tại).
    pub target: Option<PathBuf>,
    pub target_label: Option<String>,
    pub confidence: u8,
    pub bucket: Bucket,
    pub preselected: bool,
    pub reason: String,
    /// Gợi ý cho mục "Cần phân loại tay" (vd. tên bộ mới cần tạo thư mục).
    pub suggestion: Option<String>,
}

fn is_comic_archive(b: &Book) -> bool {
    b.format == "cbz" || b.format == "cbr"
}

/// Đề xuất phân loại cho sách trong hộp thư đến (E6-04, E6-05 dùng kết quả này để xem trước).
pub fn propose(conn: &rusqlite::Connection, cfg: &Config) -> Result<Vec<Proposal>> {
    let model = Model::build(conn, cfg)?;
    let lib = crate::dedupe::library_root_id(conn)?;
    let inbox = db::books_where(conn, "status = 'present' AND in_inbox = 1 ORDER BY file_name", [])?;
    // Bản thừa = có anh em cùng băm đầy đủ nằm ngoài hộp thư đến.
    let dup_hashes: std::collections::HashSet<String> = {
        let mut st = conn.prepare(
            "SELECT DISTINCT full_hash FROM books WHERE status = 'present' AND in_inbox = 0 AND full_hash IS NOT NULL
             AND hash_size = size AND hash_mtime = mtime",
        )?;
        let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        v
    };
    let comic_cat =
        model.categories.iter().find(|c| ["truyen tranh", "manga", "comic"].iter().any(|k| c.key.contains(k)));
    let mut out = Vec::new();
    for b in inbox.into_iter().filter(|b| Some(b.root_id) == lib) {
        crate::progress::pulse();
        let bucket_of = |conf: u8| {
            if conf >= cfg.classify_high {
                Bucket::High
            } else if conf >= cfg.classify_medium {
                Bucket::Medium
            } else {
                Bucket::Manual
            }
        };
        if b.full_hash.as_ref().is_some_and(|h| dup_hashes.contains(h)) {
            // Bản thừa của một nhóm trùng chắc chắn: dọn ở màn "Trùng", không di chuyển.
            out.push(Proposal {
                target: None,
                target_label: None,
                confidence: 0,
                bucket: Bucket::Manual,
                preselected: false,
                reason: "Trùng chắc chắn với sách đã có trong thư viện".into(),
                suggestion: Some("Dọn ở màn Trùng chắc chắn".into()),
                book: b,
            });
            continue;
        }
        if let Some((ci, s)) = model.match_series(&b) {
            out.push(Proposal {
                target: Some(s.path.clone()),
                target_label: Some(format!("{}\\{}", model.categories[ci].name, s.name)),
                confidence: 95,
                bucket: bucket_of(95),
                preselected: 95 >= cfg.classify_high,
                reason: format!("Khớp bộ truyện “{}”", s.name),
                suggestion: None,
                book: b,
            });
            continue;
        }
        if is_comic_archive(&b) {
            // Truyện chưa có thư mục bộ → không tự tạo thư mục, đưa vào "Cần phân loại tay" kèm gợi ý (E6-10).
            let series = b.series.clone().unwrap_or_else(|| series_from_name(b.stem()));
            let suggestion = Some(match comic_cat {
                Some(c) => format!("Bộ mới “{series}” – tạo thư mục trong “{}”", c.name),
                None => format!("Bộ mới “{series}”"),
            });
            out.push(Proposal {
                target: None,
                target_label: None,
                confidence: 0,
                bucket: Bucket::Manual,
                preselected: false,
                reason: "Truyện chưa có thư mục bộ".into(),
                suggestion,
                book: b,
            });
            continue;
        }
        let p = model.predict(&b, None);
        let bucket = if p.category.is_some() { bucket_of(p.confidence) } else { Bucket::Manual };
        let (target, label) = match (bucket, p.category) {
            (Bucket::Manual, Some(c)) => (None, Some(model.categories[c].name.clone())),
            (_, Some(c)) => (Some(model.categories[c].path.clone()), Some(model.categories[c].name.clone())),
            _ => (None, None),
        };
        out.push(Proposal {
            preselected: bucket == Bucket::High,
            suggestion: if bucket == Bucket::Manual {
                label.clone().map(|l| format!("Có thể thuộc “{l}”"))
            } else {
                None
            },
            target_label: if bucket == Bucket::Manual { None } else { label },
            target,
            confidence: p.confidence,
            bucket,
            reason: p.reason,
            book: b,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Evaluation {
    pub total: usize,
    pub correct: usize,
    pub high_total: usize,
    pub high_correct: usize,
    pub medium_total: usize,
    pub medium_correct: usize,
    pub manual: usize,
}

impl Evaluation {
    pub fn accuracy(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.correct as f64 / self.total as f64
        }
    }
}

/// Đo độ chính xác trên chính kho: giấu nhãn thư mục từng cuốn (leave-one-out) – E6-04.
pub fn evaluate(conn: &rusqlite::Connection, cfg: &Config) -> Result<Evaluation> {
    let model = Model::build(conn, cfg)?;
    let lib = crate::dedupe::library_root_id(conn)?;
    let books = db::books_where(conn, "status = 'present' AND category IS NOT NULL AND in_inbox = 0", [])?;
    let mut ev = Evaluation::default();
    for b in books.iter().filter(|b| Some(b.root_id) == lib) {
        let Some(own) = b.category.as_deref().and_then(|c| model.category_index(c)) else { continue };
        let p = model.predict(b, Some(own));
        ev.total += 1;
        let ok = p.category == Some(own);
        match p.category {
            Some(_) if p.confidence >= cfg.classify_high => {
                ev.high_total += 1;
                ev.high_correct += ok as usize;
            }
            Some(_) if p.confidence >= cfg.classify_medium => {
                ev.medium_total += 1;
                ev.medium_correct += ok as usize;
            }
            _ => ev.manual += 1,
        }
        ev.correct += ok as usize;
    }
    Ok(ev)
}
