//! My Librarian cho iPad / iPhone (Tauri 2 iOS) và điện thoại Android (Tauri 2 Android). Lệnh `lib_*` bọc `librarian_core::ipad_lib`; giao thức `book://`
//! phục vụ nội dung sách cho trình đọc dùng chung (`reader.js`) giống app Windows.

use librarian_core::ipad_lib::{self, Bookmark, IpadBook, Item, ReaderInfo, Removed, Sort, Usage};
use librarian_core::reader::ComicSource;
use librarian_core::rusqlite::Connection;
use librarian_core::sync_client::{self, Options, Pairing};
use librarian_core::sync_proto::{OutboxItem, PairQr};
use serde::Serialize;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

type CmdResult<T> = Result<T, String>;
fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

struct Lib {
    conn: Mutex<Connection>,
    db_path: PathBuf,
    sync_cfg: PathBuf,
    sync: Mutex<SyncCfg>,
    running: AtomicBool,
    cancel: Arc<AtomicBool>,
    /// iOS: `Documents/Books` – hiện trong app Tệp (cần UIFileSharingEnabled trong Info.plist, xem I1-03).
    /// Android: `Books` trong vùng dữ liệu riêng của app (không cần quyền bộ nhớ).
    books_dir: PathBuf,
    comic: Mutex<Option<(i64, Arc<ComicSource>)>>,
}

impl Lib {
    fn db<T>(&self, f: impl FnOnce(&Connection) -> librarian_core::Result<T>) -> CmdResult<T> {
        let c = self.conn.lock().map_err(|_| "khoá DB hỏng".to_string())?;
        f(&c).map_err(err)
    }
}

/// Đường dẫn file thật của sách: sách chép vào app nằm trong `books_dir`; sách mở tại chỗ lưu đường dẫn tuyệt đối.
fn book_path(lib: &Lib, b: &IpadBook) -> PathBuf {
    if b.in_place {
        PathBuf::from(&b.location)
    } else {
        lib.books_dir.join(&b.location)
    }
}

#[tauri::command]
fn lib_list(lib: State<Lib>, q: Option<String>, sort: Option<Sort>, not_worth: Option<bool>) -> CmdResult<Vec<Item>> {
    lib.db(|c| {
        ipad_lib::list(c, &ipad_lib::Query { q, sort: sort.unwrap_or_default(), not_worth: not_worth.unwrap_or(false) })
    })
}

/// Đánh dấu "không đáng đọc" / đưa lại vào thư viện. Giao diện gọi `sync_marks` ngay sau để báo máy tính.
#[tauri::command]
fn lib_set_not_worth(lib: State<Lib>, ids: Vec<i64>, on: bool) -> CmdResult<usize> {
    lib.db(|c| ipad_lib::set_not_worth(c, &ids, on))
}

#[tauri::command]
fn lib_not_worth_count(lib: State<Lib>) -> CmdResult<usize> {
    lib.db(ipad_lib::not_worth_count)
}

#[tauri::command]
fn lib_series(lib: State<Lib>, series: String) -> CmdResult<Vec<IpadBook>> {
    lib.db(|c| ipad_lib::series_volumes(c, &series))
}

#[tauri::command]
fn lib_continue(lib: State<Lib>, limit: Option<usize>) -> CmdResult<Vec<IpadBook>> {
    lib.db(|c| ipad_lib::continue_reading(c, limit.unwrap_or(10)))
}

#[tauri::command]
fn lib_remove(lib: State<Lib>, ids: Vec<i64>) -> CmdResult<Removed> {
    let r = lib.db(|c| ipad_lib::remove_books(c, &ids))?;
    for f in &r.files_to_delete {
        let p = lib.books_dir.join(f);
        // Chỉ xoá file nằm trong Books (không bao giờ đụng file ngoài).
        if p.starts_with(&lib.books_dir) && !f.contains("..") {
            let _ = std::fs::remove_file(p);
        }
    }
    if let Ok(mut c) = lib.comic.lock() {
        *c = None;
    }
    Ok(r)
}

#[tauri::command]
fn lib_read_ids(lib: State<Lib>, series: Option<String>) -> CmdResult<Vec<i64>> {
    lib.db(|c| ipad_lib::read_volume_ids(c, series.as_deref()))
}

