//! Hỗ trợ trình đọc trong app (M5): chọn bộ máy hiển thị theo định dạng, nhớ vị trí đọc (E7-06),
//! trang truyện tranh CBZ/CBR (E7-08, E7-09) và vol trước/sau của cùng bộ (E7-15).
//!
//! Truyện tranh không bao giờ được nạp cả file vào webview: app phục vụ từng trang một.
//! CBZ (và CBR thực chất là zip) đọc thẳng từng mục trong zip. CBR dạng RAR được giải nén dần ở luồng
//! nền vào thư mục cache trong `%APPDATA%` (trang nào xong đọc được ngay; RAR không "solid" thì đọc thẳng),
//! giữ vài cuốn gần nhất. Không bao giờ ghi gì vào ổ sách.

use crate::browse::{self, Item, Query};
use crate::db::{self, Book};
use crate::error::{Error, Result};
use crate::formats::{self, comic::is_image_name, Format};
use crate::normalize::natural_cmp;
use rusqlite::{params, Connection};
use serde::Serialize;
#[cfg(feature = "desktop")]
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
#[cfg(feature = "desktop")]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(feature = "desktop")]
use std::time::{Duration, Instant};

/// Ảnh trang lớn nhất chấp nhận (một trang truyện hiếm khi quá vài MB).
const MAX_PAGE: u64 = 64 * 1024 * 1024;
/// Số cuốn RAR đã giải nén giữ trong cache.
#[cfg(feature = "desktop")]
const RAR_CACHE_KEEP: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// foliate-js: EPUB, MOBI/AZW/AZW3, FB2.
    Foliate,
    /// Trình xem ảnh theo trang: CBZ, CBR.
    Comic,
    /// pdf.js, tải theo HTTP Range.
    Pdf,
    /// Văn bản thuần.
    Text,
    /// Chưa đọc được trong app (DJVU) → mở bằng ứng dụng mặc định.
    External,
}

