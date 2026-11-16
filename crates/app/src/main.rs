//! My Librarian – app Tauri 2. Toàn bộ nghiệp vụ nằm trong `librarian-core`; file này chỉ nối IPC.
//! Lệnh nặng chạy trên luồng blocking (không chặn UI); ảnh bìa phục vụ qua giao thức `cover://`
//! (WebView2: `http://cover.localhost/<id>`) bởi một nhóm luồng riêng, sinh lười và cache trên đĩa.
//! Trình đọc (M5) lấy nội dung sách qua giao thức `book://` (`http://book.localhost/…`): file gốc có
//! hỗ trợ HTTP Range (PDF chỉ tải phần cần), và từng trang ảnh của truyện CBZ/CBR.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use librarian_core::blocked;
use librarian_core::browse::{self, Page, Query};
use librarian_core::classify::{self, Proposal};
use librarian_core::covers::CoverCache;
use librarian_core::dedupe::{self, Decision, ExactGroup, SuspectGroup};
use librarian_core::formats::Format;
use librarian_core::moves::{self, BatchInfo, MoveReport, MoveRequest, Operation, UndoReport};
use librarian_core::progress::{self, Watchdog};
use librarian_core::reader::{self, ComicSource, ReaderInfo};
use librarian_core::report::{self, ReportStats};
use librarian_core::retitle::{self, RetitleOptions, RetitleReport};
use librarian_core::scan::ScanProgress;
use librarian_core::tagmap;
use librarian_core::trash::{self, SystemTrasher, TrashReport};
use librarian_core::volumes;
use librarian_core::Step;
use librarian_core::{config, db, lock, Config, Library};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State, UriSchemeResponder};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

type Shared = Arc<Mutex<Library>>;

struct AppState {
    home: PathBuf,
    lib: Shared,
    scan_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    /// Đang dọn trùng chắc chắn (chạy nền, có tiến trình): cờ dừng của lần dọn đó.
    clean_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    /// Người dùng đã đồng ý đóng app khi đang quét / dọn trùng: đang dừng, xong thì thoát.
    quitting: Arc<AtomicBool>,
    /// Sách mở từ Explorer nhưng không nằm trong thư viện: id âm → đường dẫn (không ghi DB).
    external: External,
    /// File được truyền qua dòng lệnh lúc khởi động, chờ giao diện lấy (`take_open_request`).
    pending_open: Mutex<Option<String>>,
    /// Máy chủ đồng bộ sang iPad (I3): tắt mặc định, chỉ chạy khi người dùng bật.
    sync: Mutex<Option<librarian_core::sync::SyncServer>>,
}

type External = Arc<Mutex<HashMap<i64, PathBuf>>>;

fn external_path(ext: &External, id: i64) -> Option<PathBuf> {
    ext.lock().ok()?.get(&id).cloned()
}

/// Đối số dòng lệnh đầu tiên là file sách tồn tại (Explorer: "Mở bằng My Librarian").
fn book_arg(args: &[String]) -> Option<String> {
    args.iter()
        .skip(1)
        .find(|a| !a.starts_with('-') && Path::new(a).is_file() && Format::from_path(Path::new(a)).is_some())
        .cloned()
}

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Chạy thao tác làm đổi số trùng / nghi trùng / chờ phân loại, rồi cập nhật các số đó trong báo cáo gần nhất
/// (badge + banner đọc từ báo cáo). Cập nhật lỗi thì chỉ ghi log, không làm hỏng thao tác đã xong.
fn refreshed<T>(
    lib: &mut Library,
    f: impl FnOnce(&mut Library) -> librarian_core::Result<T>,
) -> librarian_core::Result<T> {
    let out = f(lib)?;
    if let Err(e) = report::refresh_counts(&lib.conn, &lib.config) {
        tracing::warn!("cập nhật số liệu báo cáo lỗi: {e}");
    }
    Ok(out)
}

/// Chạy `f` với thư viện trên luồng blocking.
async fn with_lib<T: Send + 'static>(
    lib: &Shared,
    f: impl FnOnce(&mut Library) -> librarian_core::Result<T> + Send + 'static,
) -> CmdResult<T> {
    let lib = lib.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Một lệnh panic khi đang giữ khoá thì khoá bị "poisoned"; dữ liệu (kết nối SQLite) vẫn dùng được,
        // nên lấy lại khoá thay vì báo lỗi cho mọi lệnh sau đó.
        let mut g = lib.lock().unwrap_or_else(|e| {
            lib.clear_poison();
            e.into_inner()
        });
        f(&mut g).map_err(err)
    })
    .await
    .map_err(err)?
}

// ------------------------------------------------------------------ tổng quan & cấu hình

#[derive(Serialize)]
struct CategoryInfo {
    name: String,
    books: i64,
    series_dirs: Vec<String>,
    path: String,
}

#[derive(Serialize)]
struct Overview {
    config: Config,
    library_ok: bool,
    facets: browse::Facets,
    categories: Vec<CategoryInfo>,
    banner: Option<String>,
    /// Thông báo một lần lúc khởi động (DB hỏng đã khôi phục…).
    notice: Option<String>,
    report: Option<report::Report>,
    scanning: bool,
    /// Đang dọn trùng chắc chắn ở nền (giao diện tải lại thì mở lại hộp tiến trình).
    cleaning: bool,
    home: String,
}

#[tauri::command]
async fn overview(state: State<'_, AppState>) -> CmdResult<Overview> {
    let scanning = is_scanning(&state);
    let cleaning = is_cleaning(&state);
    let home = state.home.to_string_lossy().into_owned();
    with_lib(&state.lib, move |lib| {
        let library_ok = lib.config.library_online();
        let counts = librarian_core::search::category_counts(&lib.conn)?;
        let categories = if library_ok {
            classify::list_categories(&lib.config)?
                .into_iter()
                .map(|c| CategoryInfo {
                    books: counts
                        .iter()
                        .filter(|x| librarian_core::normalize::fold(&x.category) == c.key)
                        .map(|x| x.books)
                        .sum(),
                    series_dirs: c.series_dirs.iter().map(|s| s.name.clone()).collect(),
                    path: c.path.to_string_lossy().into_owned(),
                    name: c.name,
                })
                .collect()
        } else {
            Vec::new()
        };
        let rep = report::latest(&lib.conn)?;
        Ok(Overview {
            config: lib.config.clone(),
            library_ok,
            facets: browse::facets(&lib.conn)?,
            categories,
            banner: rep.as_ref().and_then(report::banner),
            notice: lib.notice.take(),
            report: rep,
            scanning,
            cleaning,
            home,
        })
    })
    .await
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> CmdResult<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Chọn thư mục thư viện (books)")
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .map_err(err)
}

#[tauri::command]
async fn set_library(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    with_lib(&state.lib, move |lib| lib.set_library_root(Path::new(&path))).await
}

#[tauri::command]
async fn save_config(state: State<'_, AppState>, config: Config) -> CmdResult<()> {
    with_lib(&state.lib, move |lib| {
        // Danh sách chặn chỉ đổi qua block_category / unblock_category, ánh xạ tag qua tag_map_save (form Cài đặt
        // có thể giữ bản cũ).
        // Thư mục thư viện thêm chỉ đổi qua add_library_folder / remove_library_folder.
        let blocked = std::mem::take(&mut lib.config.blocked_categories);
        let tag_map = std::mem::take(&mut lib.config.tag_map);
        let extra = std::mem::take(&mut lib.config.extra_libraries);
        lib.config = config;
        lib.config.blocked_categories = blocked;
        lib.config.tag_map = tag_map;
        lib.config.extra_libraries = extra;
        db::sync_library_roots(&lib.conn, &lib.config)?;
        lib.save_config()
    })
    .await
}

// ------------------------------------------------------------------ quét

#[derive(Clone, Serialize)]
struct DoneEvent {
    ok: bool,
    message: Option<String>,
    stats: Option<ReportStats>,
}

