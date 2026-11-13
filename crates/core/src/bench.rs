//! Đo hiệu năng lõi (E10-03) trên một kho **tổng hợp** sinh trong thư mục tạm – không đụng kho thật.
//! `librarian bench --books 50000` in bảng số liệu và so với ngưỡng chấp nhận ở `docs/PERF.md`.

use crate::browse::{self, Query};
use crate::error::Result;
use crate::scan::{self, ScanOptions};
use crate::testdata::{self, EpubSpec};
use crate::{classify, dedupe, hash, search, Library};
use serde::Serialize;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct Metric {
    pub name: String,
    pub ms: f64,
    /// Ngưỡng chấp nhận (ms), nếu có.
    pub limit_ms: Option<f64>,
    pub note: String,
}

impl Metric {
    pub fn ok(&self) -> bool {
        self.limit_ms.is_none_or(|l| self.ms <= l)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchReport {
    pub books: usize,
    pub bytes: u64,
    pub db_bytes: u64,
    pub metrics: Vec<Metric>,
}

const WORDS: &[&str] = &[
    "mắt", "biếc", "hoa", "vàng", "cỏ", "xanh", "tuổi", "thơ", "dế", "mèn", "phiêu", "lưu", "ký", "số", "đỏ", "rừng",
    "na", "uy", "nhà", "giả", "kim", "trăm", "năm", "cô", "đơn", "bố", "già", "chiến", "tranh", "lịch", "sử", "việt",
    "nam", "kinh", "tế", "học", "lập", "trình", "rust", "dữ", "liệu", "tâm", "lý", "triết", "khoa", "vũ", "trụ",
];
const AUTHORS: &[&str] = &[
    "Nguyễn Nhật Ánh",
    "Tô Hoài",
    "Vũ Trọng Phụng",
    "Nam Cao",
    "Haruki Murakami",
    "Paulo Coelho",
    "Yuval Noah Harari",
    "Walter Isaacson",
    "Robert C. Martin",
    "Martin Fowler",
    "Nguyễn Hiến Lê",
    "Bảo Ninh",
];
const CATEGORIES: &[&str] = &["Văn học", "Tiểu sử - Hồi ký", "Công nghệ", "Kinh tế", "Lịch sử", "Khoa học"];

/// Bộ sinh số giả ngẫu nhiên đơn giản (tái lập được).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[(self.next() % v.len() as u64) as usize]
    }
}

/// Sinh `n` file sách nhỏ trong `books`: ~55% EPUB, 20% PDF, 20% CBZ theo bộ, 5% bản sao (trùng chắc chắn).
pub fn generate(books: &Path, n: usize) -> std::io::Result<u64> {
    let mut rng = Rng(0x47_4c49_4252);
    let mut made: Vec<std::path::PathBuf> = Vec::new();
    for i in 0..n {
        let kind = rng.next() % 100;
        if kind < 5 && !made.is_empty() {
            let src = made[(rng.next() % made.len() as u64) as usize].clone();
            let dst = books
                .join("Chưa phân loại")
                .join(format!("bản sao {i} {}", src.file_name().unwrap().to_string_lossy()));
            std::fs::create_dir_all(dst.parent().unwrap())?;
            std::fs::copy(&src, &dst)?;
            continue;
        }
        let title: Vec<&str> = (0..2 + rng.next() % 4).map(|_| rng.pick(WORDS)).collect();
        let title = format!("{} {i}", title.join(" "));
        let author = rng.pick(AUTHORS);
        let cat = rng.pick(CATEGORIES);
        let path = if kind < 60 {
            let p = books.join(cat).join(format!("{title}.epub"));
            testdata::write_epub(
                &p,
                &EpubSpec {
                    title: &title,
                    authors: &[author],
                    subjects: &[cat],
                    isbn: None,
                    language: "vi",
                    body: &title,
                },
            )?;
            p
        } else if kind < 80 {
            let p = books.join(cat).join(format!("{title}.pdf"));
            testdata::write_pdf(&p, Some(&title), Some(author), &format!("{i}"))?;
            p
        } else {
            let series = format!("Bộ {}", rng.next() % (n as u64 / 40 + 1));
            let vol = i % 97 + 1;
            let p = books.join("truyen tranh").join(&series).join(format!("{series} {vol}.cbz"));
            testdata::write_cbz(&p, Some(&series), Some(vol as u32), 1, &format!("c{i}"))?;
            p
        };
        made.push(path);
    }
    Ok(walkdir::WalkDir::new(books).into_iter().flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum())
}