#[derive(Serialize)]
struct UsageInfo {
    #[serde(flatten)]
    usage: Usage,
    free_bytes: Option<i64>,
    low_space: bool,
}

/// Dung lượng còn trống của phân vùng chứa `path`.
fn free_bytes(path: &Path) -> Option<i64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0)
        .then(|| (st.f_bavail as u64).saturating_mul(st.f_frsize as u64) as i64)
}

#[tauri::command]
fn lib_usage(lib: State<Lib>) -> CmdResult<UsageInfo> {
    let usage = lib.db(ipad_lib::usage)?;
    let free = free_bytes(&lib.books_dir);
    Ok(UsageInfo { usage, free_bytes: free, low_space: free.is_some_and(ipad_lib::low_space) })
}

#[tauri::command]
fn reader_info(lib: State<Lib>, id: i64) -> CmdResult<ReaderInfo> {
    lib.db(|c| ipad_lib::reader_info(c, id))
}

#[tauri::command]
fn save_position(lib: State<Lib>, id: i64, locator: Option<String>, progress: f64) -> CmdResult<String> {
    lib.db(|c| ipad_lib::save_position(c, id, locator.as_deref(), progress))
}

#[tauri::command]
fn set_read_status(lib: State<Lib>, ids: Vec<i64>, status: String) -> CmdResult<()> {
    lib.db(|c| ipad_lib::set_read_status(c, &ids, &status))
}

#[tauri::command]
fn open_external(_id: i64) -> CmdResult<()> {
    Err(format!("Định dạng này chưa đọc được trên {}", device_info().name))
}

// ------------------------------------------------------------------ đánh dấu trang

#[tauri::command]
fn bookmark_list(lib: State<Lib>, id: i64) -> CmdResult<Vec<Bookmark>> {
    lib.db(|c| ipad_lib::bookmarks(c, id))
}

#[tauri::command]
fn bookmark_add(
    lib: State<Lib>,
    id: i64,
    locator: Option<String>,
    progress: f64,
    label: String,
) -> CmdResult<Bookmark> {
    lib.db(|c| ipad_lib::add_bookmark(c, id, locator.as_deref(), progress, &label))
}

#[tauri::command]
fn bookmark_remove(lib: State<Lib>, bookmark: i64) -> CmdResult<()> {
    lib.db(|c| ipad_lib::remove_bookmark(c, bookmark))
}

// ------------------------------------------------------------------ iPhone, iPad hay điện thoại Android

/// Android: tên model (`ro.product.model`), ví dụ `Pixel 8`.
#[cfg(target_os = "android")]
fn machine() -> String {
    // PROP_VALUE_MAX = 92 (gồm cả byte 0 cuối).
    let mut buf = [0u8; 92];
    let n = unsafe { libc::__system_property_get(c"ro.product.model".as_ptr(), buf.as_mut_ptr().cast()) };
    String::from_utf8_lossy(&buf[..n.clamp(0, 91) as usize]).to_string()
}

/// Mã máy, ví dụ `iPhone16,2` / `iPad13,16`. Trên Simulator `hw.machine` là `arm64` → đọc biến môi trường.
#[cfg(target_vendor = "apple")]
fn machine() -> String {
    if let Ok(m) = std::env::var("SIMULATOR_MODEL_IDENTIFIER") {
        return m;
    }
    let mut buf = [0u8; 64];
    let mut len = buf.len();
    let ok = unsafe {
        libc::sysctlbyname(c"hw.machine".as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0)
    } == 0;
    if !ok {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..len]).trim_end_matches('\0').to_string()
}

#[cfg(not(any(target_os = "android", target_vendor = "apple")))]
fn machine() -> String {
    String::new()
}

/// iPhone và điện thoại Android: chỉ đọc sách chữ (không PDF, không truyện tranh), giao diện một tay.
fn is_phone() -> bool {
    cfg!(target_os = "android") || machine().starts_with("iPhone") || std::env::var("LIBRARIAN_PHONE").is_ok()
}

fn only_filter() -> Option<fn(&str) -> bool> {
    is_phone().then_some(ipad_lib::is_text_format as fn(&str) -> bool)
}

#[derive(Serialize)]
struct DeviceInfo {
    /// `iphone` | `ipad` | `android`
    kind: &'static str,
    /// Bố cục điện thoại (iPhone, Android): giao diện thêm lớp CSS `iphone`.
    phone: bool,
    /// Tên hiện trong giao diện và gửi cho máy tính khi ghép nối.
    name: &'static str,
    machine: String,
}