/// Quét + băm + phân tích trên luồng riêng với kết nối DB riêng (WAL cho phép UI đọc song song).
#[tauri::command]
fn start_scan(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    if is_cleaning(&state) {
        return Err("Đang dọn trùng – đợi xong rồi quét".into());
    }
    let mut slot = state.scan_cancel.lock().map_err(err)?;
    if slot.is_some() {
        return Err("Đang quét".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *slot = Some(cancel.clone());
    let home = state.home.clone();
    let slot_ref = state.scan_cancel.clone();
    std::thread::spawn(move || {
        // Giám sát treo: mỗi giây gửi "bước nào, đứng yên bao lâu" lên giao diện; quá 60 giây không có tiến
        // triển thì tự huỷ quét và báo lỗi (xem librarian_core::progress).
        // Báo "xong" đúng một lần: hoặc khi luồng quét trả về, hoặc ngay khi phát hiện treo (từ luồng giám sát) –
        // luồng quét có thể kẹt mãi trong một lệnh I/O không trả về, giao diện không được chờ nó.
        let reported = Arc::new(AtomicBool::new(false));
        let finish_ui = {
            let (app, slot_ref, cancel, reported) = (app.clone(), slot_ref.clone(), cancel.clone(), reported.clone());
            move |ev: DoneEvent| {
                if reported.swap(true, Ordering::SeqCst) {
                    return;
                }
                // chỉ nhả ô của chính lần quét này (lần quét bị bỏ rơi trả về muộn không được xoá ô của lần sau)
                if let Ok(mut s) = slot_ref.lock() {
                    if s.as_ref().is_some_and(|c| Arc::ptr_eq(c, &cancel)) {
                        *s = None;
                    }
                }
                let _ = app.emit("scan-done", ev);
            }
        };
        let health_app = app.clone();
        let on_stall = {
            let finish_ui = finish_ui.clone();
            move |stall: &progress::Stall| {
                finish_ui(DoneEvent { ok: false, message: Some(stall.to_string()), stats: None });
            }
        };
        let watchdog = Watchdog::start_with(
            cancel.clone(),
            progress::STALL_LIMIT,
            move |h| {
                let _ = health_app.emit("scan-health", h);
            },
            on_stall,
        );
        let res = (|| -> librarian_core::Result<ReportStats> {
            let _lock = lock::ScanLock::try_acquire(&home)?;
            let lib = Library::open(&home)?;
            let emit_step = |s: Step| {
                watchdog.observe(s);
                let _ = app.emit("scan-progress", s);
            };
            // Tổng do lõi đếm trước khi quét (0 = đang đếm) → mẫu số "x / y" cố định trong một lần quét.
            let progress = |p: &ScanProgress| {
                let total = if p.total == 0 { 0 } else { p.total.max(p.files_seen) };
                emit_step(Step { phase: p.phase, done: p.files_seen, total })
            };
            let summary = librarian_core::scan::scan(
                &lib.conn,
                &lib.config,
                &librarian_core::scan::ScanOptions {
                    cancel: Some(&cancel),
                    progress: Some(&progress),
                    kind: Some("app"),
                },
            )?;
            if summary.cancelled {
                return Ok(ReportStats { scan: summary, ..Default::default() });
            }
            // run_analysis: tự phân loại hộp thư đến → quét lại (tăng dần, nhanh vì sách đã chuyển được cập nhật
            // đường dẫn ngay) → băm + phân tích. Mỗi bước lưu kết quả theo từng sách/lô.
            report::run_analysis_with(&lib.conn, &lib.config, "app", Some(&cancel), None, Some(&emit_step))
        })();
        let stall = watchdog.finish();
        let ev = match (res, stall) {
            (_, Some(stall)) => DoneEvent { ok: false, message: Some(stall.to_string()), stats: None },
            (Ok(st), None) => DoneEvent { ok: true, message: None, stats: Some(st) },
            (Err(e), None) => DoneEvent { ok: false, message: Some(e.to_string()), stats: None },
        };
        finish_ui(ev); // không làm gì nếu đã báo lúc phát hiện treo
    });
    Ok(())
}

#[tauri::command]
fn cancel_scan(state: State<'_, AppState>) {
    if let Ok(g) = state.scan_cancel.lock() {
        if let Some(c) = g.as_ref() {
            c.store(true, Ordering::Relaxed);
        }
    }
}

fn is_scanning(state: &AppState) -> bool {
    state.scan_cancel.lock().map(|g| g.is_some()).unwrap_or(false)
}

fn is_cleaning(state: &AppState) -> bool {
    state.clean_cancel.lock().map(|g| g.is_some()).unwrap_or(false)
}

/// Việc nền đang chạy mà đóng app phải hỏi lại: `scan` · `clean` · `None`.
fn busy_kind(state: &AppState) -> Option<&'static str> {
    if is_cleaning(state) {
        Some("clean")
    } else if is_scanning(state) {
        Some("scan")
    } else {
        None
    }
}

/// Đóng app khi đang quét / dọn trùng (người dùng đã xác nhận): báo dừng, đợi luồng nền lưu phần đã làm
/// và nhả khoá rồi mới thoát. Ổ chậm có thể mất một lúc cho file đang đọc / đang xoá dở; quá lâu thì thoát luôn
/// (mỗi file đã xử lý được ghi DB ngay nên vẫn an toàn, lần sau làm tiếp phần còn lại).
#[tauri::command]
fn quit_app(app: AppHandle, state: State<'_, AppState>) {
    state.quitting.store(true, Ordering::Relaxed);
    let slots = [state.scan_cancel.clone(), state.clean_cancel.clone()];
    for slot in &slots {
        if let Ok(g) = slot.lock() {
            if let Some(c) = g.as_ref() {
                c.store(true, Ordering::Relaxed);
            }
        }
    }
    std::thread::spawn(move || {
        let busy = || slots.iter().any(|s| s.lock().map(|g| g.is_some()).unwrap_or(false));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline && busy() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let stopped = !busy();
        tracing::info!("đóng app khi đang chạy nền: {}", if stopped { "đã dừng" } else { "quá 15 giây, thoát luôn" });
        app.exit(0);
        // Phòng khi vòng sự kiện không thoát (vd. luồng quét đang kẹt ở một lần đọc ổ đĩa): thoát hẳn tiến trình.
        std::thread::sleep(std::time::Duration::from_secs(3));
        std::process::exit(0);
    });
}

// ------------------------------------------------------------------ thư viện

#[tauri::command]
async fn query(state: State<'_, AppState>, q: Query) -> CmdResult<Page> {
    with_lib(&state.lib, move |lib| browse::query(&lib.conn, &q)).await
}

#[tauri::command]
async fn book(state: State<'_, AppState>, id: i64) -> CmdResult<Option<db::Book>> {
    with_lib(&state.lib, move |lib| db::get_book(&lib.conn, id)).await
}

#[tauri::command]
async fn set_read_status(state: State<'_, AppState>, ids: Vec<i64>, status: String) -> CmdResult<()> {
    let ids: Vec<i64> = ids.into_iter().filter(|&i| i > 0).collect();
    with_lib(&state.lib, move |lib| browse::set_read_status(&lib.conn, &ids, &status)).await
}

/// Đánh dấu "không đáng đọc" (ẩn khỏi thư viện) hoặc đưa lại vào thư viện. iPad/iPhone/Android nhận theo
/// ở lần đồng bộ kế tiếp (tự chạy khi mở app trên thiết bị cùng Wi-Fi).
#[tauri::command]
async fn set_not_worth(state: State<'_, AppState>, ids: Vec<i64>, on: bool) -> CmdResult<usize> {
    let ids: Vec<i64> = ids.into_iter().filter(|&i| i > 0).collect();
    with_lib(&state.lib, move |lib| browse::set_not_worth(&lib.conn, &ids, on)).await
}

async fn book_path(state: &State<'_, AppState>, id: i64) -> CmdResult<String> {
    if id < 0 {
        return external_path(&state.external, id)
            .map(|p| p.to_string_lossy().into_owned())
            .ok_or_else(|| "Không có sách này".into());
    }
    with_lib(&state.lib, move |lib| {
        db::get_book(&lib.conn, id)?.map(|b| b.path).ok_or_else(|| librarian_core::Error::msg("Không có sách này"))
    })
    .await
}

/// Mở thư mục chứa file trong Explorer, chọn sẵn file (E3-06).
#[tauri::command]
async fn reveal(app: AppHandle, state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    let p = book_path(&state, id).await?;
    app.opener().reveal_item_in_dir(p).map_err(err)
}

/// Mở bằng ứng dụng mặc định của Windows (DJVU, hoặc khi bạn muốn dùng app khác).
#[tauri::command]
async fn open_external(app: AppHandle, state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    let p = book_path(&state, id).await?;
    app.opener().open_path(p, None::<&str>).map_err(err)
}

// ------------------------------------------------------------------ trình đọc (M5)

#[tauri::command]
async fn reader_info(state: State<'_, AppState>, id: i64) -> CmdResult<ReaderInfo> {
    if id < 0 {
        let p = external_path(&state.external, id).ok_or("Không có sách này")?;
        return tauri::async_runtime::spawn_blocking(move || reader::external_info(id, &p).map_err(err))
            .await
            .map_err(err)?;
    }
    with_lib(&state.lib, move |lib| reader::reader_info(&lib.conn, id)).await
}

/// Lưu vị trí đọc (E7-06); trả trạng thái đọc mới (`reading` / `read`).
#[tauri::command]
async fn save_position(
    state: State<'_, AppState>,
    id: i64,
    locator: Option<String>,
    progress: f64,
) -> CmdResult<String> {
    if id < 0 {
        return Ok("reading".into()); // sách ngoài thư viện: không lưu
    }
    with_lib(&state.lib, move |lib| reader::save_position(&lib.conn, id, locator.as_deref(), progress)).await
}

/// Đường dẫn file được truyền lúc khởi động (nếu có), lấy một lần.
#[tauri::command]
fn take_open_request(state: State<'_, AppState>) -> Option<String> {
    state.pending_open.lock().ok()?.take()
}

/// Mở một file từ Explorer: trả id sách trong thư viện, hoặc id âm tạm thời nếu file ở ngoài thư viện.
#[tauri::command]
async fn open_path(state: State<'_, AppState>, path: String) -> CmdResult<i64> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("Không tìm thấy file {path}"));
    }
    let q = p.clone();
    if let Some(id) = with_lib(&state.lib, move |lib| reader::find_by_path(&lib.conn, &q)).await? {
        return Ok(id);
    }
    let mut ext = state.external.lock().map_err(err)?;
    if let Some((id, _)) = ext.iter().find(|(_, v)| **v == p) {
        return Ok(*id);
    }
    let id = -(ext.len() as i64) - 1;
    ext.insert(id, p);
    Ok(id)
}