impl Engine {
    pub fn for_format(f: Format) -> Engine {
        match f {
            Format::Epub | Format::Mobi | Format::Azw | Format::Azw3 | Format::Fb2 => Engine::Foliate,
            Format::Cbz | Format::Cbr => Engine::Comic,
            Format::Pdf => Engine::Pdf,
            Format::Txt => Engine::Text,
            Format::Djvu => Engine::External,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VolumeRef {
    pub id: i64,
    pub title: String,
    pub volume: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReaderInfo {
    pub book: Book,
    pub engine: Engine,
    /// Vị trí đã lưu: CFI (foliate) hoặc `page:<n>` (truyện tranh/PDF, n tính từ 0).
    pub locator: Option<String>,
    pub progress: Option<f64>,
    pub prev: Option<VolumeRef>,
    pub next: Option<VolumeRef>,
    /// (vị trí 1-based, tổng số vol) khi sách thuộc một bộ.
    pub series_pos: Option<(usize, usize)>,
}

pub fn reader_info(conn: &Connection, id: i64) -> Result<ReaderInfo> {
    let book = db::get_book(conn, id)?.ok_or_else(|| Error::msg("Không có sách này"))?;
    if book.status != "present" {
        return Err(Error::msg("File sách không còn ở chỗ cũ (đã bị xoá hoặc ổ chưa cắm)"));
    }
    let engine = Format::parse(&book.format).map(Engine::for_format).unwrap_or(Engine::External);
    let (locator, progress): (Option<String>, Option<f64>) =
        conn.query_row("SELECT locator, progress FROM books WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let (mut prev, mut next, mut series_pos) = (None, None, None);
    if let Some(series) = &book.series {
        let vols = series_volumes(conn, series, book.category.as_deref())?;
        if let Some(i) = vols.iter().position(|v| v.id == id) {
            series_pos = Some((i + 1, vols.len()));
            prev = i.checked_sub(1).map(|j| vols[j].clone());
            next = vols.get(i + 1).cloned();
        }
    }
    Ok(ReaderInfo { book, engine, locator, progress, prev, next, series_pos })
}

/// Sách mở từ Explorer nhưng không nằm trong thư viện: đọc được, không ghi gì vào DB
/// (`id` âm do app cấp, vị trí đọc không được lưu).
pub fn external_book(id: i64, path: &Path) -> Result<Book> {
    let meta = std::fs::metadata(path).map_err(|e| Error::msg(format!("Không mở được file: {e}")))?;
    let format = Format::from_path(path).ok_or_else(|| Error::msg("Định dạng không hỗ trợ"))?;
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let md = formats::extract(path, format).unwrap_or_default();
    let stem = file_name.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or_else(|| file_name.clone());
    let title = md.title.clone().unwrap_or(stem);
    Ok(Book {
        id,
        root_id: 0,
        path: path.to_string_lossy().into_owned(),
        rel_path: file_name.clone(),
        file_name,
        format: format.as_str().to_string(),
        size: meta.len() as i64,
        mtime: meta.modified().map(db::mtime_ms).unwrap_or(0),
        norm_title: crate::normalize::normalize_title(&title),
        title_from_meta: md.title.is_some(),
        title,
        authors: md.authors.join(", "),
        norm_author: String::new(),
        language: md.language,
        subjects: md.subjects.join(", "),
        isbn: md.isbn,
        series: None,
        volume: None,
        page_count: md.page_count.map(i64::from),
        category: None,
        in_inbox: false,
        quick_hash: None,
        full_hash: None,
        hash_size: None,
        hash_mtime: None,
        status: "present".into(),
        error: None,
        read_status: "reading".into(),
        progress: None,
        drm: md.drm,
        not_worth: false,
    })
}

pub fn external_info(id: i64, path: &Path) -> Result<ReaderInfo> {
    let book = external_book(id, path)?;
    let engine = Format::parse(&book.format).map(Engine::for_format).unwrap_or(Engine::External);
    Ok(ReaderInfo { book, engine, locator: None, progress: None, prev: None, next: None, series_pos: None })
}

/// Tìm sách trong thư viện theo đường dẫn (không phân biệt hoa thường, `/` hay `\`).
pub fn find_by_path(conn: &Connection, path: &Path) -> Result<Option<i64>> {
    let want = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let mut st =
        conn.prepare("SELECT id, path FROM books WHERE status = 'present' AND file_name = ?1 COLLATE NOCASE")?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let rows = st.query_map([name], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (id, p) = row?;
        if p.replace('/', "\\").to_lowercase() == want {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// Các vol của một bộ theo đúng thứ tự màn chi tiết bộ (E3-07).
fn series_volumes(conn: &Connection, series: &str, category: Option<&str>) -> Result<Vec<VolumeRef>> {
    let page = browse::query(
        conn,
        &Query {
            series: Some(series.to_string()),
            category: Some(category.unwrap_or("").to_string()),
            limit: 100_000,
            ..Default::default()
        },
    )?;
    Ok(page
        .items
        .into_iter()
        .filter_map(|it| match it {
            Item::Book(b) => Some(VolumeRef { id: b.id, title: b.title, volume: b.volume }),
            Item::Series(_) => None,
        })
        .collect())
}

/// Lưu vị trí đọc. Trả trạng thái đọc mới: đọc tới ≥ 99,5% thì thành "đã đọc"; sách đã đọc
/// được đọc lại vẫn giữ "đã đọc" (giống Apple Books).
pub fn save_position(conn: &Connection, id: i64, locator: Option<&str>, progress: f64) -> Result<String> {
    let progress = if progress.is_finite() { progress.clamp(0.0, 1.0) } else { 0.0 };
    let current: String = conn
        .query_row("SELECT read_status FROM books WHERE id = ?1", [id], |r| r.get(0))
        .map_err(|_| Error::msg("Không có sách này"))?;
    let status = if progress >= 0.995 || current == "read" { "read" } else { "reading" };
    conn.execute(
        "UPDATE books SET locator = ?2, progress = ?3, read_status = ?4, last_read_at = ?5 WHERE id = ?1",
        params![id, locator, if progress >= 0.995 { 1.0 } else { progress }, status, db::now()],
    )?;
    Ok(status.to_string())
}

// ------------------------------------------------------------------ truyện tranh

pub fn image_mime(name: &str) -> &'static str {
    match name.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        Some("avif") => "image/avif",
        Some("jxl") => "image/jxl",
        _ => "application/octet-stream",
    }
}

fn sorted_images<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut v: Vec<String> = names.filter(|n| is_image_name(n) && !n.ends_with('/')).map(str::to_string).collect();
    v.sort_by(|a, b| natural_cmp(a, b));
    v
}

fn is_zip(path: &Path) -> Result<bool> {
    let mut m = [0u8; 4];
    File::open(path)?.read_exact(&mut m)?;
    Ok(&m == b"PK\x03\x04")
}

#[cfg(feature = "desktop")]
fn is_rar(path: &Path) -> Result<bool> {
    let mut m = [0u8; 4];
    File::open(path)?.read_exact(&mut m)?;
    Ok(&m == b"Rar!")
}

type ZipReader = zip::ZipArchive<BufReader<File>>;

/// Nguồn trang của một cuốn truyện đã mở.
pub enum ComicSource {
    /// Đọc trực tiếp từ zip; `pages` là tên mục trong zip. `pool` giữ sẵn các bản zip đã đọc
    /// mục lục (central directory) để lật trang không phải phân tích lại file vài GB mỗi lần.
    Zip { path: PathBuf, pages: Vec<String>, pool: std::sync::Mutex<Vec<ZipReader>> },
    /// RAR đã giải nén xong vào thư mục cache; `pages` là tên file trong `dir`.
    Dir { dir: PathBuf, pages: Vec<String> },
    /// RAR đang được giải nén dần ở luồng nền; trang nào xong trước đọc được trước.
    #[cfg(feature = "desktop")]
    Rar(Arc<RarJob>),
}

impl std::fmt::Debug for ComicSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComicSource::Zip { path, pages, .. } => write!(f, "Zip({}, {} trang)", path.display(), pages.len()),
            ComicSource::Dir { dir, pages } => write!(f, "Dir({}, {} trang)", dir.display(), pages.len()),
            #[cfg(feature = "desktop")]
            ComicSource::Rar(j) => write!(f, "Rar({}, {} trang)", j.path.display(), j.orig.len()),
        }
    }
}

impl ComicSource {
    pub fn page_count(&self) -> usize {
        match self {
            ComicSource::Zip { pages, .. } | ComicSource::Dir { pages, .. } => pages.len(),
            #[cfg(feature = "desktop")]
            ComicSource::Rar(j) => j.orig.len(),
        }
    }

    /// Đọc trang `i` (0-based): (bytes, MIME).
    pub fn page(&self, i: usize) -> Result<(Vec<u8>, &'static str)> {
        match self {
            ComicSource::Zip { path, pages, pool } => {
                let name = pages.get(i).ok_or_else(|| Error::msg("Không có trang này"))?;
                let taken = pool.lock().ok().and_then(|mut v| v.pop());
                let mut z = match taken {
                    Some(z) => z,
                    None => open_zip(path)?,
                };
                let bytes = formats::read_zip_entry(&mut z, name, MAX_PAGE).map_err(Error::msg)?;
                if let Ok(mut v) = pool.lock() {
                    if v.len() < 4 {
                        v.push(z);
                    }
                }
                Ok((bytes, image_mime(name)))
            }
            ComicSource::Dir { dir, pages } => {
                let name = pages.get(i).ok_or_else(|| Error::msg("Không có trang này"))?;
                Ok((std::fs::read(dir.join(name))?, image_mime(name)))
            }
            #[cfg(feature = "desktop")]
            ComicSource::Rar(j) => j.page(i),
        }
    }
}

fn open_zip(path: &Path) -> Result<ZipReader> {
    zip::ZipArchive::new(BufReader::with_capacity(256 * 1024, File::open(path)?))
        .map_err(|e| Error::msg(format!("zip hỏng: {e}")))
}

/// Mở một cuốn truyện tranh. Không đọc hết file: zip chỉ đọc mục lục; RAR chỉ liệt kê tên rồi
/// giải nén dần vào `cache_root` ở luồng nền (lần sau mở lại dùng luôn bản đã giải nén).
pub fn open_comic(path: &Path, size: i64, mtime: i64, cache_root: &Path) -> Result<ComicSource> {
    if is_zip(path)? {
        let z = open_zip(path)?;
        let pages = sorted_images(z.file_names());
        if pages.is_empty() {
            return Err(Error::msg("Không có ảnh trang nào trong file"));
        }
        return Ok(ComicSource::Zip { path: path.to_path_buf(), pages, pool: std::sync::Mutex::new(vec![z]) });
    }
    #[cfg(feature = "desktop")]
    if is_rar(path)? {
        return open_rar(path, size, mtime, cache_root);
    }
    #[cfg(not(feature = "desktop"))]
    let _ = (size, mtime, cache_root);
    Err(Error::msg("Không phải CBZ hợp lệ (CBR/RAR chưa hỗ trợ trên bản này)"))
}

/// Khoá cache theo đường dẫn + dung lượng + mtime: file đổi thì giải nén lại.
#[cfg(feature = "desktop")]
fn cache_key(path: &Path, size: i64, mtime: i64) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(path.to_string_lossy().as_bytes());
    h.update(size.to_le_bytes());
    h.update(mtime.to_le_bytes());
    h.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect()
}

#[cfg(feature = "desktop")]
const MANIFEST: &str = "pages.json";

/// Các cuốn RAR đang giải nén (theo thư mục cache) – mở lại cuốn đang giải nén thì dùng chung.
#[cfg(feature = "desktop")]
static RAR_JOBS: std::sync::LazyLock<Mutex<HashMap<PathBuf, Arc<RarJob>>>> = std::sync::LazyLock::new(Default::default);

/// Giải nén RAR ở luồng nền, ghi từng trang `00001.ext`… vào `dir` theo thứ tự tự nhiên của tên gốc.
#[cfg(feature = "desktop")]
pub struct RarJob {
    path: PathBuf,
    dir: PathBuf,
    /// Tên gốc trong RAR, đã sắp theo thứ tự trang.
    orig: Vec<String>,
    /// Tên file trong `dir` tương ứng.
    names: Vec<String>,
    solid: bool,
    state: Mutex<RarState>,
    cv: Condvar,
}

#[cfg(feature = "desktop")]
#[derive(Default)]
struct RarState {
    done: Vec<bool>,
    finished: bool,
    error: Option<String>,
}

/// Đợi tối đa chừng này cho một trang của RAR "solid" (phải giải nén tuần tự tới trang đó).
#[cfg(feature = "desktop")]
const RAR_WAIT: Duration = Duration::from_secs(300);

#[cfg(feature = "desktop")]
impl RarJob {
    fn page(&self, i: usize) -> Result<(Vec<u8>, &'static str)> {
        let name = self.names.get(i).ok_or_else(|| Error::msg("Không có trang này"))?;
        let mime = image_mime(name);
        let ready = |s: &RarState| s.done.get(i).copied().unwrap_or(false);
        {
            let s = self.state.lock().map_err(|_| Error::msg("khoá hỏng"))?;
            if ready(&s) {
                drop(s);
                return Ok((std::fs::read(self.dir.join(name))?, mime));
            }
        }
        // RAR thường (không solid): nhảy thẳng tới mục cần đọc, không phải đợi luồng nền.
        if !self.solid {
            return Ok((rar_read_entry(&self.path, &self.orig[i], MAX_PAGE)?, mime));
        }
        let deadline = Instant::now() + RAR_WAIT;
        let mut s = self.state.lock().map_err(|_| Error::msg("khoá hỏng"))?;
        loop {
            if ready(&s) {
                drop(s);
                return Ok((std::fs::read(self.dir.join(name))?, mime));
            }
            if let Some(e) = &s.error {
                return Err(Error::msg(e.clone()));
            }
            if s.finished {
                return Err(Error::msg("Không đọc được trang này trong file RAR"));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::msg("Giải nén RAR quá lâu"));
            }
            s = self.cv.wait_timeout(s, left).map_err(|_| Error::msg("khoá hỏng"))?.0;
        }
    }