#[tauri::command]
fn device_info() -> DeviceInfo {
    let phone = is_phone();
    let (kind, name) = match (cfg!(target_os = "android"), phone) {
        (true, _) => ("android", "Android"),
        (_, true) => ("iphone", "iPhone"),
        _ => ("ipad", "iPad"),
    };
    DeviceInfo { kind, phone, name, machine: machine() }
}

/// Loại máy gửi cho máy tính khi ghép nối. Giao thức chỉ có `ipad` | `iphone`; `iphone` nghĩa là "điện thoại,
/// chỉ nhận sách chữ" nên điện thoại Android cũng gửi `iphone` (app Windows không cần đổi).
fn wire_kind() -> &'static str {
    if is_phone() {
        "iphone"
    } else {
        "ipad"
    }
}

// ------------------------------------------------------------------ đồng bộ với máy tính (I4)

#[derive(Default, Serialize, serde::Deserialize, Clone)]
struct SyncCfg {
    pairing: Option<Pairing>,
    #[serde(default = "yes")]
    auto_sync: bool,
}

fn yes() -> bool {
    true
}

impl Lib {
    fn save_cfg(&self, c: &SyncCfg) {
        if let Ok(s) = serde_json::to_string_pretty(c) {
            let _ = std::fs::write(&self.sync_cfg, s);
        }
    }
}

#[derive(Serialize)]
struct SyncState {
    paired: bool,
    pc_name: Option<String>,
    auto_sync: bool,
    running: bool,
}

#[tauri::command]
fn sync_state(lib: State<Lib>) -> CmdResult<SyncState> {
    let c = lib.sync.lock().map_err(|_| "khoá hỏng")?;
    Ok(SyncState {
        paired: c.pairing.is_some(),
        pc_name: c.pairing.as_ref().map(|p| p.pc_name.clone()),
        auto_sync: c.auto_sync,
        running: lib.running.load(Ordering::SeqCst),
    })
}

/// Ghép nối: `payload` là nội dung mã QR (JSON `PairQr`) hoặc, khi gõ tay, `addr` + `code`.
#[tauri::command]
async fn sync_pair(
    lib: State<'_, Lib>,
    payload: Option<String>,
    addr: Option<String>,
    code: Option<String>,
    device_name: Option<String>,
) -> CmdResult<SyncState> {
    let qr: PairQr = match payload {
        Some(p) => serde_json::from_str(&p).map_err(|_| "Mã QR không phải của My Librarian".to_string())?,
        None => {
            let a = addr.unwrap_or_default();
            let (host, port) = match a.rsplit_once(':') {
                Some((h, p)) if p.parse::<u16>().is_ok() && !h.contains(':') => (h.to_string(), p.parse().unwrap()),
                _ => (a.trim().to_string(), librarian_core::sync_proto::DEFAULT_PORT),
            };
            if host.is_empty() {
                return Err("Nhập địa chỉ IP của máy tính".into());
            }
            PairQr { v: 1, addrs: vec![host], port, code: code.unwrap_or_default(), library_id: String::new() }
        }
    };
    let info = device_info();
    let name = device_name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| info.name.into());
    let p = std::thread::spawn(move || sync_client::pair(&qr, &name, wire_kind()))
        .join()
        .map_err(|_| "lỗi nội bộ".to_string())?
        .map_err(err)?;
    let mut c = lib.sync.lock().map_err(|_| "khoá hỏng")?;
    c.pairing = Some(p);
    lib.save_cfg(&c);
    Ok(SyncState {
        paired: true,
        pc_name: c.pairing.as_ref().map(|p| p.pc_name.clone()),
        auto_sync: c.auto_sync,
        running: false,
    })
}

#[tauri::command]
fn sync_unpair(lib: State<Lib>) -> CmdResult<()> {
    let mut c = lib.sync.lock().map_err(|_| "khoá hỏng")?;
    c.pairing = None;
    lib.save_cfg(&c);
    Ok(())
}

#[tauri::command]
fn sync_set_auto(lib: State<Lib>, on: bool) -> CmdResult<()> {
    let mut c = lib.sync.lock().map_err(|_| "khoá hỏng")?;
    c.auto_sync = on;
    lib.save_cfg(&c);
    Ok(())
}