/// Bật/tắt toàn màn hình (F11) cho cửa sổ chính; trả trạng thái mới.
#[tauri::command]
fn toggle_fullscreen(window: tauri::WebviewWindow) -> CmdResult<bool> {
    let on = !window.is_fullscreen().map_err(err)?;
    window.set_fullscreen(on).map_err(err)?;
    Ok(on)
}

#[tauri::command]
fn open_recycle_bin() -> CmdResult<()> {
    trash::open_recycle_bin().map_err(err)
}

#[tauri::command]
async fn trash_books(state: State<'_, AppState>, ids: Vec<i64>) -> CmdResult<TrashReport> {
    if ids.iter().any(|&i| i < 0) {
        return Err("Chỉ xoá được sách trong thư viện".into());
    }
    with_lib(&state.lib, move |lib| {
        refreshed(lib, |lib| trash::trash_books(&lib.conn, &lib.config, &SystemTrasher, &ids))
    })
    .await
}

/// Sửa tiêu đề (một cuốn, hoặc mọi định dạng của một tác phẩm): DB + tên file + (EPUB/FB2) nội dung sách.
/// Xem `librarian_core::retitle`. Không chạy khi đang quét / dọn trùng (đều đụng tới chính các file này).
#[tauri::command]
async fn retitle_books(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    title: String,
    rename_file: bool,
    write_content: bool,
) -> CmdResult<RetitleReport> {
    if ids.iter().any(|&i| i < 0) {
        return Err("Chỉ sửa được sách trong thư viện".into());
    }
    if busy_kind(&state).is_some() {
        return Err("Đang quét hoặc dọn trùng – đợi xong rồi sửa tiêu đề".into());
    }
    let opts = RetitleOptions { rename_file, write_content };
    with_lib(&state.lib, move |lib| {
        refreshed(lib, |lib| retitle::retitle_books(&lib.conn, &lib.config, &SystemTrasher, &ids, &title, opts))
    })
    .await
}

// ------------------------------------------------------------------ thể loại bị chặn

/// Cây thư mục thể loại (3 cấp) kèm số sách và trạng thái chặn – để chọn thư mục cần chặn.
#[tauri::command]
async fn category_tree(state: State<'_, AppState>) -> CmdResult<Vec<blocked::FolderNode>> {
    with_lib(&state.lib, |lib| blocked::folder_tree(&lib.conn, &lib.config, 3)).await
}

/// Chặn thư mục `rel` (`A/B`): lưu vào cấu hình rồi đưa mọi sách trong đó (và chính thư mục) vào Thùng rác ở nền –
/// cùng hộp tiến trình / giám sát treo / nút Dừng như dọn trùng. Từ đó sách tự phân loại vào đó cũng vào Thùng rác.
#[tauri::command]
async fn block_category(app: AppHandle, state: State<'_, AppState>, rel: String) -> CmdResult<()> {
    if busy_kind(&state).is_some() {
        return Err("Đang quét hoặc dọn trùng – đợi xong rồi chặn".into());
    }
    let rel = with_lib(&state.lib, move |lib| {
        let rel = blocked::add(&mut lib.config, &rel)?;
        lib.save_config()?;
        Ok(rel)
    })
    .await?;
    spawn_clean(app, state, move |lib, cancel, on| {
        blocked::enforce(&lib.conn, &lib.config, &SystemTrasher, Some(&rel), Some(cancel), Some(on))
    })
}

/// Bỏ chặn `rel` (sách đã xoá vẫn nằm trong Thùng rác Windows, khôi phục được).
#[tauri::command]
async fn unblock_category(state: State<'_, AppState>, rel: String) -> CmdResult<bool> {
    with_lib(&state.lib, move |lib| {
        let removed = blocked::remove(&mut lib.config, &rel);
        if removed {
            lib.save_config()?;
        }
        Ok(removed)
    })
    .await
}

// ------------------------------------------------------------------ ánh xạ tag → thể loại

/// Mọi tag (metadata `subject`) của sách trong thư viện kèm số sách, nơi đang nằm và thư mục đang được gán.
#[tauri::command]
async fn tag_list(state: State<'_, AppState>) -> CmdResult<Vec<tagmap::TagInfo>> {
    with_lib(&state.lib, |lib| tagmap::list_tags(&lib.conn, &lib.config)).await
}

/// Kế hoạch: áp `changes` lên bản sao cấu hình (chưa lưu) rồi tìm sách cần chuyển – sách mang các tag vừa đổi, hoặc
/// mọi tag đã gán nếu `all`.
fn tag_plan(lib: &Library, changes: &[tagmap::Change], all: bool) -> librarian_core::Result<(Config, tagmap::Plan)> {
    let mut cfg = lib.config.clone();
    let olds = tagmap::apply_changes(&mut cfg, changes)?;
    let opts = tagmap::PlanOptions {
        tags: (!all).then(|| changes.iter().map(|c| c.tag.clone()).collect()),
        old_targets: olds,
        first_seen_scan: None,
    };
    let plan = tagmap::plan(&lib.conn, &cfg, &opts)?;
    Ok((cfg, plan))
}

/// Xem trước: sách nào sẽ chuyển đi đâu nếu lưu `changes` (không đổi gì).
#[tauri::command]
async fn tag_map_plan(state: State<'_, AppState>, changes: Vec<tagmap::Change>, all: bool) -> CmdResult<tagmap::Plan> {
    with_lib(&state.lib, move |lib| tag_plan(lib, &changes, all).map(|(_, p)| p)).await
}

/// Lưu `changes` vào cấu hình rồi chuyển sách mang các tag đó sang thư mục mới (`all`: theo mọi tag đã gán). Lô
/// `tagmap` trong Hoạt động, hoàn tác được.
#[tauri::command]
async fn tag_map_save(state: State<'_, AppState>, changes: Vec<tagmap::Change>, all: bool) -> CmdResult<MoveReport> {
    if busy_kind(&state).is_some() {
        return Err("Đang quét hoặc dọn trùng – đợi xong rồi đổi thể loại".into());
    }
    with_lib(&state.lib, move |lib| {
        let (cfg, plan) = tag_plan(lib, &changes, all)?;
        lib.config.tag_map = cfg.tag_map;
        lib.save_config()?;
        refreshed(lib, |lib| tagmap::apply(&lib.conn, &lib.config, &plan))
    })
    .await
}

// ------------------------------------------------------------------ ổ lưu sách (thư viện trên nhiều ổ)

/// Các thư mục thư viện kèm dung lượng ổ, số sách.
#[tauri::command]
async fn volumes_info(state: State<'_, AppState>) -> CmdResult<Vec<volumes::Volume>> {
    with_lib(&state.lib, |lib| volumes::list(&lib.conn, &lib.config)).await
}

/// Chọn một thư mục (thường ở ổ khác) rồi thêm vào thư viện. `None` = người dùng huỷ hộp chọn.
#[tauri::command]
async fn add_library_folder(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Option<String>> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Chọn thư mục sách thêm (thường ở ổ khác)")
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
    })
    .await
    .map_err(err)?;
    let Some(path) = picked else { return Ok(None) };
    let shown = path.to_string_lossy().into_owned();
    with_lib(&state.lib, move |lib| lib.add_library_root(&path)).await?;
    Ok(Some(shown))
}

/// Bỏ một thư mục thư viện thêm khỏi app (file trên ổ giữ nguyên).
#[tauri::command]
async fn remove_library_folder(state: State<'_, AppState>, path: String) -> CmdResult<bool> {
    if busy_kind(&state).is_some() {
        return Err("Đang quét hoặc dọn – đợi xong rồi bỏ thư mục".into());
    }
    with_lib(&state.lib, move |lib| lib.remove_library_root(Path::new(&path))).await
}