    fn mark(&self, f: impl FnOnce(&mut RarState)) {
        if let Ok(mut s) = self.state.lock() {
            f(&mut s);
        }
        self.cv.notify_all();
    }

    /// Chạy ở luồng nền: giải nén các ảnh còn thiếu, xong thì ghi manifest.
    fn run(self: Arc<Self>, cache_root: PathBuf) {
        let res = self.extract();
        match res {
            Ok(()) => {
                let all = self.state.lock().map(|s| s.done.iter().all(|d| *d)).unwrap_or(false);
                if all {
                    if let Ok(json) = serde_json::to_string(&self.names) {
                        let _ = std::fs::write(self.dir.join(MANIFEST), json);
                    }
                }
                self.mark(|s| s.finished = true);
            }
            Err(e) => {
                tracing::warn!("giải nén RAR {}: {e}", self.path.display());
                self.mark(|s| {
                    s.error = Some(e.to_string());
                    s.finished = true;
                });
            }
        }
        if let Ok(mut jobs) = RAR_JOBS.lock() {
            jobs.remove(&self.dir);
        }
        prune_cache(&cache_root, RAR_CACHE_KEEP);
    }

    fn extract(&self) -> Result<()> {
        let rar_err = |e: unrar::error::UnrarError| Error::msg(format!("RAR hỏng hoặc có mật khẩu: {e}"));
        let index: HashMap<&str, usize> = self.orig.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
        let mut archive = unrar::Archive::new(&self.path).open_for_processing().map_err(rar_err)?;
        while let Some(header) = archive.read_header().map_err(rar_err)? {
            let entry = header.entry();
            let name = entry.filename.to_string_lossy().replace('\\', "/");
            let want = index.get(name.as_str()).copied().filter(|&i| {
                entry.is_file()
                    && entry.unpacked_size <= MAX_PAGE
                    && !self.state.lock().map(|s| s.done[i]).unwrap_or(false)
            });
            archive = match want {
                Some(i) => {
                    let (data, rest) = header.read().map_err(rar_err)?;
                    // Ghi ra file tạm rồi đổi tên: file trang nào có mặt thì đã đầy đủ.
                    let tmp = self.dir.join(format!("{}.part", self.names[i]));
                    std::fs::write(&tmp, data)?;
                    std::fs::rename(&tmp, self.dir.join(&self.names[i]))?;
                    self.mark(|s| s.done[i] = true);
                    rest
                }
                None => header.skip().map_err(rar_err)?,
            };
        }
        Ok(())
    }
}