#[derive(Serialize)]
struct SyncCheck {
    reachable: bool,
    error: Option<String>,
    pc_name: String,
    drive_connected: bool,
    /// Các cuốn máy tính muốn gửi sang và chưa có trên iPad.
    items: Vec<OutboxItem>,
    need_bytes: i64,
    free_bytes: Option<i64>,
    enough_space: bool,
}

/// Hỏi máy tính có gì mới (không tải). Máy tắt/khác mạng thì `reachable = false`, không phải lỗi.
#[tauri::command]
async fn sync_check(lib: State<'_, Lib>) -> CmdResult<SyncCheck> {
    let mut p = lib.sync.lock().map_err(|_| "khoá hỏng")?.pairing.clone().ok_or("Chưa ghép nối với máy tính")?;
    let db_path = lib.db_path.clone();
    let free = free_bytes(&lib.books_dir);
    let out = std::thread::spawn(move || -> Result<SyncCheck, String> {
        let name = p.pc_name.clone();
        let (c, hello) = match sync_client::connect(&mut p) {
            Ok(x) => x,
            Err(e) => {
                return Ok(SyncCheck {
                    reachable: false,
                    error: Some(e.to_string()),
                    pc_name: name,
                    drive_connected: false,
                    items: vec![],
                    need_bytes: 0,
                    free_bytes: free,
                    enough_space: true,
                })
            }
        };
        let ob = c.outbox().map_err(err)?;
        let conn = ipad_lib::open(&db_path).map_err(err)?;
        let have = |sha: &Option<String>| match sha {
            Some(s) => conn.query_row("SELECT 1 FROM books WHERE sha256 = ?1", [s], |_| Ok(())).is_ok(),
            None => false,
        };
        let only = only_filter();
        let items: Vec<OutboxItem> =
            ob.items.into_iter().filter(|i| only.is_none_or(|f| f(&i.format)) && !have(&i.sha256)).collect();
        let need: i64 = items.iter().map(|i| i.size).sum();
        let enough = free.is_none_or(|f| need + sync_client::FREE_MARGIN <= f);
        Ok(SyncCheck {
            reachable: true,
            error: None,
            pc_name: hello.name,
            drive_connected: hello.drive_connected,
            items,
            need_bytes: need,
            free_bytes: free,
            enough_space: enough,
        })
    })
    .join()
    .map_err(|_| "lỗi nội bộ".to_string())??;
    Ok(out)
}

#[tauri::command]
fn sync_run(app: AppHandle, lib: State<Lib>) -> CmdResult<()> {
    if lib.running.swap(true, Ordering::SeqCst) {
        return Err("Đang đồng bộ".into());
    }
    lib.cancel.store(false, Ordering::SeqCst);
    let Some(mut p) = lib.sync.lock().map_err(|_| "khoá hỏng")?.pairing.clone() else {
        lib.running.store(false, Ordering::SeqCst);
        return Err("Chưa ghép nối với máy tính".into());
    };
    let (db_path, books_dir, cancel) = (lib.db_path.clone(), lib.books_dir.clone(), lib.cancel.clone());
    std::thread::spawn(move || {
        let mut run = || -> Result<sync_client::Report, String> {
            let (c, _) = sync_client::connect(&mut p).map_err(err)?;
            let conn = ipad_lib::open(&db_path).map_err(err)?;
            let o = Options {
                books_dir: &books_dir,
                free_bytes: free_bytes(&books_dir),
                cancel: &cancel,
                only: only_filter(),
            };
            let a = app.clone();
            sync_client::sync(&conn, &c, &o, &move |pr| {
                let _ = a.emit("sync_progress", pr);
            })
            .map_err(err)
        };
        let res = run();
        // Địa chỉ chạy được nhớ lại cho lần sau.
        if let Some(lib) = app.try_state::<Lib>() {
            if let Ok(mut c) = lib.sync.lock() {
                if c.pairing.is_some() {
                    c.pairing = Some(p.clone());
                    lib.save_cfg(&c);
                }
            }
            lib.running.store(false, Ordering::SeqCst);
        }
        let _ = app.emit(
            "sync_done",
            match res {
                Ok(r) => serde_json::json!({ "report": r }),
                Err(e) => serde_json::json!({ "error": e }),
            },
        );
    });
    Ok(())
}