/// Xem trước cân bằng ổ: thư mục nào chuyển từ ổ nào sang ổ nào. `from` + `want_gb`: chuyển bớt chừng ấy khỏi ổ đó;
/// bỏ trống thì chỉ xét ổ dưới mức trống tối thiểu.
#[tauri::command]
async fn balance_preview(
    state: State<'_, AppState>,
    from: Option<String>,
    want_gb: Option<f64>,
) -> CmdResult<volumes::BalancePlan> {
    with_lib(&state.lib, move |lib| balance_plan_for(lib, from.as_deref(), want_gb)).await
}

fn balance_plan_for(
    lib: &Library,
    from: Option<&str>,
    want_gb: Option<f64>,
) -> librarian_core::Result<volumes::BalancePlan> {
    let want = want_gb.filter(|g| *g > 0.0).map(|g| (g * 1024.0 * 1024.0 * 1024.0) as u64);
    volumes::balance_plan(&lib.conn, &lib.config, from.map(Path::new), want)
}

/// Cân bằng ổ ở nền (chép sang ổ khác có thể lâu): tính lại kế hoạch rồi chuyển, cùng hộp tiến trình / giám sát treo /
/// nút Dừng như dọn trùng (`clean-progress` bước `move`, `clean-done` mang `MoveReport`). Hoàn tác được.
#[tauri::command]
fn start_balance(
    app: AppHandle,
    state: State<'_, AppState>,
    from: Option<String>,
    want_gb: Option<f64>,
) -> CmdResult<()> {
    spawn_clean(app, state, move |lib, cancel, on| {
        let plan = balance_plan_for(lib, from.as_deref(), want_gb)?;
        volumes::apply_balance(&lib.conn, &lib.config, &plan, Some(cancel), Some(on))
    })
}

// ------------------------------------------------------------------ trùng lặp

#[tauri::command]
async fn exact_groups(state: State<'_, AppState>) -> CmdResult<Vec<ExactGroup>> {
    with_lib(&state.lib, |lib| {
        let groups = dedupe::exact_groups(&lib.conn)?;
        // Badge đọc số từ báo cáo quét: đồng bộ với danh sách thật mỗi lần mở màn này.
        report::sync_exact(&lib.conn, &groups)?;
        Ok(groups)
    })
    .await
}

#[derive(Clone, Serialize)]
struct CleanDone {
    ok: bool,
    message: Option<String>,
    /// Báo cáo của việc vừa chạy (`TrashReport` khi dọn / chặn, `MoveReport` khi cân bằng ổ).
    report: Option<serde_json::Value>,
}

/// Dọn các nhóm trùng chắc chắn đã chọn – chạy nền (luồng + kết nối DB riêng) vì có thể hàng nghìn file trên
/// ổ chậm. Giao diện nhận `clean-progress` (bước `trash` x / y, rồi `refresh`), `clean-health` mỗi giây (giám sát
/// treo 60 giây như khi quét) và đúng một `clean-done`. Dừng bằng `cancel_clean` hoặc khi đóng app.
#[tauri::command]
fn start_clean_exact(
    app: AppHandle,
    state: State<'_, AppState>,
    hashes: Vec<String>,
    keep: HashMap<String, i64>,
) -> CmdResult<()> {
    spawn_clean(app, state, move |lib, cancel, on| {
        let groups: Vec<ExactGroup> =
            dedupe::exact_groups(&lib.conn)?.into_iter().filter(|g| hashes.contains(&g.full_hash)).collect();
        dedupe::clean_exact_with(&lib.conn, &lib.config, &SystemTrasher, &groups, &keep, Some(cancel), Some(on))
    })
}

/// Tự dọn nghi trùng có độ tin cậy ≥ `dedupe::AUTO_SUSPECT_CONFIDENCE`: tính lại nhóm (bước `suspect_hash`) rồi
/// giữ bản đề xuất, đưa bản còn lại vào Thùng rác (bước `trash`). Cùng sự kiện / giám sát treo / dừng như
/// [`start_clean_exact`].
#[tauri::command]
fn start_clean_suspects(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    spawn_clean(app, state, move |lib, cancel, on| {
        let threads = lib.config.hash_threads.max(1);
        let Some(groups) = dedupe::suspect_groups_with(&lib.conn, threads, Some(cancel), Some(on))? else {
            return Ok(TrashReport { cancelled: true, ..Default::default() });
        };
        dedupe::clean_suspects_with(
            &lib.conn,
            &lib.config,
            &SystemTrasher,
            &groups,
            dedupe::AUTO_SUSPECT_CONFIDENCE,
            Some(cancel),
            Some(on),
        )
    })
}