#[cfg(feature = "desktop")]
fn open_rar(path: &Path, size: i64, mtime: i64, cache_root: &Path) -> Result<ComicSource> {
    let dir = cache_root.join(cache_key(path, size, mtime));
    if let Ok(s) = std::fs::read_to_string(dir.join(MANIFEST)) {
        if let Ok(pages) = serde_json::from_str::<Vec<String>>(&s) {
            if !pages.is_empty() && pages.iter().all(|p| dir.join(p).is_file()) {
                touch(&dir);
                return Ok(ComicSource::Dir { dir, pages });
            }
        }
    }
    let mut jobs = RAR_JOBS.lock().map_err(|_| Error::msg("khoá hỏng"))?;
    if let Some(j) = jobs.get(&dir) {
        return Ok(ComicSource::Rar(j.clone()));
    }
    let solid = unrar::Archive::new(path)
        .open_for_listing()
        .map(|a| a.is_solid())
        .map_err(|e| Error::msg(format!("RAR hỏng hoặc có mật khẩu: {e}")))?;
    let orig = rar_image_names(path)?;
    if orig.is_empty() {
        return Err(Error::msg("Không có ảnh trang nào trong file RAR"));
    }
    let names: Vec<String> = orig
        .iter()
        .enumerate()
        .map(|(i, o)| format!("{:05}.{}", i + 1, o.rsplit('.').next().unwrap_or("img").to_ascii_lowercase()))
        .collect();
    std::fs::create_dir_all(&dir)?;
    let _ = std::fs::remove_file(dir.join(MANIFEST));
    // Lần trước giải nén dở: các trang đã có (ghi qua file tạm + đổi tên) dùng lại được.
    let done = names.iter().map(|n| dir.join(n).is_file()).collect();
    let job = Arc::new(RarJob {
        path: path.to_path_buf(),
        dir: dir.clone(),
        orig,
        names,
        solid,
        state: Mutex::new(RarState { done, ..Default::default() }),
        cv: Condvar::new(),
    });
    jobs.insert(dir, job.clone());
    drop(jobs);
    touch_dir(&job.dir);
    let (j, root) = (job.clone(), cache_root.to_path_buf());
    std::thread::Builder::new()
        .name("rar-extract".into())
        .spawn(move || j.run(root))
        .map_err(|e| Error::msg(format!("không tạo được luồng giải nén: {e}")))?;
    Ok(ComicSource::Rar(job))
}