fn time<T>(f: impl FnOnce() -> Result<T>) -> Result<(T, Duration)> {
    let t = Instant::now();
    let v = f()?;
    Ok((v, t.elapsed()))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Chạy đo trong `work` (tạo `books/` và `home/` bên trong). `progress` nhận tên bước đang chạy.
pub fn run(work: &Path, n: usize, threads: usize, progress: &dyn Fn(&str)) -> Result<BenchReport> {
    let books = work.join("books");
    let home = work.join("home");
    progress("sinh kho tổng hợp");
    let bytes = generate(&books, n)?;
    let mut lib = Library::open(&home)?;
    lib.set_library_root(&books)?;
    let mut m = Vec::new();
    // Ngưỡng ở docs/PERF.md, quy về kho n sách (tuyến tính theo 50k).
    let scale = n as f64 / 50_000.0;
    let mut push = |name: &str, d: Duration, limit: Option<f64>, note: String| {
        m.push(Metric { name: name.into(), ms: ms(d), limit_ms: limit, note });
    };

    progress("quét lần đầu");
    let (s1, d) = time(|| scan::scan(&lib.conn, &lib.config, &ScanOptions::default()))?;
    push(
        "Quét lần đầu",
        d,
        None,
        format!("{} file, {:.0} file/giây", s1.files_seen, s1.files_seen as f64 / d.as_secs_f64()),
    );
    progress("quét lại (không đổi gì)");
    let (s2, d) = time(|| scan::scan(&lib.conn, &lib.config, &ScanOptions::default()))?;
    push("Quét lại (tăng dần)", d, Some(120_000.0 * scale.max(0.02)), format!("{} không đổi", s2.unchanged));
    progress("băm");
    let (hs, d) = time(|| hash::ensure_hashes(&lib.conn, threads, None))?;
    push(
        "Băm (lọc dung lượng → đầu/giữa/cuối → đầy đủ)",
        d,
        None,
        format!("{} ứng viên, băm đầy đủ {} file", hs.size_candidates, hs.full_hashed),
    );
    progress("tìm trùng");
    let (ex, d) = time(|| dedupe::exact_groups(&lib.conn))?;
    push("Trùng chắc chắn", d, Some(5_000.0 * scale.max(0.02)), format!("{} nhóm", ex.len()));
    let (su, d) = time(|| dedupe::suspect_groups(&lib.conn))?;
    push("Nghi trùng", d, Some(10_000.0 * scale.max(0.02)), format!("{} nhóm", su.len()));

    progress("tìm kiếm");
    let queries =
        ["mat bi", "nguyen nhat", "hoa vang co", "lap trinh rust", "bo 3", "tam ly", "su viet", "khoa hoc vu"];
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    for _ in 0..3 {
        for q in queries {
            let (_, d) = time(|| search::search(&lib.conn, q, 50))?;
            worst = worst.max(d);
            total += d;
        }
    }
    let avg = total / (queries.len() as u32 * 3);
    push("Tìm kiếm (trung bình)", avg, Some(200.0), format!("{} truy vấn × 3", queries.len()));
    push("Tìm kiếm (chậm nhất)", worst, Some(200.0), String::new());

    progress("lưới thư viện");
    let (p, d) = time(|| {
        browse::query(
            &lib.conn,
            &Query { group_series: true, sort: browse::Sort::Title, limit: 120, ..Default::default() },
        )
    })?;
    push("Lưới thư viện (gộp bộ, sắp theo tên)", d, Some(500.0), format!("{} ô", p.total));
    let (p, d) =
        time(|| browse::query(&lib.conn, &Query { q: Some("hoa".into()), group_series: true, ..Default::default() }))?;
    push("Lưới thư viện khi gõ tìm", d, Some(200.0), format!("{} ô", p.total));

    progress("đề xuất phân loại");
    let (pr, d) = time(|| classify::propose(&lib.conn, &lib.config))?;
    push("Đề xuất phân loại hộp thư đến", d, Some(15_000.0 * scale.max(0.02)), format!("{} sách", pr.len()));

    let db_bytes = std::fs::metadata(home.join("library.db")).map(|m| m.len()).unwrap_or(0);
    Ok(BenchReport { books: s1.files_seen, bytes, db_bytes, metrics: m })
}

#[cfg(test)]
mod tests {
    #[test]
    fn small_bench_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let r = super::run(tmp.path(), 300, 2, &|_| {}).unwrap();
        assert!(r.books >= 280, "{}", r.books);
        assert!(r.metrics.iter().any(|m| m.name.starts_with("Trùng") && m.note != "0 nhóm"));
        assert!(r.metrics.len() >= 10);
    }
}