/// Chạy một việc dọn ở nền (luồng + kết nối DB riêng, cùng khoá với quét), phát `clean-progress` / `clean-health`
/// / đúng một `clean-done`; dừng bằng `cancel_clean`.
fn spawn_clean<F, R>(app: AppHandle, state: State<'_, AppState>, job: F) -> CmdResult<()>
where
    F: FnOnce(&Library, &AtomicBool, &dyn Fn(Step)) -> librarian_core::Result<R> + Send + 'static,
    R: Serialize + 'static,
{
    if is_scanning(&state) {
        return Err("Đang quét thư viện – đợi quét xong rồi dọn".into());
    }
    let mut slot = state.clean_cancel.lock().map_err(err)?;
    if slot.is_some() {
        return Err("Đang dọn trùng".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *slot = Some(cancel.clone());
    drop(slot);
    let home = state.home.clone();
    let slot_ref = state.clean_cancel.clone();
    std::thread::spawn(move || {
        let reported = Arc::new(AtomicBool::new(false));
        let finish_ui = {
            let (app, slot_ref, cancel, reported) = (app.clone(), slot_ref.clone(), cancel.clone(), reported.clone());
            move |ev: CleanDone| {
                if reported.swap(true, Ordering::SeqCst) {
                    return;
                }
                if let Ok(mut s) = slot_ref.lock() {
                    if s.as_ref().is_some_and(|c| Arc::ptr_eq(c, &cancel)) {
                        *s = None;
                    }
                }
                let _ = app.emit("clean-done", ev);
            }
        };
        let health_app = app.clone();
        let on_stall = {
            let finish_ui = finish_ui.clone();
            move |stall: &progress::Stall| {
                finish_ui(CleanDone { ok: false, message: Some(stall.to_string()), report: None });
            }
        };
        let watchdog = Watchdog::start_with(
            cancel.clone(),
            progress::STALL_LIMIT,
            move |h| {
                let _ = health_app.emit("clean-health", h);
            },
            on_stall,
        );
        let res = (|| -> librarian_core::Result<R> {
            // Cùng khoá với quét (app + tác vụ quét ngầm): không dọn khi một tiến trình khác đang quét.
            let _lock = lock::ScanLock::try_acquire(&home)?;
            let lib = Library::open(&home)?;
            let emit_step = |s: Step| {
                watchdog.observe(s);
                let _ = app.emit("clean-progress", s);
            };
            let rep = job(&lib, &cancel, &emit_step)?;
            // Badge / banner: tính lại số trùng, nghi trùng… (kể cả khi đã dừng giữa chừng).
            emit_step(Step { phase: "refresh", done: 0, total: 0 });
            if let Err(e) = report::refresh_counts(&lib.conn, &lib.config) {
                tracing::warn!("cập nhật số liệu báo cáo lỗi: {e}");
            }
            Ok(rep)
        })();
        let stall = watchdog.finish();
        let ev = match (res, stall) {
            (_, Some(stall)) => CleanDone { ok: false, message: Some(stall.to_string()), report: None },
            (Ok(r), None) => CleanDone { ok: true, message: None, report: serde_json::to_value(r).ok() },
            (Err(librarian_core::Error::Locked), None) => CleanDone {
                ok: false,
                message: Some("Tác vụ quét ngầm đang chạy – thử lại sau ít phút".into()),
                report: None,
            },
            (Err(e), None) => CleanDone { ok: false, message: Some(e.to_string()), report: None },
        };
        finish_ui(ev);
    });
    Ok(())
}

#[tauri::command]
fn cancel_clean(state: State<'_, AppState>) {
    if let Ok(g) = state.clean_cancel.lock() {
        if let Some(c) = g.as_ref() {
            c.store(true, Ordering::Relaxed);
        }
    }
}

#[tauri::command]
async fn suspect_groups(state: State<'_, AppState>) -> CmdResult<Vec<SuspectGroup>> {
    with_lib(&state.lib, |lib| {
        let groups = dedupe::suspect_groups(&lib.conn)?;
        report::sync_suspects(&lib.conn, groups.len())?;
        Ok(groups)
    })
    .await
}

#[tauri::command]
async fn decide(state: State<'_, AppState>, ids: Vec<i64>, decision: String) -> CmdResult<usize> {
    let d = match decision.as_str() {
        "keep_both" => Decision::KeepBoth,
        "not_duplicate" => Decision::NotDuplicate,
        _ => return Err("quyết định không hợp lệ".into()),
    };
    with_lib(&state.lib, move |lib| refreshed(lib, |lib| dedupe::decide(&lib.conn, &ids, d))).await
}

// ------------------------------------------------------------------ phân loại

#[derive(Serialize)]
struct Target {
    label: String,
    path: String,
}

#[derive(Serialize)]
struct ClassifyView {
    proposals: Vec<Proposal>,
    targets: Vec<Target>,
}

#[tauri::command]
async fn propose(state: State<'_, AppState>) -> CmdResult<ClassifyView> {
    with_lib(&state.lib, |lib| {
        let mut targets = Vec::new();
        for c in classify::list_categories(&lib.config)? {
            targets.push(Target { label: c.name.clone(), path: c.path.to_string_lossy().into_owned() });
            for s in &c.series_dirs {
                targets.push(Target {
                    label: format!("{} › {}", c.name, s.name),
                    path: s.path.to_string_lossy().into_owned(),
                });
            }
        }
        Ok(ClassifyView { proposals: classify::propose(&lib.conn, &lib.config)?, targets })
    })
    .await
}

#[derive(Deserialize)]
struct MoveIn {
    book_id: i64,
    target_dir: String,
}

#[tauri::command]
async fn apply_moves(state: State<'_, AppState>, moves: Vec<MoveIn>, kind: Option<String>) -> CmdResult<MoveReport> {
    with_lib(&state.lib, move |lib| {
        let reqs: Vec<MoveRequest> = moves
            .into_iter()
            .map(|m| MoveRequest { book_id: m.book_id, target_dir: PathBuf::from(m.target_dir) })
            .collect();
        refreshed(lib, |lib| moves::apply_moves(&lib.conn, &lib.config, &reqs, kind.as_deref().unwrap_or("classify")))
    })
    .await
}

#[tauri::command]
async fn undo_batch(state: State<'_, AppState>, id: i64) -> CmdResult<UndoReport> {
    with_lib(&state.lib, move |lib| refreshed(lib, |lib| moves::undo_batch(&lib.conn, &lib.config, id))).await
}

#[tauri::command]
async fn list_batches(state: State<'_, AppState>) -> CmdResult<Vec<BatchInfo>> {
    with_lib(&state.lib, |lib| moves::list_batches(&lib.conn, 200)).await
}

#[tauri::command]
async fn batch_ops(state: State<'_, AppState>, id: i64) -> CmdResult<Vec<Operation>> {
    with_lib(&state.lib, move |lib| moves::batch_operations(&lib.conn, id)).await
}

#[tauri::command]
async fn evaluate(state: State<'_, AppState>) -> CmdResult<classify::Evaluation> {
    with_lib(&state.lib, |lib| classify::evaluate(&lib.conn, &lib.config)).await
}

#[tauri::command]
async fn report_history(state: State<'_, AppState>) -> CmdResult<Vec<report::Report>> {
    with_lib(&state.lib, |lib| report::history(&lib.conn, 30)).await
}

// ------------------------------------------------------------------ ảnh bìa

type CoverJob = (i64, UriSchemeResponder);

fn cover_response(status: u16, ctype: &str, body: Vec<u8>) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(status)
        .header("Content-Type", ctype)
        .header("Cache-Control", "max-age=86400")
        .header("Access-Control-Allow-Origin", "*")
        .body(body)
        .unwrap()
}

/// 4 luồng sinh thumbnail; mỗi luồng có kết nối DB chỉ đọc riêng.
fn start_cover_workers(home: &Path) -> mpsc::Sender<CoverJob> {
    let (tx, rx) = mpsc::channel::<CoverJob>();
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..4 {
        let rx = rx.clone();
        let home = home.to_path_buf();
        std::thread::spawn(move || {
            let cache = CoverCache::new(&home);
            let conn = db::open(&home.join("library.db")).ok();
            loop {
                let job = { rx.lock().ok().and_then(|r| r.recv().ok()) };
                let Some((id, responder)) = job else { break };
                let found = conn.as_ref().and_then(|c| db::get_book(c, id).ok().flatten());
                let resp = match found {
                    Some(b) => {
                        let fmt = Format::parse(&b.format);
                        match fmt.map(|f| cache.get(b.id, Path::new(&b.path), f, b.size, b.mtime)) {
                            Some(Ok(Some(jpg))) => cover_response(200, "image/jpeg", jpg),
                            _ => cover_response(404, "text/plain", b"no cover".to_vec()),
                        }
                    }
                    None => cover_response(404, "text/plain", b"no book".to_vec()),
                };
                responder.respond(resp);
            }
        });
    }
    tx
}

// ------------------------------------------------------------------ giao thức book:// (trình đọc)

type BookJob = (tauri::http::Request<Vec<u8>>, UriSchemeResponder);
type HttpBuilder = tauri::http::response::Builder;

fn http_response(status: u16, ctype: &str) -> HttpBuilder {
    tauri::http::Response::builder()
        .status(status)
        .header("Content-Type", ctype)
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Headers", "Range")
        .header("Access-Control-Expose-Headers", "Content-Range, Content-Length, Accept-Ranges")
        .header("Cache-Control", "no-store")
}

fn respond(responder: UriSchemeResponder, b: HttpBuilder, body: Vec<u8>) {
    responder.respond(b.body(body).unwrap_or_else(|_| tauri::http::Response::new(Vec::new())));
}

fn text(responder: UriSchemeResponder, status: u16, msg: &str) {
    respond(responder, http_response(status, "text/plain; charset=utf-8"), msg.as_bytes().to_vec());
}

fn book_mime(format: &str) -> &'static str {
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
        let n: u64 = b.parse().ok()?;
        (len.saturating_sub(n), last)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() { last } else { b.parse::<u64>().ok()?.min(last) };
        (start, end)
    };
    (start <= end).then_some((start, end))
}

/// Mỗi lần đọc Range tối đa 32 MB (pdf.js xin từng khúc nhỏ).
const MAX_RANGE: u64 = 32 * 1024 * 1024;

/// Phục vụ file sách, có Range. `extra`: phần nối thêm sau file gốc (bảng xref đã sửa của PDF hỏng).
fn serve_file(responder: UriSchemeResponder, path: &Path, format: &str, range: Option<&str>, extra: Option<&[u8]>) {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return text(responder, 404, &format!("Không mở được file: {e}")),
    };
    let file_len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let extra = extra.unwrap_or(&[]);
    let len = file_len + extra.len() as u64;
    let mime = book_mime(format);
    match range {
        Some(r) => {
            let Some((start, end)) = parse_range(r, len) else {
                let b = http_response(416, "text/plain").header("Content-Range", format!("bytes */{len}"));
                return respond(responder, b, Vec::new());
            };
            let end = end.min(start + MAX_RANGE - 1);
            let mut buf = vec![0u8; (end - start + 1) as usize];
            // Phần trong file gốc, rồi phần trong đuôi nối thêm.
            let in_file = if start < file_len { ((end + 1).min(file_len) - start) as usize } else { 0 };
            if in_file > 0 {
                if let Err(e) = f.seek(SeekFrom::Start(start)).and_then(|_| f.read_exact(&mut buf[..in_file])) {
                    return text(responder, 500, &format!("Lỗi đọc file: {e}"));
                }
            }
            if in_file < buf.len() {
                let from = (start + in_file as u64 - file_len) as usize;
                let n = buf.len() - in_file;
                buf[in_file..].copy_from_slice(&extra[from..from + n]);
            }
            let b = http_response(206, mime)
                .header("Accept-Ranges", "bytes")
                .header("Content-Range", format!("bytes {start}-{end}/{len}"));
            respond(responder, b, buf);
        }
        None => {
            let mut buf = Vec::with_capacity(len as usize);
            if let Err(e) = f.read_to_end(&mut buf) {
                return text(responder, 500, &format!("Lỗi đọc file: {e}"));
            }
            buf.extend_from_slice(extra);
            respond(responder, http_response(200, mime).header("Accept-Ranges", "bytes"), buf);
        }
    }
}

/// Bảng xref đã sửa cho PDF hỏng (xem `librarian_core::pdf_repair`), tính một lần cho mỗi file
/// (theo đường dẫn + dung lượng + mtime) và lưu xuống `cache/pdffix` để lần mở sau dùng ngay.
type FixCell = std::sync::OnceLock<Option<Arc<Vec<u8>>>>;

struct PdfFixes {
    dir: PathBuf,
    memo: Mutex<HashMap<String, Arc<FixCell>>>,
}