/// Đồng bộ riêng cờ "không đáng đọc" với máy tính (nhanh, không tải sách) – chạy khi mở app, khi app quay lại
/// màn hình và ngay sau khi đổi cờ. Máy tính tắt / khác mạng thì trả `None`, không phải lỗi.
/// Trả về số cuốn trên máy này đổi theo máy tính.
#[tauri::command]
async fn sync_marks(lib: State<'_, Lib>) -> CmdResult<Option<usize>> {
    let Some(mut p) = lib.sync.lock().map_err(|_| "khoá hỏng")?.pairing.clone() else { return Ok(None) };
    let db_path = lib.db_path.clone();
    std::thread::spawn(move || -> Option<usize> {
        let (c, _) = sync_client::connect(&mut p).ok()?;
        let conn = ipad_lib::open(&db_path).ok()?;
        sync_client::sync_marks(&conn, &c, &p.library_id).ok()
    })
    .join()
    .map_err(|_| "lỗi nội bộ".to_string())
}

#[tauri::command]
fn sync_cancel(lib: State<Lib>) {
    lib.cancel.store(true, Ordering::SeqCst);
}

// ------------------------------------------------------------------ giao thức book://

type Resp = tauri::http::Response<Vec<u8>>;

fn reply(status: u16, ctype: &str, body: Vec<u8>) -> Resp {
    tauri::http::Response::builder()
        .status(status)
        .header("Content-Type", ctype)
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Headers", "Range")
        .header("Access-Control-Expose-Headers", "Content-Range, Content-Length, Accept-Ranges")
        .header("Cache-Control", "no-store")
        .body(body)
        .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
}

fn fail(status: u16, msg: &str) -> Resp {
    reply(status, "text/plain; charset=utf-8", msg.as_bytes().to_vec())
}

fn mime(format: &str) -> &'static str {
    match format {
        "epub" => "application/epub+zip",
        "pdf" => "application/pdf",
        "fb2" => "application/x-fictionbook+xml",
        "cbz" => "application/vnd.comicbook+zip",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// `Range: bytes=a-b` → (a, b) gồm cả hai đầu, đã kẹp theo dung lượng file.
fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    let spec = h.trim().strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    let last = len.checked_sub(1)?;
    let (start, end) = if a.is_empty() {
        (len.saturating_sub(b.parse::<u64>().ok()?), last)
    } else {
        let end = if b.is_empty() { last } else { b.parse::<u64>().ok()?.min(last) };
        (a.parse().ok()?, end)
    };
    (start <= end).then_some((start, end))
}

const MAX_RANGE: u64 = 32 * 1024 * 1024;

fn serve_file(path: &Path, format: &str, range: Option<&str>) -> Resp {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return fail(404, &format!("Không mở được file: {e}")),
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    match range {
        Some(r) => {
            let Some((start, end)) = parse_range(r, len) else {
                return tauri::http::Response::builder()
                    .status(416)
                    .header("Content-Range", format!("bytes */{len}"))
                    .body(Vec::new())
                    .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()));
            };
            let end = end.min(start + MAX_RANGE - 1);
            let mut buf = vec![0u8; (end - start + 1) as usize];
            if let Err(e) = f.seek(SeekFrom::Start(start)).and_then(|_| f.read_exact(&mut buf)) {
                return fail(500, &format!("Lỗi đọc file: {e}"));
            }
            let mut r = reply(206, mime(format), buf);
            r.headers_mut().insert("Accept-Ranges", "bytes".parse().unwrap());
            r.headers_mut().insert("Content-Range", format!("bytes {start}-{end}/{len}").parse().unwrap());
            r
        }
        None => {
            let mut buf = Vec::with_capacity(len as usize);
            if let Err(e) = f.read_to_end(&mut buf) {
                return fail(500, &format!("Lỗi đọc file: {e}"));
            }
            let mut r = reply(200, mime(format), buf);
            r.headers_mut().insert("Accept-Ranges", "bytes".parse().unwrap());
            r
        }
    }
}

fn comic(lib: &Lib, b: &IpadBook, path: &Path) -> librarian_core::Result<Arc<ComicSource>> {
    let mut slot = lib.comic.lock().map_err(|_| librarian_core::Error::msg("khoá hỏng"))?;
    if let Some((id, s)) = slot.as_ref() {
        if *id == b.id {
            return Ok(s.clone());
        }
    }
    let src = Arc::new(librarian_core::reader::open_comic(path, b.size, 0, &lib.books_dir)?);
    *slot = Some((b.id, src.clone()));
    Ok(src)
}