impl ComicSource {
    /// RAR đang giải nén ở luồng nền: đợi tới khi xong (các nguồn khác trả về ngay).
    pub fn wait_extracted(&self) {
        #[cfg(feature = "desktop")]
        if let ComicSource::Rar(j) = self {
            let Ok(mut s) = j.state.lock() else { return };
            while !s.finished {
                match j.cv.wait(s) {
                    Ok(g) => s = g,
                    Err(_) => return,
                }
            }
        }
    }
}

#[cfg(feature = "desktop")]
fn touch(dir: &Path) {
    // Cập nhật mtime của manifest để biết cuốn nào vừa dùng (dọn cache theo LRU).
    if let Ok(f) = File::options().append(true).open(dir.join(MANIFEST)) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

#[cfg(feature = "desktop")]
fn touch_dir(dir: &Path) {
    if let Ok(f) = File::open(dir) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// Tổng dung lượng cache RAR tối đa (ngoài cuốn mới dùng nhất, luôn được giữ).
#[cfg(feature = "desktop")]
const RAR_CACHE_BYTES: u64 = 6 * 1024 * 1024 * 1024;

#[cfg(feature = "desktop")]
fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

/// Giữ `keep` thư mục dùng gần nhất (và tổng không quá `RAR_CACHE_BYTES`), xoá phần còn lại –
#[cfg(feature = "desktop")]
/// trừ cuốn đang giải nén.
pub fn prune_cache(cache_root: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(cache_root) else { return };
    let busy: Vec<PathBuf> = RAR_JOBS.lock().map(|j| j.keys().cloned().collect()).unwrap_or_default();
    let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().is_dir() && !busy.contains(&e.path()))
        .map(|e| {
            let p = e.path();
            let t = std::fs::metadata(p.join(MANIFEST))
                .or_else(|_| std::fs::metadata(&p))
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            (t, p)
        })
        .collect();
    dirs.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut total: u64 = busy.iter().map(|d| dir_size(d)).sum();
    for (i, (_, p)) in dirs.into_iter().enumerate() {
        let size = dir_size(&p);
        total += size;
        if i >= keep || (i > 0 && total > RAR_CACHE_BYTES) {
            let _ = std::fs::remove_dir_all(p);
            total -= size;
        }
    }
}

/// Danh sách ảnh trong RAR (không giải nén) – dùng đếm trang & tìm bìa.
#[cfg(feature = "desktop")]
pub fn rar_image_names(path: &Path) -> Result<Vec<String>> {
    let list = unrar::Archive::new(path)
        .open_for_listing()
        .map_err(|e| Error::msg(format!("RAR hỏng hoặc có mật khẩu: {e}")))?;
    let mut names = Vec::new();
    for h in list {
        let h = h.map_err(|e| Error::msg(format!("RAR hỏng: {e}")))?;
        if h.is_file() {
            names.push(h.filename.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(sorted_images(names.iter().map(String::as_str)))
}

/// Đọc một mục trong RAR theo tên (giải nén tuần tự tới mục đó).
#[cfg(feature = "desktop")]
pub fn rar_read_entry(path: &Path, wanted: &str, max: u64) -> Result<Vec<u8>> {
    let rar_err = |e: unrar::error::UnrarError| Error::msg(format!("RAR hỏng: {e}"));
    let mut archive = unrar::Archive::new(path).open_for_processing().map_err(rar_err)?;
    while let Some(header) = archive.read_header().map_err(rar_err)? {
        let entry = header.entry();
        if entry.is_file() && entry.filename.to_string_lossy().replace('\\', "/") == wanted {
            if entry.unpacked_size > max {
                return Err(Error::msg("mục quá lớn"));
            }
            return Ok(header.read().map_err(rar_err)?.0);
        }
        archive = header.skip().map_err(rar_err)?;
    }
    Err(Error::msg("không có mục này"))
}

/// Bản không có `unrar` (iPad): không đọc được RAR.
#[cfg(not(feature = "desktop"))]
pub fn rar_image_names(_path: &Path) -> Result<Vec<String>> {
    Err(Error::msg("Định dạng chưa hỗ trợ (CBR)"))
}

/// Bản không có `unrar` (iPad): không đọc được RAR.
#[cfg(not(feature = "desktop"))]
pub fn rar_read_entry(_path: &Path, _wanted: &str, _max: u64) -> Result<Vec<u8>> {
    Err(Error::msg("Định dạng chưa hỗ trợ (CBR)"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn engine_by_format() {
        assert_eq!(Engine::for_format(Format::Azw3), Engine::Foliate);
        assert_eq!(Engine::for_format(Format::Cbr), Engine::Comic);
        assert_eq!(Engine::for_format(Format::Pdf), Engine::Pdf);
        assert_eq!(Engine::for_format(Format::Djvu), Engine::External);
    }

    #[test]
    fn images_sorted_naturally() {
        let v = sorted_images(["b/Page 10.jpg", "b/Page 2.jpg", "ComicInfo.xml", "__MACOSX/x.jpg", "b/"].into_iter());
        assert_eq!(v, vec!["b/Page 2.jpg", "b/Page 10.jpg"]);
        assert_eq!(image_mime("A.JPG"), "image/jpeg");
    }

    #[test]
    fn position_and_status() {
        let conn = db::open_in_memory().unwrap();
        db::ensure_root(&conn, "C:/books", "library").unwrap();
        conn.execute(
            "INSERT INTO books (id, root_id, path, rel_path, file_name, format, size, mtime, title, norm_title, added_at, updated_at)
             VALUES (1, 1, 'C:/books/a.epub', 'a.epub', 'a.epub', 'epub', 1, 1, 'A', 'a', 0, 0)",
            [],
        )
        .unwrap();
        assert_eq!(save_position(&conn, 1, Some("epubcfi(/6/4!/4)"), 0.4).unwrap(), "reading");
        let info = reader_info(&conn, 1).unwrap();
        assert_eq!(info.engine, Engine::Foliate);
        assert_eq!(info.locator.as_deref(), Some("epubcfi(/6/4!/4)"));
        assert_eq!(info.progress, Some(0.4));
        assert_eq!(save_position(&conn, 1, Some("x"), 0.999).unwrap(), "read");
        // Đọc lại sách đã đọc: vẫn "đã đọc".
        assert_eq!(save_position(&conn, 1, Some("y"), 0.1).unwrap(), "read");
        assert_eq!(save_position(&conn, 1, None, f64::NAN).unwrap(), "read");
        assert!(save_position(&conn, 99, None, 0.5).is_err());
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn cbz_pages_reuse_zip_index() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.cbz");
        crate::testdata::write_cbz(&p, None, None, 12, "x").unwrap();
        let src = open_comic(&p, 0, 0, &d.path().join("cache")).unwrap();
        assert_eq!(src.page_count(), 12);
        for i in [0, 11, 5, 5] {
            let (b, mime) = src.page(i).unwrap();
            assert!(!b.is_empty());
            assert_eq!(mime, "image/png");
        }
        assert!(src.page(12).is_err());
    }

    /// Đo thời gian mở + trang đầu/cuối với file truyện lớn thật:
    /// `LIBRARIAN_BIG_COMIC="a.cbz;b.cbr" cargo test -p librarian-core big_comic -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn big_comic_timing() {
        let list = std::env::var("LIBRARIAN_BIG_COMIC").unwrap_or_default();
        let cache = tempfile::tempdir().unwrap();
        for p in list.split(';').filter(|s| !s.is_empty()) {
            let t = Instant::now();
            let src = open_comic(Path::new(p), 1, 1, cache.path()).unwrap();
            let open = t.elapsed();
            let first = src.page(0).unwrap().0.len();
            let t1 = t.elapsed();
            let last = src.page(src.page_count() - 1).unwrap().0.len();
            let t2 = t.elapsed();
            let mid = src.page(src.page_count() / 2).unwrap().0.len();
            let t3 = t.elapsed();
            eprintln!(
                "{p}: {} trang · mở {open:?} · trang đầu {t1:?} ({first} B) · trang cuối {t2:?} ({last} B) · giữa {t3:?} ({mid} B)",
                src.page_count()
            );
            src.wait_extracted();
        }
    }

    #[cfg(feature = "desktop")]
    /// CBR thật (RAR) – cần Rar.exe của WinRAR; máy không có thì bỏ qua.
    #[cfg(feature = "desktop")]
    #[test]
    fn rar_pages_available_before_full_extract() {
        let rar = Path::new(r"C:\Program Files\WinRAR\Rar.exe");
        if !rar.is_file() {
            eprintln!("bỏ qua: không có Rar.exe");
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let imgs = d.path().join("imgs");
        std::fs::create_dir_all(&imgs).unwrap();
        for i in 1..=30 {
            std::fs::write(imgs.join(format!("Page {i}.png")), crate::testdata::sample_png(&format!("p{i}"), 300, 400))
                .unwrap();
        }
        for (name, solid) in [("solid.cbr", true), ("plain.cbr", false)] {
            let out = d.path().join(name);
            let mut cmd = std::process::Command::new(rar);
            cmd.arg("a").arg("-ep1").arg("-idq");
            if solid {
                cmd.arg("-s");
            }
            assert!(cmd.arg(&out).arg(imgs.join("*.png")).status().unwrap().success());
            let cache = d.path().join("cache");
            let src = open_comic(&out, 1, 1, &cache).unwrap();
            assert!(matches!(src, ComicSource::Rar(_)), "{name}: lần đầu giải nén dần");
            assert_eq!(src.page_count(), 30);
            // Trang cuối đọc được ngay (RAR thường) hoặc sau khi luồng nền tới nơi (solid).
            let want = crate::testdata::sample_png("p30", 300, 400);
            assert_eq!(src.page(29).unwrap().0, want, "{name}");
            assert_eq!(src.page(0).unwrap().0, crate::testdata::sample_png("p1", 300, 400));
            src.wait_extracted();
            let again = open_comic(&out, 1, 1, &cache).unwrap();
            assert!(matches!(again, ComicSource::Dir { .. }), "{name}: lần sau dùng cache");
            assert_eq!(again.page(9).unwrap().0, crate::testdata::sample_png("p10", 300, 400));
        }
    }
}