impl PdfFixes {
    fn new(dir: PathBuf) -> Self {
        Self { dir, memo: Mutex::default() }
    }

    fn get(&self, b: &db::Book) -> Option<Arc<Vec<u8>>> {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b.path.as_bytes());
        h.update(b.size.to_le_bytes());
        h.update(b.mtime.to_le_bytes());
        let key: String = h.finalize().iter().take(12).map(|x| format!("{x:02x}")).collect();
        let cell = self.memo.lock().ok()?.entry(key.clone()).or_default().clone();
        // OnceLock: các luồng xin cùng file cùng lúc chỉ tính một lần, luồng khác đợi.
        cell.get_or_init(|| {
            let file = self.dir.join(format!("{key}.xref"));
            if let Ok(t) = std::fs::read(&file) {
                return Some(Arc::new(t));
            }
            let t0 = std::time::Instant::now();
            match librarian_core::pdf_repair::repair_tail(Path::new(&b.path)) {
                Ok(Some(t)) => {
                    tracing::info!("sửa xref PDF hỏng {} ({} B, {:?})", b.path, t.len(), t0.elapsed());
                    let _ = std::fs::create_dir_all(&self.dir);
                    let _ = std::fs::write(&file, &t);
                    Some(Arc::new(t))
                }
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!("kiểm tra xref {}: {e}", b.path);
                    None
                }
            }
        })
        .clone()
    }
}

type OpenComic = (i64, i64, Arc<ComicSource>);

/// Các cuốn truyện đang mở (giữ vài cuốn gần nhất để lật trang không phải mở lại zip/RAR).
#[derive(Default)]
struct ComicCache {
    open: Mutex<Vec<OpenComic>>,
    /// Mỗi lúc chỉ mở/giải nén một cuốn (tránh giải nén cùng một RAR hai lần).
    opening: Mutex<()>,
}

impl ComicCache {
    fn find(&self, b: &db::Book) -> Option<Arc<ComicSource>> {
        let v = self.open.lock().ok()?;
        v.iter().find(|(id, m, _)| *id == b.id && *m == b.mtime).map(|x| x.2.clone())
    }

    fn get(&self, b: &db::Book, rar_dir: &Path) -> librarian_core::Result<Arc<ComicSource>> {
        if let Some(s) = self.find(b) {
            return Ok(s);
        }
        let _g = self.opening.lock().map_err(|_| librarian_core::Error::msg("khoá hỏng"))?;
        if let Some(s) = self.find(b) {
            return Ok(s);
        }
        let src = Arc::new(reader::open_comic(Path::new(&b.path), b.size, b.mtime, rar_dir)?);
        if let Ok(mut v) = self.open.lock() {
            v.retain(|(id, _, _)| *id != b.id);
            v.insert(0, (b.id, b.mtime, src.clone()));
            v.truncate(6);
        }
        Ok(src)
    }
}

/// `http://book.localhost/<id>/file` (có Range), `/<id>/pages` (JSON số trang), `/<id>/page/<n>` (ảnh).
fn handle_book(
    conn: Option<&librarian_core::rusqlite::Connection>,
    external: &External,
    comics: &ComicCache,
    pdffix: &PdfFixes,
    rar_dir: &Path,
    job: BookJob,
) {
    let (req, responder) = job;
    if req.method() == tauri::http::Method::OPTIONS {
        return respond(responder, http_response(204, "text/plain"), Vec::new());
    }
    let parts: Vec<&str> = req.uri().path().trim_matches('/').split('/').collect();
    let Some(id) = parts.first().and_then(|s| s.parse::<i64>().ok()) else {
        return text(responder, 400, "bad id");
    };
    let found = if id < 0 {
        external_path(external, id).and_then(|p| reader::external_book(id, &p).ok())
    } else {
        conn.and_then(|c| db::get_book(c, id).ok().flatten())
    };
    let Some(b) = found else {
        return text(responder, 404, "Không có sách này");
    };
    match parts.get(1).copied() {
        Some("file") => {
            let range = req.headers().get("range").and_then(|v| v.to_str().ok()).map(str::to_string);
            let fix = if b.format == "pdf" { pdffix.get(&b) } else { None };
            serve_file(
                responder,
                Path::new(&b.path),
                &b.format,
                range.as_deref(),
                fix.as_deref().map(|v| v.as_slice()),
            );
        }
        Some("pages") => match comics.get(&b, rar_dir) {
            Ok(src) => {
                let body = format!("{{\"count\":{}}}", src.page_count()).into_bytes();
                respond(responder, http_response(200, "application/json"), body);
            }
            Err(e) => text(responder, 422, &e.to_string()),
        },
        Some("page") => {
            let Some(n) = parts.get(2).and_then(|s| s.parse::<usize>().ok()) else {
                return text(responder, 400, "bad page");
            };
            match comics.get(&b, rar_dir).and_then(|s| s.page(n)) {
                Ok((bytes, mime)) => {
                    respond(responder, http_response(200, mime).header("Cache-Control", "max-age=3600"), bytes)
                }
                Err(e) => text(responder, 404, &e.to_string()),
            }
        }
        _ => text(responder, 400, "bad path"),
    }
}

/// 4 luồng phục vụ `book://`; mỗi luồng có kết nối DB riêng, dùng chung cache truyện đang mở.
fn start_book_workers(home: &Path, external: External) -> mpsc::Sender<BookJob> {
    let (tx, rx) = mpsc::channel::<BookJob>();
    let rx = Arc::new(Mutex::new(rx));
    let comics = Arc::new(ComicCache::default());
    let rar_dir = home.join("cache").join("rar");
    let pdffix = Arc::new(PdfFixes::new(home.join("cache").join("pdffix")));
    for _ in 0..4 {
        let (rx, comics, rar_dir, home) = (rx.clone(), comics.clone(), rar_dir.clone(), home.to_path_buf());
        let pdffix = pdffix.clone();
        let external = external.clone();
        std::thread::spawn(move || {
            let conn = db::open(&home.join("library.db")).ok();
            loop {
                let job = { rx.lock().ok().and_then(|r| r.recv().ok()) };
                let Some(job) = job else { break };
                handle_book(conn.as_ref(), &external, &comics, &pdffix, &rar_dir, job);
            }
        });
    }
    tx
}

// ------------------------------------------------------------------ đồng bộ sang iPad (I3)

#[derive(Serialize)]
struct IpadState {
    running: bool,
    port: u16,
    addrs: Vec<String>,
    /// Mỗi thiết bị (iPad/iPhone) có hàng chờ riêng.
    devices: Vec<DeviceView>,
    /// Id sách đang có trên ít nhất một thiết bị.
    on_device: Vec<i64>,
}

#[derive(Serialize)]
struct DeviceView {
    #[serde(flatten)]
    device: librarian_core::sync::Device,
    queue: Vec<librarian_core::sync_proto::OutboxItem>,
    queue_bytes: i64,
    /// Số cuốn đang có trên thiết bị (báo cáo gần nhất).
    books: usize,
}