/// `book://localhost/<id>/file` (có Range), `/<id>/pages` (JSON số trang), `/<id>/page/<n>` (ảnh).
fn handle_book(lib: &Lib, req: &tauri::http::Request<Vec<u8>>) -> Resp {
    if req.method() == tauri::http::Method::OPTIONS {
        return reply(204, "text/plain", Vec::new());
    }
    let parts: Vec<&str> = req.uri().path().trim_matches('/').split('/').collect();
    let Some(id) = parts.first().and_then(|s| s.parse::<i64>().ok()) else {
        return fail(400, "bad id");
    };
    let Ok(Some(b)) = lib.db(|c| ipad_lib::get(c, id)) else {
        return fail(404, "Không có sách này");
    };
    let path = book_path(lib, &b);
    match parts.get(1).copied() {
        Some("file") => {
            let range = req.headers().get("range").and_then(|v| v.to_str().ok());
            serve_file(&path, &b.format, range)
        }
        Some("pages") => match comic(lib, &b, &path) {
            Ok(s) => reply(200, "application/json", format!("{{\"count\":{}}}", s.page_count()).into_bytes()),
            Err(e) => fail(422, &e.to_string()),
        },
        Some("page") => {
            let Some(n) = parts.get(2).and_then(|s| s.parse::<usize>().ok()) else {
                return fail(400, "bad page");
            };
            match comic(lib, &b, &path).and_then(|s| s.page(n)) {
                Ok((bytes, m)) => reply(200, m, bytes),
                Err(e) => fail(404, &e.to_string()),
            }
        }
        _ => fail(400, "bad path"),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let b = tauri::Builder::default();
    #[cfg(mobile)]
    let b = b.plugin(tauri_plugin_barcode_scanner::init());
    b.register_asynchronous_uri_scheme_protocol("book", |ctx, request, responder| {
        let app = ctx.app_handle().clone();
        std::thread::spawn(move || {
            let resp = match app.try_state::<Lib>() {
                Some(lib) => handle_book(&lib, &request),
                None => fail(503, "Thư viện chưa sẵn sàng"),
            };
            responder.respond(resp);
        });
    })
    .setup(|app| {
        // Android: thư mục Documents chung cần quyền bộ nhớ (scoped storage) → dùng vùng dữ liệu riêng của app.
        let docs = if cfg!(target_os = "android") {
            app.path().app_data_dir()?
        } else {
            app.path().document_dir().or_else(|_| app.path().app_data_dir())?
        };
        let books_dir = docs.join("Books");
        std::fs::create_dir_all(&books_dir)?;
        let data = app.path().app_data_dir()?;
        std::fs::create_dir_all(&data)?;
        let db_path = data.join("ipad.db");
        let conn = ipad_lib::open(&db_path)?;
        let sync_cfg = data.join("sync.json");
        let cfg: SyncCfg = std::fs::read_to_string(&sync_cfg)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(SyncCfg { pairing: None, auto_sync: true });
        app.manage(Lib {
            conn: Mutex::new(conn),
            db_path,
            sync_cfg,
            sync: Mutex::new(cfg),
            running: AtomicBool::new(false),
            cancel: Arc::new(AtomicBool::new(false)),
            books_dir,
            comic: Mutex::new(None),
        });
        Ok(())
    })
    .invoke_handler(tauri::generate_handler![
        lib_list,
        lib_set_not_worth,
        lib_not_worth_count,
        sync_marks,
        lib_series,
        lib_continue,
        lib_remove,
        lib_read_ids,
        lib_usage,
        reader_info,
        save_position,
        set_read_status,
        open_external,
        bookmark_list,
        bookmark_add,
        bookmark_remove,
        device_info,
        sync_state,
        sync_pair,
        sync_unpair,
        sync_set_auto,
        sync_check,
        sync_run,
        sync_cancel
    ])
    .run(tauri::generate_context!())
    .expect("lỗi khi chạy My Librarian iPad");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=50-500", 100), Some((50, 99)));
        assert_eq!(parse_range("bytes=200-300", 100), None);
    }
}