/// Địa chỉ IPv4 nội bộ chính của máy (UDP "connect" không gửi gói nào, chỉ để hệ điều hành chọn card mạng).
fn local_addrs() -> Vec<String> {
    let probe = || -> Option<String> {
        let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        s.connect("192.0.2.1:9").ok()?;
        let ip = s.local_addr().ok()?.ip();
        (!ip.is_unspecified() && librarian_core::sync::is_local_ip(ip)).then(|| ip.to_string())
    };
    let mut out: Vec<String> = probe().into_iter().collect();
    // Thêm mọi IPv4 riêng của các card mạng (Wi-Fi, Ethernet…): đường mặc định có thể là VPN/ảo
    // nên IP dò được không phải IP Wi-Fi mà điện thoại cần; máy khách thử lần lượt từng địa chỉ.
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in ifs {
            if let std::net::IpAddr::V4(v) = i.ip() {
                if v.is_private() && !i.is_loopback() {
                    let s = v.to_string();
                    if !out.contains(&s) {
                        out.push(s);
                    }
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------ tường lửa / loại mạng (I3-07)
// Wi-Fi bị Windows xếp là mạng Công cộng thì gói tin từ iPad/điện thoại tới cổng 47470 bị chặn,
// kể cả khi app đã có luật "Cho phép". Màn iPad cảnh báo và có nút sửa (một lần UAC):
// chuyển card mạng sang Riêng tư + thêm luật mở cổng 47470 chỉ cho mạng nội bộ.

/// Tên luật tường lửa app tự thêm (bộ gỡ cài xoá theo đúng tên này – xem installer-hooks.nsh).
const FIREWALL_RULE: &str = "My Librarian sync (TCP 47470)";

#[derive(Serialize, serde::Deserialize, Clone)]
struct PublicNet {
    /// Tên card mạng (vd. "Wi-Fi").
    alias: String,
    /// Tên mạng (vd. tên Wi-Fi).
    name: String,
    index: u32,
}

#[derive(Serialize, Default)]
struct NetCheck {
    /// Các card mạng có IP nội bộ đang bị xếp là mạng Công cộng.
    public: Vec<PublicNet>,
    /// Đã có luật mở cổng 47470 của app.
    rule: bool,
}

/// Base64 của chuỗi UTF-16LE – dạng `powershell -EncodedCommand` cần, khỏi lo trích dẫn.
#[cfg(windows)]
fn ps_encode(script: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Chạy PowerShell ẩn (không nháy cửa sổ), trả về stdout UTF-8.
#[cfg(windows)]
fn run_ps(script: &str) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8\n{script}");
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &ps_encode(&script)])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(err)?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(windows)]
fn net_check() -> Result<NetCheck, String> {
    #[derive(serde::Deserialize)]
    struct Row {
        alias: String,
        name: Option<String>,
        index: u32,
        ips: Vec<String>,
    }
    #[derive(serde::Deserialize)]
    struct Out {
        public: Vec<Row>,
        rule: bool,
    }
    let script = format!(
        r#"$p = @(Get-NetConnectionProfile -NetworkCategory Public -ErrorAction SilentlyContinue | ForEach-Object {{
  [pscustomobject]@{{ alias = $_.InterfaceAlias; name = $_.Name; index = [int]$_.InterfaceIndex;
    ips = @((Get-NetIPAddress -InterfaceIndex $_.InterfaceIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue).IPAddress) }} }})
$r = [bool](Get-NetFirewallRule -DisplayName '{FIREWALL_RULE}' -ErrorAction SilentlyContinue)
ConvertTo-Json -Compress -Depth 4 -InputObject ([pscustomobject]@{{ public = $p; rule = $r }})"#
    );
    let out: Out = serde_json::from_str(&run_ps(&script)?).map_err(err)?;
    let private_v4 = |s: &String| s.parse::<std::net::Ipv4Addr>().is_ok_and(|v| v.is_private());
    let public = out
        .public
        .into_iter()
        .filter(|r| r.ips.iter().any(private_v4)) // bỏ VPN / card ảo không có IP nội bộ
        .map(|r| PublicNet { name: r.name.unwrap_or_else(|| r.alias.clone()), alias: r.alias, index: r.index })
        .collect();
    Ok(NetCheck { public, rule: out.rule })
}

/// Loại mạng của các card có IP nội bộ + đã có luật cổng 47470 chưa. Chỉ Windows; nơi khác trả rỗng.
#[tauri::command]
async fn ipad_net_check() -> CmdResult<NetCheck> {
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(net_check).await.map_err(err)?;
    #[cfg(not(windows))]
    Ok(NetCheck::default())
}

/// Chuyển các card trong `nets` sang mạng Riêng tư và thêm luật mở cổng 47470 (chỉ mạng nội bộ,
/// hồ sơ Riêng tư). Cần quyền Admin nên Windows hiện hộp UAC; người dùng bấm Không thì báo lỗi.
#[tauri::command]
async fn ipad_net_fix(nets: Vec<u32>) -> CmdResult<NetCheck> {
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(move || {
        let idx = nets.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let port = librarian_core::sync_proto::DEFAULT_PORT;
        let inner = format!(
            r#"foreach ($i in @({idx})) {{ try {{ Set-NetConnectionProfile -InterfaceIndex $i -NetworkCategory Private -ErrorAction Stop }} catch {{ }} }}
if (-not (Get-NetFirewallRule -DisplayName '{FIREWALL_RULE}' -ErrorAction SilentlyContinue)) {{
  New-NetFirewallRule -DisplayName '{FIREWALL_RULE}' -Direction Inbound -Protocol TCP -LocalPort {port} -RemoteAddress LocalSubnet -Action Allow -Profile Private | Out-Null }}"#
        );
        let outer = format!(
            "try {{ $p = Start-Process powershell -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile','-NonInteractive','-EncodedCommand','{}'; 'ok' }} catch {{ 'cancel' }}",
            ps_encode(&inner)
        );
        if run_ps(&outer)? == "cancel" {
            return Err("Chưa sửa: cần cho phép quyền quản trị (UAC) để đổi loại mạng và mở cổng tường lửa.".to_string());
        }
        net_check()
    })
    .await
    .map_err(err)?;
    #[cfg(not(windows))]
    {
        let _ = nets;
        Ok(NetCheck::default())
    }
}

fn computer_name() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Máy tính".into())
}

#[tauri::command]
async fn ipad_state(state: State<'_, AppState>) -> CmdResult<IpadState> {
    let (running, port) = {
        let g = state.sync.lock().map_err(|_| "khoá hỏng".to_string())?;
        (g.is_some(), g.as_ref().map(|s| s.port()).unwrap_or(librarian_core::sync_proto::DEFAULT_PORT))
    };
    with_lib(&state.lib, move |lib| {
        use librarian_core::sync;
        let mut devices = Vec::new();
        for device in sync::list_devices(&lib.conn)? {
            let queue = sync::queue_list(&lib.conn, device.id)?;
            let books = sync::device_books(&lib.conn, device.id)?.len();
            devices.push(DeviceView { queue_bytes: queue.iter().map(|i| i.size).sum(), queue, books, device });
        }
        Ok(IpadState { running, port, addrs: local_addrs(), on_device: sync::on_device_ids(&lib.conn)?, devices })
    })
    .await
}

#[tauri::command]
async fn ipad_enable(state: State<'_, AppState>, on: bool) -> CmdResult<()> {
    let home = state.home.clone();
    let mut g = state.sync.lock().map_err(|_| "khoá hỏng".to_string())?;
    if on && g.is_none() {
        let bind = format!("0.0.0.0:{}", librarian_core::sync_proto::DEFAULT_PORT);
        *g = Some(librarian_core::sync::SyncServer::start(&home, &bind, &computer_name()).map_err(err)?);
    } else if !on {
        *g = None; // Drop tắt máy chủ
    }
    Ok(())
}

#[derive(Serialize)]
struct PairInfo {
    svg: String,
    code: String,
    addrs: Vec<String>,
    port: u16,
    /// Còn bao nhiêu ms thì QR này được đổi mã.
    remaining_ms: u64,
}

#[tauri::command]
async fn ipad_pair_qr(state: State<'_, AppState>) -> CmdResult<PairInfo> {
    let g = state.sync.lock().map_err(|_| "khoá hỏng".to_string())?;
    let srv = g.as_ref().ok_or("Chưa bật kết nối iPad")?;
    // QR giữ nguyên tối thiểu 60 giây (mở lại vẫn thấy mã cũ); hết hạn hoặc đã có máy ghép mới đổi.
    let (code, remaining_ms) = srv.pair_code_for(std::time::Duration::from_secs(60));
    let addrs = local_addrs();
    let qr = srv.pair_qr(&code, addrs.clone()).map_err(err)?;
    let payload = serde_json::to_string(&qr).map_err(err)?;
    let svg = qrcode::QrCode::new(payload.as_bytes())
        .map_err(err)?
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build();
    Ok(PairInfo { svg, code, addrs, port: srv.port(), remaining_ms })
}

#[tauri::command]
async fn ipad_queue_add(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    devices: Vec<i64>,
) -> CmdResult<librarian_core::sync::QueueResult> {
    let home = state.home.clone();
    let res = with_lib(&state.lib, move |lib| librarian_core::sync::queue_add(&lib.conn, &ids, &devices)).await?;
    if res.added > 0 {
        // Tính SHA-256 nền để iPad kiểm tra file sau khi tải (I3-04).
        std::thread::spawn(move || {
            if let Ok(conn) = db::open(&home.join("library.db")) {
                let _ = librarian_core::sync::ensure_outbox_hashes(&conn, None);
            }
        });
    }
    Ok(res)
}

#[tauri::command]
async fn ipad_queue_remove(state: State<'_, AppState>, ids: Vec<i64>, device: Option<i64>) -> CmdResult<usize> {
    with_lib(&state.lib, move |lib| librarian_core::sync::queue_remove(&lib.conn, &ids, device)).await
}

/// Danh sách thiết bị đã ghép (cho menu "Gửi sang …").
#[tauri::command]
async fn ipad_devices(state: State<'_, AppState>) -> CmdResult<Vec<librarian_core::sync::Device>> {
    with_lib(&state.lib, move |lib| librarian_core::sync::list_devices(&lib.conn)).await
}

#[tauri::command]
async fn ipad_rename_device(state: State<'_, AppState>, id: i64, name: String) -> CmdResult<String> {
    with_lib(&state.lib, move |lib| librarian_core::sync::rename_device(&lib.conn, id, &name)).await
}

/// Sách đang có trên một thiết bị.
#[tauri::command]
async fn ipad_device_books(
    state: State<'_, AppState>,
    id: i64,
) -> CmdResult<Vec<librarian_core::sync_proto::OutboxItem>> {
    with_lib(&state.lib, move |lib| librarian_core::sync::device_books(&lib.conn, id)).await
}

/// Tên các thiết bị đang có cuốn này.
#[tauri::command]
async fn book_devices(state: State<'_, AppState>, id: i64) -> CmdResult<Vec<String>> {
    with_lib(&state.lib, move |lib| librarian_core::sync::devices_with_book(&lib.conn, id)).await
}

#[tauri::command]
async fn ipad_remove_device(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    with_lib(&state.lib, move |lib| librarian_core::sync::remove_device(&lib.conn, id)).await
}

/// Thư mục dữ liệu WebView2 (cache, localStorage). Bản dùng `%APPDATA%` giữ mặc định của Tauri
/// (`%LOCALAPPDATA%\<identifier>`); bản dữ liệu theo thư mục cài thì để ở `<home>\webview` và chuyển bản cũ sang (một lần).
fn webview_data_dir(home: &Path) -> Option<PathBuf> {
    if !config::is_install_home(home) {
        return None;
    }
    let dir = home.join("webview");
    let Some(local) = std::env::var_os("LOCALAPPDATA") else { return Some(dir) };
    let old_root = PathBuf::from(local).join("vn.hungtrieu.mylibrarian");
    let old = old_root.join("EBWebView");
    if old.is_dir() && !dir.join("EBWebView").exists() {
        let _ = config::copy_tree(&old, &dir.join("EBWebView"));
    }
    // Đã có bản ở thư mục cài thì xoá bản cũ; lần trước sót (file bị khoá) thì lần mở sau xoá tiếp.
    if old_root.is_dir() && dir.join("EBWebView").is_dir() {
        let _ = std::fs::remove_dir_all(&old_root);
    }
    Some(dir)
}

fn main() {
    let home = config::default_home();
    let moved = config::migrate_legacy_home(&home);
    let webview_dir = webview_data_dir(&home);
    let _guard = librarian_core::logging::init(&home, cfg!(debug_assertions));
    match moved {
        Ok(Some(old)) => tracing::info!("đã chuyển dữ liệu từ {} sang {}", old.display(), home.display()),
        Ok(None) => {}
        Err(e) => tracing::warn!("chuyển dữ liệu cũ sang {} lỗi: {e}", home.display()),
    }
    let lib = Library::open_recovering(&home).expect("không mở được dữ liệu thư viện");
    // Sao lưu DB mỗi ngày (E10-06) trên luồng riêng, kết nối riêng.
    {
        let home = home.clone();
        std::thread::spawn(move || {
            if let Ok(conn) = db::open(&home.join("library.db")) {
                match librarian_core::backup::backup_if_due(&conn, &home, librarian_core::backup::DAILY) {
                    Ok(Some(p)) => tracing::info!("đã sao lưu DB: {}", p.display()),
                    Ok(None) => {}
                    Err(e) => tracing::warn!("sao lưu DB lỗi: {e}"),
                }
            }
        });
    }
    let covers = Mutex::new(start_cover_workers(&home));
    let external: External = Arc::default();
    let books = Mutex::new(start_book_workers(&home, external.clone()));
    let args: Vec<String> = std::env::args().collect();
    tauri::Builder::default()
        // Một cửa sổ duy nhất: mở thêm (vd. bấm đúp file sách trong Explorer) thì chuyển file cho cửa sổ đang chạy.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
            if let Some(p) = book_arg(&argv) {
                let _ = app.emit("open-file", p);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol("cover", move |_ctx, request, responder| {
            let id = request.uri().path().trim_matches('/').split('/').next().and_then(|s| s.parse::<i64>().ok());
            match id {
                Some(id) => {
                    if let Ok(tx) = covers.lock() {
                        let _ = tx.send((id, responder));
                    }
                }
                None => responder.respond(cover_response(400, "text/plain", b"bad id".to_vec())),
            }
        })
        .register_asynchronous_uri_scheme_protocol("book", move |_ctx, request, responder| {
            if let Ok(tx) = books.lock() {
                let _ = tx.send((request, responder));
            }
        })
        .manage(AppState {
            home: home.clone(),
            lib: Arc::new(Mutex::new(lib)),
            scan_cancel: Arc::new(Mutex::new(None)),
            clean_cancel: Arc::new(Mutex::new(None)),
            quitting: Arc::new(AtomicBool::new(false)),
            external,
            pending_open: Mutex::new(book_arg(&args)),
            sync: Mutex::new(None),
        })
        // Đóng cửa sổ khi đang quét / dọn trùng → hỏi lại trên giao diện (sự kiện `close-requested`, kèm loại việc)
        // thay vì cắt ngang.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                if let Some(kind) = busy_kind(&state).filter(|_| !state.quitting.load(Ordering::Relaxed)) {
                    api.prevent_close();
                    let _ = window.emit("close-requested", kind);
                }
            }
        })
        .setup(move |app| {
            // Cửa sổ tạo ở đây (tauri.conf: "create": false) để đặt thư mục dữ liệu WebView2 cạnh dữ liệu app.
            if let Some(cfg) = app.config().app.windows.iter().find(|w| w.label == "main").cloned() {
                let mut b = tauri::WebviewWindowBuilder::from_config(app.handle(), &cfg)?;
                if let Some(dir) = webview_dir.clone() {
                    b = b.data_directory(dir);
                }
                b.title("My Librarian").build()?;
            }
            // Số trùng / nghi trùng / chờ phân loại trong báo cáo có thể đã cũ (dọn ở bản trước, xoá file ngoài
            // app…): tính lại khi mở app – trên luồng riêng, kết nối riêng, SAU khi đã có cửa sổ. Tìm nghi trùng
            // có thể phải băm nhanh hàng nghìn file trên ổ chậm (kho 75k sách, ổ USB: ~4 phút); chạy trước khi
            // tạo cửa sổ thì app mở mà không hiện gì. Xong thì báo giao diện tải lại badge.
            {
                let home = home.clone();
                let app = app.handle().clone();
                std::thread::spawn(move || {
                    let t = std::time::Instant::now();
                    let r = Config::load(&home).and_then(|cfg| {
                        let conn = db::open(&home.join("library.db"))?;
                        report::refresh_counts(&conn, &cfg)
                    });
                    match r {
                        Ok(()) => {
                            tracing::info!("cập nhật số liệu báo cáo: {:?}", t.elapsed());
                            let _ = app.emit("stats-updated", ());
                        }
                        Err(e) => tracing::warn!("cập nhật số liệu báo cáo lỗi: {e}"),
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            overview,
            pick_folder,
            set_library,
            save_config,
            start_scan,
            cancel_scan,
            quit_app,
            query,
            book,
            set_read_status,
            set_not_worth,
            reveal,
            open_external,
            reader_info,
            save_position,
            take_open_request,
            open_path,
            toggle_fullscreen,
            open_recycle_bin,
            trash_books,
            retitle_books,
            category_tree,
            block_category,
            unblock_category,
            tag_list,
            tag_map_plan,
            tag_map_save,
            volumes_info,
            add_library_folder,
            remove_library_folder,
            balance_preview,
            start_balance,
            exact_groups,
            start_clean_exact,
            cancel_clean,
            suspect_groups,
            start_clean_suspects,
            decide,
            propose,
            apply_moves,
            undo_batch,
            list_batches,
            batch_ops,
            evaluate,
            report_history,
            ipad_state,
            ipad_enable,
            ipad_net_check,
            ipad_net_fix,
            ipad_pair_qr,
            ipad_queue_add,
            ipad_queue_remove,
            ipad_remove_device,
            ipad_devices,
            ipad_rename_device,
            ipad_device_books,
            book_devices
        ])
        .run(tauri::generate_context!())
        .expect("không khởi động được ứng dụng");
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=990-5000", 1000), Some((990, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=5-1", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
        assert_eq!(parse_range("bytes=0-0", 0), None);
    }
}

#[cfg(all(test, windows))]
mod net_tests {
    use super::*;

    #[test]
    fn ps_encode_is_utf16le_base64() {
        assert_eq!(ps_encode("a"), "YQA=");
        assert_eq!(ps_encode("ab"), "YQBiAA==");
        assert_eq!(ps_encode("abc"), "YQBiAGMA");
        assert_eq!(ps_encode("đ"), "EQE="); // U+0111
    }

    #[test]
    fn net_check_runs_on_this_machine() {
        let r = net_check().expect("PowerShell check");
        // Chỉ báo card có IP nội bộ; Tailscale (100.x) không bao giờ có mặt.
        assert!(r.public.iter().all(|n| !n.alias.eq_ignore_ascii_case("Tailscale")));
    }
}
