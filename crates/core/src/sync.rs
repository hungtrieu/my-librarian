//! Phía máy tính của đồng bộ sang iPad (I3): hàng chờ "Gửi sang iPad", ghép nối, và máy chủ HTTP
//! trong mạng nội bộ. Máy chủ **chỉ đọc** với ổ sách; chỉ ghi vào DB (hàng chờ, thiết bị).

use crate::config::Config;
use crate::covers::CoverCache;
use crate::db;
use crate::error::{Error, Result};
use crate::formats::Format;
use crate::hash;
use crate::sync_proto::*;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Mã ghép hết hạn sau 5 phút, dùng một lần.
const PAIR_TTL: Duration = Duration::from_secs(300);
/// Giới hạn kích thước body JSON của yêu cầu.
const MAX_BODY: u64 = 1 << 20;

// ---------------------------------------------------------------------------------------------
// Ngẫu nhiên, library_id, token
// ---------------------------------------------------------------------------------------------

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("nguồn ngẫu nhiên của hệ điều hành");
    b
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// UUID của thư viện này, sinh một lần (nhận dạng sách giữa hai máy – mục 2.2).
pub fn library_id(conn: &Connection) -> Result<String> {
    if let Some(v) = conn.query_row("SELECT value FROM meta WHERE key = 'library_id'", [], |r| r.get(0)).optional()? {
        return Ok(v);
    }
    let id = to_hex(&random_bytes::<16>());
    conn.execute("INSERT OR IGNORE INTO meta (key, value) VALUES ('library_id', ?1)", [&id])?;
    Ok(conn.query_row("SELECT value FROM meta WHERE key = 'library_id'", [], |r| r.get(0))?)
}

fn token_hash(token: &str) -> String {
    to_hex(&Sha256::digest(token.as_bytes()))
}

/// Mã ghép 6 số.
fn new_code() -> String {
    // Lấy mẫu từ u32 trong vùng chia hết cho 10^6 để không lệch.
    loop {
        let n = u32::from_le_bytes(random_bytes::<4>());
        if n < 4_294_000_000 {
            return format!("{:06}", n % 1_000_000);
        }
    }
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Chỉ nhận kết nối từ mạng nội bộ: loopback, 10/8, 172.16/12, 192.168/16, 169.254/16, IPv6 ULA/link-local.
pub fn is_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
        IpAddr::V6(v) => {
            if let Some(m) = v.to_ipv4_mapped() {
                return is_local_ip(IpAddr::V4(m));
            }
            let s = v.segments();
            v.is_loopback() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Hàng chờ (I3-03) và hash (I3-04)
// ---------------------------------------------------------------------------------------------

/// CBR / DJVU không đọc được trên iPad (DI8, DI9) nên không gửi.
pub fn sendable(format: &str) -> bool {
    !matches!(format, "cbr" | "djvu")
}

/// Thiết bị chỉ đọc sách chữ (iPhone) thì không nhận PDF/truyện tranh.
pub fn sendable_to(kind: &str, format: &str) -> bool {
    sendable(format) && (kind != "iphone" || crate::ipad_lib::is_text_format(format))
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct QueueResult {
    /// Số cặp (sách, thiết bị) đã xếp hàng.
    pub added: usize,
    /// Số cuốn (khác nhau) đã xếp cho ít nhất một thiết bị.
    pub books: usize,
    pub skipped_unsupported: usize,
    /// Cặp bị bỏ vì thiết bị không đọc định dạng đó (PDF/truyện tranh → iPhone).
    pub skipped_device: usize,
    pub skipped_missing: usize,
}

/// Thêm sách vào hàng chờ của các thiết bị `devices`. Cuốn đã gửi tới thiết bị đó trước đây được xếp lại hàng.
pub fn queue_add(conn: &Connection, ids: &[i64], devices: &[i64]) -> Result<QueueResult> {
    let mut res = QueueResult::default();
    let now = db::now();
    let tx = crate::db::write_tx(conn)?;
    let mut kinds = Vec::new();
    for &d in devices {
        let k: Option<String> = tx.query_row("SELECT kind FROM devices WHERE id = ?1", [d], |r| r.get(0)).optional()?;
        match k {
            Some(k) => kinds.push((d, k)),
            None => return Err(Error::msg("Thiết bị không còn trong danh sách đã ghép")),
        }
    }
    if kinds.is_empty() {
        return Err(Error::msg("Chưa chọn thiết bị nào"));
    }
    for &id in ids {
        let fmt: Option<(String, String)> = tx
            .query_row("SELECT format, status FROM books WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        match fmt {
            Some((f, st)) if st == "present" => {
                if !sendable(&f) {
                    res.skipped_unsupported += 1;
                    continue;
                }
                let mut any = false;
                for (d, kind) in &kinds {
                    if !sendable_to(kind, &f) {
                        res.skipped_device += 1;
                        continue;
                    }
                    tx.execute(
                        "INSERT INTO outbox (book_id, device_id, queued_at, sent_at) VALUES (?1, ?2, ?3, NULL)
                         ON CONFLICT(book_id, device_id) DO UPDATE SET queued_at = ?3, sent_at = NULL",
                        params![id, d, now],
                    )?;
                    res.added += 1;
                    any = true;
                }
                res.books += any as usize;
            }
            _ => res.skipped_missing += 1,
        }
    }
    tx.commit()?;
    Ok(res)
}

/// Bỏ sách khỏi hàng chờ (chưa gửi) của một thiết bị, hoặc của mọi thiết bị khi `device` là `None`.
pub fn queue_remove(conn: &Connection, ids: &[i64], device: Option<i64>) -> Result<usize> {
    let mut n = 0;
    for &id in ids {
        n += conn.execute(
            "DELETE FROM outbox WHERE book_id = ?1 AND sent_at IS NULL AND (?2 IS NULL OR device_id = ?2)",
            params![id, device],
        )?;
    }
    Ok(n)
}

const ITEM_COLS: &str = "b.id, b.title, b.authors, b.series, b.volume, b.format, b.size, b.full_hash, b.hash_size";

fn outbox_item(r: &rusqlite::Row) -> rusqlite::Result<OutboxItem> {
    let size: i64 = r.get(6)?;
    let hash: Option<String> = r.get(7)?;
    let hash_size: Option<i64> = r.get(8)?;
    Ok(OutboxItem {
        id: r.get(0)?,
        title: r.get(1)?,
        authors: r.get(2)?,
        series: r.get(3)?,
        volume: r.get(4)?,
        format: r.get(5)?,
        size,
        sha256: hash.filter(|_| hash_size == Some(size)),
    })
}

/// Hàng chờ đang đợi tải (chưa `ack`) của một thiết bị, xếp theo bộ truyện rồi số vol.
pub fn queue_list(conn: &Connection, device: i64) -> Result<Vec<OutboxItem>> {
    let mut st = conn.prepare(&format!(
        "SELECT {ITEM_COLS} FROM outbox o JOIN books b ON b.id = o.book_id
         WHERE o.device_id = ?1 AND o.sent_at IS NULL AND b.status = 'present'
         ORDER BY COALESCE(b.series, b.title) COLLATE NOCASE, b.volume, b.title COLLATE NOCASE"
    ))?;
    let rows = st.query_map([device], outbox_item)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Sách đang có trên một thiết bị (theo báo cáo gần nhất của nó + các cuốn đã gửi xong), mới gửi trước.
pub fn device_books(conn: &Connection, device: i64) -> Result<Vec<OutboxItem>> {
    let mut st = conn.prepare(&format!(
        "SELECT {ITEM_COLS} FROM books b
         LEFT JOIN outbox o ON o.book_id = b.id AND o.device_id = ?1
         WHERE b.id IN (SELECT value FROM devices, json_each(devices.on_device) WHERE devices.id = ?1)
         ORDER BY o.sent_at DESC, COALESCE(b.series, b.title) COLLATE NOCASE, b.volume"
    ))?;
    let rows = st.query_map([device], outbox_item)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Tên các thiết bị đang có cuốn `book` (để hiện "Có trên …").
pub fn devices_with_book(conn: &Connection, book: i64) -> Result<Vec<String>> {
    let mut st = conn
        .prepare("SELECT name FROM devices WHERE ?1 IN (SELECT value FROM json_each(devices.on_device)) ORDER BY id")?;
    let v = st.query_map([book], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
    Ok(v)
}

/// Id các cuốn đã gửi xong và đang có trên iPad (để hiện "Có trên iPad" – I3-06).
pub fn on_device_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut ids: Vec<i64> = Vec::new();
    let mut st = conn.prepare("SELECT on_device FROM devices")?;
    for j in st.query_map([], |r| r.get::<_, String>(0))? {
        if let Ok(v) = serde_json::from_str::<Vec<i64>>(&j?) {
            ids.extend(v);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

/// Gộp cờ "không đáng đọc" thiết bị gửi lên (`POST /api/v1/marks`): bản nào đổi sau thắng. Mốc thời gian
/// của thiết bị bị kẹp ở "bây giờ" để đồng hồ thiết bị chạy nhanh không thắng mãi. Trả trạng thái cuối của
/// đúng các cuốn được hỏi (cuốn không còn trong thư viện bị bỏ qua).
pub fn merge_marks(conn: &Connection, marks: &[WorthMark]) -> Result<Vec<WorthMark>> {
    let now = db::now();
    let tx = crate::db::write_tx(conn)?;
    let mut out = Vec::with_capacity(marks.len());
    for m in marks {
        let Some((nw, at)) = tx
            .query_row("SELECT not_worth, worth_at FROM books WHERE id = ?1", [m.book_id], |r| {
                Ok((r.get::<_, bool>(0)?, r.get::<_, i64>(1)?))
            })
            .optional()?
        else {
            continue;
        };
        let theirs = m.changed_at.min(now);
        if theirs > at && m.not_worth != nw {
            tx.execute(
                "UPDATE books SET not_worth = ?2, worth_at = ?3 WHERE id = ?1",
                params![m.book_id, m.not_worth, theirs],
            )?;
            out.push(WorthMark { book_id: m.book_id, not_worth: m.not_worth, changed_at: theirs });
        } else {
            out.push(WorthMark { book_id: m.book_id, not_worth: nw, changed_at: at.max(theirs) });
        }
    }
    tx.commit()?;
    Ok(out)
}

/// Tính SHA-256 đầy đủ cho các cuốn trong hàng chờ chưa có hash. Trả về số cuốn đã tính.
/// Chạy ở luồng nền; hash lưu vào DB và dùng chung với dedupe.
pub fn ensure_outbox_hashes(conn: &Connection, cancel: Option<&AtomicBool>) -> Result<usize> {
    let todo: Vec<(i64, String, i64, i64)> = {
        let mut st = conn.prepare(
            "SELECT DISTINCT b.id, b.path, b.size, b.mtime FROM outbox o JOIN books b ON b.id = o.book_id
             WHERE o.sent_at IS NULL AND b.status = 'present'
               AND (b.full_hash IS NULL OR b.hash_size IS NOT b.size OR b.hash_mtime IS NOT b.mtime)",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    let mut n = 0;
    for (id, path, size, mtime) in todo {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            break;
        }
        match hash::full_hash(Path::new(&path), cancel) {
            Ok(h) => {
                conn.execute(
                    "UPDATE books SET full_hash = ?1, hash_size = ?2, hash_mtime = ?3 WHERE id = ?4",
                    params![h, size, mtime, id],
                )?;
                n += 1;
            }
            Err(e) => tracing::warn!("không băm được {path}: {e}"),
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------------------------------------
// Thiết bị đã ghép
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct Device {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub last_seen: Option<i64>,
    pub free_bytes: Option<i64>,
    /// `ipad` | `iphone`
    pub kind: String,
}

pub fn list_devices(conn: &Connection) -> Result<Vec<Device>> {
    let mut st = conn.prepare("SELECT id, name, created_at, last_seen, free_bytes, kind FROM devices ORDER BY id")?;
    let rows = st.query_map([], |r| {
        Ok(Device {
            id: r.get(0)?,
            name: r.get(1)?,
            created_at: r.get(2)?,
            last_seen: r.get(3)?,
            free_bytes: r.get(4)?,
            kind: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn remove_device(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM devices WHERE id = ?1", [id])?;
    Ok(())
}

/// Đổi tên thiết bị (hiện trong danh sách và menu "Gửi sang …"). Tên trùng thì thêm số.
pub fn rename_device(conn: &Connection, id: i64, name: &str) -> Result<String> {
    let base = name.trim();
    if base.is_empty() {
        return Err(Error::msg("Tên thiết bị không được để trống"));
    }
    let name = unique_name(conn, base, Some(id))?;
    conn.execute("UPDATE devices SET name = ?1 WHERE id = ?2", params![name, id])?;
    Ok(name)
}

/// `iPad`, `iPad 2`, `iPad 3`… – không trùng tên thiết bị khác.
fn unique_name(conn: &Connection, base: &str, except: Option<i64>) -> Result<String> {
    let taken = |n: &str| -> Result<bool> {
        Ok(conn
            .query_row(
                "SELECT 1 FROM devices WHERE name = ?1 COLLATE NOCASE AND (?2 IS NULL OR id <> ?2)",
                params![n, except],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    };
    if !taken(base)? {
        return Ok(base.to_string());
    }
    for i in 2.. {
        let n = format!("{base} {i}");
        if !taken(&n)? {
            return Ok(n);
        }
    }
    unreachable!()
}

fn add_device(conn: &Connection, name: &str, kind: &str) -> Result<String> {
    let token = to_hex(&random_bytes::<32>());
    let kind = if kind == "iphone" { "iphone" } else { "ipad" };
    let base = name.trim();
    let base = if base.is_empty() {
        if kind == "iphone" {
            "iPhone"
        } else {
            "iPad"
        }
    } else {
        base
    };
    let name = unique_name(conn, base, None)?;
    conn.execute(
        "INSERT INTO devices (name, token_hash, created_at, kind) VALUES (?1, ?2, ?3, ?4)",
        params![name, token_hash(&token), db::now(), kind],
    )?;
    Ok(token)
}

fn auth_device(conn: &Connection, token: &str) -> Result<Option<i64>> {
    let id: Option<i64> =
        conn.query_row("SELECT id FROM devices WHERE token_hash = ?1", [token_hash(token)], |r| r.get(0)).optional()?;
    if let Some(id) = id {
        conn.execute("UPDATE devices SET last_seen = ?1 WHERE id = ?2", params![db::now(), id])?;
    }
    Ok(id)
}

// ---------------------------------------------------------------------------------------------
// Máy chủ HTTP
// ---------------------------------------------------------------------------------------------

struct Shared {
    home: PathBuf,
    name: String,
    pairing: Mutex<Option<(String, Instant)>>,
    stop: AtomicBool,
}

pub struct SyncServer {
    server: Arc<tiny_http::Server>,
    shared: Arc<Shared>,
    port: u16,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SyncServer {
    /// Bật máy chủ tại `bind` (vd. `0.0.0.0:47470`). `home`: thư mục dữ liệu app (có `library.db`).
    pub fn start(home: &Path, bind: &str, name: &str) -> Result<SyncServer> {
        let server =
            tiny_http::Server::http(bind).map_err(|e| Error::msg(format!("không mở được cổng {bind}: {e}")))?;
        let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(DEFAULT_PORT);
        let server = Arc::new(server);
        let shared = Arc::new(Shared {
            home: home.to_path_buf(),
            name: name.to_string(),
            pairing: Mutex::new(None),
            stop: AtomicBool::new(false),
        });
        // Bảo đảm library_id tồn tại và hash hàng chờ chạy nền.
        if let Ok(conn) = db::open(&home.join("library.db")) {
            let _ = library_id(&conn);
        }
        let (srv, sh) = (server.clone(), shared.clone());
        let thread = std::thread::Builder::new()
            .name("sync-server".into())
            .spawn(move || {
                while !sh.stop.load(Ordering::Relaxed) {
                    match srv.recv_timeout(Duration::from_millis(250)) {
                        Ok(Some(req)) => {
                            let sh = sh.clone();
                            std::thread::spawn(move || handle(&sh, req));
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| Error::msg(format!("không tạo được luồng máy chủ: {e}")))?;
        Ok(SyncServer { server, shared, port, thread: Some(thread) })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Tạo mã ghép mới (thay mã cũ). Hết hạn sau 5 phút, dùng một lần.
    pub fn new_pair_code(&self) -> String {
        let code = new_code();
        if let Ok(mut p) = self.shared.pairing.lock() {
            *p = Some((code.clone(), Instant::now()));
        }
        code
    }

    /// Mã ghép đang dùng nếu còn sống dưới `min_life` (chưa bị dùng) – trả cùng mã và số ms còn lại;
    /// ngược lại cấp mã mới. Để QR hiện trên màn hình không đổi trong `min_life`.
    pub fn pair_code_for(&self, min_life: Duration) -> (String, u64) {
        if let Ok(p) = self.shared.pairing.lock() {
            if let Some((c, t)) = p.as_ref() {
                if t.elapsed() < min_life {
                    return (c.clone(), (min_life - t.elapsed()).as_millis() as u64);
                }
            }
        }
        (self.new_pair_code(), min_life.as_millis() as u64)
    }

    /// Nội dung mã QR cho `code` hiện tại.
    pub fn pair_qr(&self, code: &str, addrs: Vec<String>) -> Result<PairQr> {
        let conn = db::open(&self.shared.home.join("library.db"))?;
        Ok(PairQr {
            v: PROTOCOL_VERSION,
            addrs,
            port: self.port,
            code: code.to_string(),
            library_id: library_id(&conn)?,
        })
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.server.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for SyncServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

type Resp = tiny_http::Response<Box<dyn Read + Send>>;

fn header(k: &str, v: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("header hợp lệ")
}

fn bytes_resp(status: u16, ctype: &str, body: Vec<u8>) -> Resp {
    let len = body.len();
    tiny_http::Response::new(
        tiny_http::StatusCode(status),
        vec![header("Content-Type", ctype), header("Cache-Control", "no-store")],
        Box::new(std::io::Cursor::new(body)) as Box<dyn Read + Send>,
        Some(len),
        None,
    )
}

fn json_resp<T: serde::Serialize>(status: u16, v: &T) -> Resp {
    bytes_resp(status, "application/json", serde_json::to_vec(v).unwrap_or_default())
}

fn err_resp(status: u16, msg: &str) -> Resp {
    json_resp(status, &ErrorBody { error: msg.to_string() })
}

/// `Range: bytes=a-b` / `a-` / `-n` → (start, end) gồm cả hai đầu. `Err(())` = không thoả được (416).
fn parse_range(h: &str, len: u64) -> std::result::Result<(u64, u64), ()> {
    let spec = h.trim().strip_prefix("bytes=").ok_or(())?;
    if spec.contains(',') || len == 0 {
        return Err(());
    }
    let (a, b) = spec.split_once('-').ok_or(())?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        (len.saturating_sub(n), len - 1)
    } else {
        let s: u64 = a.parse().map_err(|_| ())?;
        let e = if b.is_empty() { len - 1 } else { b.parse::<u64>().map_err(|_| ())?.min(len - 1) };
        (s, e)
    };
    if start > end || start >= len {
        return Err(());
    }
    Ok((start, end))
}

fn handle(sh: &Shared, mut req: tiny_http::Request) {
    let resp = match route(sh, &mut req) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("sync: {e}");
            err_resp(500, "Lỗi nội bộ")
        }
    };
    let _ = req.respond(resp);
}

fn route(sh: &Shared, req: &mut tiny_http::Request) -> Result<Resp> {
    match req.remote_addr() {
        Some(a) if is_local_ip(a.ip()) => {}
        _ => return Ok(err_resp(403, "Chỉ nhận kết nối từ mạng nội bộ")),
    }
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let method = req.method().clone();
    let Some(rest) = path.strip_prefix("/api/v1/") else { return Ok(err_resp(404, "Không có")) };
    let conn = db::open(&sh.home.join("library.db"))?;
    let config = Config::load(&sh.home)?;
    let drive = config.library_root.as_ref().is_some_and(|r| r.is_dir());
    let lib_id = library_id(&conn)?;

    let body = |req: &mut tiny_http::Request| -> Result<String> {
        let mut s = String::new();
        req.as_reader().take(MAX_BODY).read_to_string(&mut s)?;
        Ok(s)
    };

    // Không cần token.
    match (&method, rest) {
        (tiny_http::Method::Get, "hello") => {
            return Ok(json_resp(
                200,
                &Hello {
                    name: sh.name.clone(),
                    library_id: lib_id,
                    protocol: PROTOCOL_VERSION,
                    drive_connected: drive,
                },
            ));
        }
        (tiny_http::Method::Post, "pair") => {
            let Ok(pr) = serde_json::from_str::<PairRequest>(&body(req)?) else {
                return Ok(err_resp(400, "Yêu cầu sai"));
            };
            let ok = {
                let mut p = sh.pairing.lock().map_err(|_| Error::msg("khoá hỏng"))?;
                let valid = p.as_ref().is_some_and(|(c, t)| t.elapsed() < PAIR_TTL && ct_eq(c, pr.code.trim()));
                if valid {
                    *p = None; // dùng một lần
                }
                valid
            };
            if !ok {
                return Ok(err_resp(403, "Mã ghép sai hoặc đã hết hạn"));
            }
            let token = add_device(&conn, &pr.device_name, &pr.device_kind)?;
            return Ok(json_resp(200, &PairResponse { token, library_id: lib_id }));
        }
        _ => {}
    }

    // Còn lại cần token.
    let token = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(str::to_string));
    let Some(device) = token.as_deref().map(|t| auth_device(&conn, t)).transpose()?.flatten() else {
        return Ok(err_resp(401, "Thiếu hoặc sai token"));
    };

    match (&method, rest) {
        (tiny_http::Method::Get, "outbox") => {
            // Hash tính nền: kích hoạt nếu còn thiếu rồi trả ngay (sha256 = null cho tới khi xong).
            let items = queue_list(&conn, device)?;
            if items.iter().any(|i| i.sha256.is_none()) {
                let home = sh.home.clone();
                std::thread::spawn(move || {
                    if let Ok(c) = db::open(&home.join("library.db")) {
                        let _ = ensure_outbox_hashes(&c, None);
                    }
                });
            }
            Ok(json_resp(200, &Outbox { library_id: lib_id, drive_connected: drive, items }))
        }
        (tiny_http::Method::Post, "outbox/ack") => {
            let Ok(a) = serde_json::from_str::<AckRequest>(&body(req)?) else {
                return Ok(err_resp(400, "Yêu cầu sai"));
            };
            let stored: Option<Option<String>> = conn
                .query_row(
                    "SELECT b.full_hash FROM outbox o JOIN books b ON b.id = o.book_id WHERE o.book_id = ?1 AND o.device_id = ?2",
                    [a.book_id, device],
                    |r| r.get(0),
                )
                .optional()?;
            match stored {
                Some(Some(h)) if ct_eq(&h, &a.sha256.to_ascii_lowercase()) => {
                    conn.execute(
                        "UPDATE outbox SET sent_at = ?1 WHERE book_id = ?2 AND device_id = ?3",
                        params![db::now(), a.book_id, device],
                    )?;
                    let on: String =
                        conn.query_row("SELECT on_device FROM devices WHERE id = ?1", [device], |r| r.get(0))?;
                    let mut ids: Vec<i64> = serde_json::from_str(&on).unwrap_or_default();
                    if !ids.contains(&a.book_id) {
                        ids.push(a.book_id);
                    }
                    conn.execute(
                        "UPDATE devices SET on_device = ?1 WHERE id = ?2",
                        params![serde_json::to_string(&ids)?, device],
                    )?;
                    Ok(json_resp(200, &serde_json::json!({ "ok": true })))
                }
                Some(_) => Ok(err_resp(400, "Hash không khớp")),
                None => Ok(err_resp(404, "Không có trong hàng chờ")),
            }
        }
        (tiny_http::Method::Post, "status") => {
            let Ok(s) = serde_json::from_str::<StatusRequest>(&body(req)?) else {
                return Ok(err_resp(400, "Yêu cầu sai"));
            };
            conn.execute(
                "UPDATE devices SET free_bytes = ?1, on_device = ?2 WHERE id = ?3",
                params![s.free_bytes, serde_json::to_string(&s.books)?, device],
            )?;
            Ok(json_resp(200, &serde_json::json!({ "ok": true })))
        }
        (tiny_http::Method::Post, "marks") => {
            let Ok(m) = serde_json::from_str::<MarksRequest>(&body(req)?) else {
                return Ok(err_resp(400, "Yêu cầu sai"));
            };
            Ok(json_resp(200, &MarksResponse { marks: merge_marks(&conn, &m.marks)? }))
        }
        (tiny_http::Method::Get, r) if r.starts_with("books/") => {
            let mut parts = r["books/".len()..].split('/');
            let (Some(id), Some(what), None) =
                (parts.next().and_then(|s| s.parse::<i64>().ok()), parts.next(), parts.next())
            else {
                return Ok(err_resp(404, "Không có"));
            };
            let Some(book) = db::get_book(&conn, id)?.filter(|b| b.status == "present") else {
                return Ok(err_resp(404, "Không có sách này"));
            };
            if !drive {
                return Ok(err_resp(503, "Ổ sách trên máy tính chưa kết nối"));
            }
            let path = PathBuf::from(&book.path);
            match what {
                "file" => {
                    let range =
                        req.headers().iter().find(|h| h.field.equiv("Range")).map(|h| h.value.as_str().to_string());
                    serve_file(&path, range.as_deref())
                }
                "cover" => {
                    let fmt = Format::from_ext(&book.format);
                    let cover = match fmt {
                        Some(f) => CoverCache::new(&sh.home).get(book.id, &path, f, book.size, book.mtime)?,
                        None => None,
                    };
                    Ok(match cover {
                        Some(b) => bytes_resp(200, "image/jpeg", b),
                        None => err_resp(404, "Không có bìa"),
                    })
                }
                _ => Ok(err_resp(404, "Không có")),
            }
        }
        _ => Ok(err_resp(404, "Không có")),
    }
}

fn serve_file(path: &Path, range: Option<&str>) -> Result<Resp> {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Ok(err_resp(503, "Không mở được file trên ổ sách")),
    };
    let len = f.metadata()?.len();
    let (status, start, end) = match range {
        None => (200, 0, len.saturating_sub(1)),
        Some(h) => match parse_range(h, len) {
            Ok((s, e)) => (206, s, e),
            Err(()) => {
                let mut r = err_resp(416, "Range không hợp lệ");
                r.add_header(header("Content-Range", &format!("bytes */{len}")));
                return Ok(r);
            }
        },
    };
    let n = if len == 0 { 0 } else { end - start + 1 };
    f.seek(SeekFrom::Start(start))?;
    let mut headers = vec![
        header("Content-Type", "application/octet-stream"),
        header("Accept-Ranges", "bytes"),
        header("Cache-Control", "no-store"),
    ];
    if status == 206 {
        headers.push(header("Content-Range", &format!("bytes {start}-{end}/{len}")));
    }
    Ok(tiny_http::Response::new(
        tiny_http::StatusCode(status),
        headers,
        Box::new(f.take(n)) as Box<dyn Read + Send>,
        Some(n as usize),
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;

    struct Env {
        _dir: tempfile::TempDir,
        home: PathBuf,
        books: PathBuf,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let books = dir.path().join("books");
        std::fs::create_dir_all(&books).unwrap();
        let mut lib = crate::Library::open(&home).unwrap();
        lib.set_library_root(&books).unwrap();
        Env { _dir: dir, home, books }
    }

    fn add_book(e: &Env, id: i64, name: &str, data: &[u8]) -> String {
        let p = e.books.join(name);
        std::fs::write(&p, data).unwrap();
        let conn = db::open(&e.home.join("library.db")).unwrap();
        let fmt = name.rsplit('.').next().unwrap();
        conn.execute(
            "INSERT INTO books (id, root_id, path, rel_path, file_name, format, size, mtime, title, norm_title, added_at, updated_at)
             VALUES (?1, 1, ?2, ?3, ?3, ?4, ?5, 1, ?3, ?3, 0, 0)",
            params![id, p.to_string_lossy(), name, fmt, data.len() as i64],
        )
        .unwrap();
        to_hex(&Sha256::digest(data))
    }

    /// Gọi HTTP/1.1 thô, trả về (mã trạng thái, header viết thường, body).
    fn http(
        port: u16,
        method: &str,
        path: &str,
        token: Option<&str>,
        extra: &[(&str, &str)],
        body: &str,
    ) -> (u16, String, Vec<u8>) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut req =
            format!("{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
        if let Some(t) = token {
            req += &format!("Authorization: Bearer {t}\r\n");
        }
        for (k, v) in extra {
            req += &format!("{k}: {v}\r\n");
        }
        req += "\r\n";
        s.write_all(req.as_bytes()).unwrap();
        s.write_all(body.as_bytes()).unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, head, raw[split + 4..].to_vec())
    }

    fn pair(srv: &SyncServer) -> String {
        let code = srv.new_pair_code();
        let (st, _, b) = http(
            srv.port(),
            "POST",
            "/api/v1/pair",
            None,
            &[],
            &format!(r#"{{"code":"{code}","device_name":"iPad Air"}}"#),
        );
        assert_eq!(st, 200);
        serde_json::from_slice::<PairResponse>(&b).unwrap().token
    }

    #[test]
    fn ranges_parse() {
        assert_eq!(parse_range("bytes=0-9", 100), Ok((0, 9)));
        assert_eq!(parse_range("bytes=90-", 100), Ok((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Ok((90, 99)));
        assert_eq!(parse_range("bytes=50-500", 100), Ok((50, 99)));
        assert!(parse_range("bytes=100-", 100).is_err());
        assert!(parse_range("bytes=9-3", 100).is_err());
        assert!(parse_range("bytes=0-1,5-6", 100).is_err());
    }

    #[test]
    fn only_local_addresses() {
        for ok in [
            "127.0.0.1",
            "192.168.1.5",
            "10.0.0.2",
            "172.20.1.1",
            "169.254.3.3",
            "::1",
            "fd00::1",
            "::ffff:192.168.0.9",
        ] {
            assert!(is_local_ip(ok.parse().unwrap()), "{ok}");
        }
        for bad in ["8.8.8.8", "172.32.0.1", "2001:4860::1", "::ffff:8.8.8.8"] {
            assert!(!is_local_ip(bad.parse().unwrap()), "{bad}");
        }
    }

    fn dev(conn: &Connection, name: &str, kind: &str) -> i64 {
        add_device(conn, name, kind).unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn queue_is_per_device_and_iphone_gets_text_only() {
        let e = env();
        add_book(&e, 1, "a.epub", b"aaa");
        add_book(&e, 2, "b.cbr", b"bbb");
        add_book(&e, 3, "c.djvu", b"ccc");
        add_book(&e, 4, "d.pdf", b"ddd");
        let conn = db::open(&e.home.join("library.db")).unwrap();
        let (pad, phone) = (dev(&conn, "", "ipad"), dev(&conn, "", "iphone"));
        assert!(queue_add(&conn, &[1], &[]).is_err());
        let r = queue_add(&conn, &[1, 2, 3, 4, 99], &[pad, phone]).unwrap();
        // epub → cả hai; pdf → chỉ iPad; cbr/djvu bỏ; 99 không có.
        assert_eq!((r.added, r.books, r.skipped_unsupported, r.skipped_device, r.skipped_missing), (3, 2, 2, 1, 1));
        assert_eq!(queue_list(&conn, pad).unwrap().len(), 2);
        assert_eq!(queue_list(&conn, phone).unwrap().iter().map(|i| i.id).collect::<Vec<_>>(), vec![1]);
        // Gửi xong cho iPad không ảnh hưởng hàng chờ iPhone.
        conn.execute("UPDATE outbox SET sent_at = 5 WHERE device_id = ?1", [pad]).unwrap();
        assert!(queue_list(&conn, pad).unwrap().is_empty());
        assert_eq!(queue_list(&conn, phone).unwrap().len(), 1);
        queue_add(&conn, &[1], &[pad]).unwrap();
        assert_eq!(queue_list(&conn, pad).unwrap().len(), 1);
        assert_eq!(queue_remove(&conn, &[1], Some(pad)).unwrap(), 1);
        assert_eq!(queue_list(&conn, phone).unwrap().len(), 1);
        assert_eq!(queue_remove(&conn, &[1], None).unwrap(), 1);
        // Gỡ thiết bị thì hàng chờ của nó đi theo.
        queue_add(&conn, &[1], &[phone]).unwrap();
        remove_device(&conn, phone).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM outbox WHERE device_id = ?1", [phone], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn device_names_are_unique_and_renamable() {
        let e = env();
        let conn = db::open(&e.home.join("library.db")).unwrap();
        let a = dev(&conn, "", "ipad");
        let b = dev(&conn, "", "ipad");
        let c = dev(&conn, "", "iphone");
        let names =
            |conn: &Connection| list_devices(conn).unwrap().into_iter().map(|d| (d.name, d.kind)).collect::<Vec<_>>();
        assert_eq!(
            names(&conn),
            [("iPad".into(), "ipad".into()), ("iPad 2".into(), "ipad".into()), ("iPhone".into(), "iphone".into())]
        );
        assert_eq!(rename_device(&conn, b, "iPad của Na").unwrap(), "iPad của Na");
        assert_eq!(rename_device(&conn, c, "ipad").unwrap(), "ipad 2");
        assert_eq!(rename_device(&conn, a, "iPad").unwrap(), "iPad");
        assert!(rename_device(&conn, a, "  ").is_err());
    }

    #[test]
    fn full_sync_flow() {
        let e = env();
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let sha = add_book(&e, 1, "a.epub", &data);
        let srv = SyncServer::start(&e.home, "127.0.0.1:0", "PC").unwrap();
        let port = srv.port();

        // hello không cần token, ổ đang cắm.
        let (st, _, b) = http(port, "GET", "/api/v1/hello", None, &[], "");
        assert_eq!(st, 200);
        let hello: Hello = serde_json::from_slice(&b).unwrap();
        assert!(hello.drive_connected);
        assert_eq!(hello.protocol, 1);

        // Chưa ghép → 401; mã sai → 403.
        assert_eq!(http(port, "GET", "/api/v1/outbox", None, &[], "").0, 401);
        assert_eq!(http(port, "GET", "/api/v1/outbox", Some("sai"), &[], "").0, 401);
        srv.new_pair_code();
        assert_eq!(http(port, "POST", "/api/v1/pair", None, &[], r#"{"code":"000000x"}"#).0, 403);

        // Ghép: mã dùng một lần.
        let code = srv.new_pair_code();
        let body = format!(r#"{{"code":"{code}","device_name":"iPad Air"}}"#);
        let (st, _, b) = http(port, "POST", "/api/v1/pair", None, &[], &body);
        assert_eq!(st, 200);
        let token = serde_json::from_slice::<PairResponse>(&b).unwrap().token;
        assert_eq!(http(port, "POST", "/api/v1/pair", None, &[], &body).0, 403);
        // Thiết bị thứ hai (iPhone): hàng chờ riêng, không thấy sách xếp cho iPad.
        let code = srv.new_pair_code();
        let body2 = format!(r#"{{"code":"{code}","device_name":"iPhone","device_kind":"iphone"}}"#);
        let (_, _, b) = http(port, "POST", "/api/v1/pair", None, &[], &body2);
        let token2 = serde_json::from_slice::<PairResponse>(&b).unwrap().token;

        // Đưa vào hàng chờ iPad; hash tính xong thì outbox có sha256.
        let conn = db::open(&e.home.join("library.db")).unwrap();
        let devs = list_devices(&conn).unwrap();
        assert_eq!((devs[1].name.as_str(), devs[1].kind.as_str()), ("iPhone", "iphone"));
        queue_add(&conn, &[1], &[devs[0].id]).unwrap();
        ensure_outbox_hashes(&conn, None).unwrap();
        let (st, _, b) = http(port, "GET", "/api/v1/outbox", Some(&token), &[], "");
        assert_eq!(st, 200);
        let ob: Outbox = serde_json::from_slice(&b).unwrap();
        assert_eq!(ob.items.len(), 1);
        assert_eq!(ob.items[0].sha256.as_deref(), Some(sha.as_str()));
        let (_, _, b) = http(port, "GET", "/api/v1/outbox", Some(&token2), &[], "");
        assert!(serde_json::from_slice::<Outbox>(&b).unwrap().items.is_empty());
        // iPhone không ack được cuốn không xếp cho nó.
        let ack = format!(r#"{{"book_id":1,"sha256":"{sha}"}}"#);
        assert_eq!(http(port, "POST", "/api/v1/outbox/ack", Some(&token2), &[], &ack).0, 404);

        // Tải nguyên file và tải tiếp bằng Range.
        let (st, _, full) = http(port, "GET", "/api/v1/books/1/file", Some(&token), &[], "");
        assert_eq!((st, full.as_slice()), (200, data.as_slice()));
        let (st, head, part) = http(port, "GET", "/api/v1/books/1/file", Some(&token), &[("Range", "bytes=4000-")], "");
        assert_eq!(st, 206);
        assert!(head.contains("content-range: bytes 4000-4999/5000"), "{head}");
        assert_eq!(part, &data[4000..]);
        assert_eq!(http(port, "GET", "/api/v1/books/1/file", Some(&token), &[("Range", "bytes=9999-")], "").0, 416);
        assert_eq!(http(port, "GET", "/api/v1/books/77/file", Some(&token), &[], "").0, 404);
        assert_eq!(http(port, "GET", "/api/v1/books/1/file", None, &[], "").0, 401);

        // Ack sai hash bị từ chối, đúng hash thì rời hàng chờ.
        let bad = format!(r#"{{"book_id":1,"sha256":"{}"}}"#, "0".repeat(64));
        assert_eq!(http(port, "POST", "/api/v1/outbox/ack", Some(&token), &[], &bad).0, 400);
        let good = format!(r#"{{"book_id":1,"sha256":"{sha}"}}"#);
        assert_eq!(http(port, "POST", "/api/v1/outbox/ack", Some(&token), &[], &good).0, 200);
        let (_, _, b) = http(port, "GET", "/api/v1/outbox", Some(&token), &[], "");
        assert!(serde_json::from_slice::<Outbox>(&b).unwrap().items.is_empty());
        assert_eq!(on_device_ids(&conn).unwrap(), vec![1]);

        // status cập nhật dung lượng + danh sách.
        assert_eq!(
            http(port, "POST", "/api/v1/status", Some(&token), &[], r#"{"free_bytes":12345,"books":[1]}"#).0,
            200
        );
        let d = list_devices(&conn).unwrap();
        assert_eq!((d.len(), d[0].free_bytes, d[0].name.as_str()), (2, Some(12345), "iPad Air"));
        assert_eq!(device_books(&conn, d[0].id).unwrap().len(), 1);
        assert!(device_books(&conn, d[1].id).unwrap().is_empty());
        assert_eq!(devices_with_book(&conn, 1).unwrap(), vec!["iPad Air".to_string()]);

        // Gỡ thiết bị → token vô hiệu.
        remove_device(&conn, d[0].id).unwrap();
        assert_eq!(http(port, "GET", "/api/v1/outbox", Some(&token), &[], "").0, 401);
        srv.stop();
    }

    #[test]
    fn ipad_client_syncs_resumes_and_verifies() {
        use crate::ipad_lib;
        use crate::sync_client::{self, Options};
        use std::sync::atomic::AtomicBool;
        let e = env();
        let a: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
        let b: Vec<u8> = (0..1000u32).map(|i| (i % 7) as u8).collect();
        let sha_a = add_book(&e, 1, "a.epub", &a);
        add_book(&e, 2, "b.epub", &b);
        let srv = SyncServer::start(&e.home, "127.0.0.1:0", "PC").unwrap();
        let conn = db::open(&e.home.join("library.db")).unwrap();

        // Ghép bằng mã QR.
        let qr = srv.pair_qr(&srv.new_pair_code(), vec!["127.0.0.1".into()]).unwrap();
        let mut p = sync_client::pair(&qr, "iPad Test", "ipad").unwrap();
        let pad = list_devices(&conn).unwrap()[0].id;
        queue_add(&conn, &[1, 2], &[pad]).unwrap();
        ensure_outbox_hashes(&conn, None).unwrap();
        assert_eq!(p.pc_name, "PC");
        let (client, hello) = sync_client::connect(&mut p).unwrap();
        assert!(hello.drive_connected);

        let dir = tempfile::tempdir().unwrap();
        let books = dir.path().join("Books");
        std::fs::create_dir_all(books.join(".partial")).unwrap();
        // Đã có sẵn nửa đầu file a trong thư mục tạm → phải tải tiếp bằng Range.
        std::fs::write(books.join(".partial").join(format!("{sha_a}.part")), &a[..100_000]).unwrap();
        let ipad = ipad_lib::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);

        // Thiếu chỗ → báo lỗi, chưa tải gì.
        let o = Options { books_dir: &books, free_bytes: Some(1000), cancel: &cancel, only: None };
        let err = sync_client::sync(&ipad, &client, &o, &|_| {}).unwrap_err().to_string();
        assert!(err.contains("không đủ chỗ"), "{err}");

        let o = Options { books_dir: &books, free_bytes: Some(10 << 30), cancel: &cancel, only: None };
        let rep = sync_client::sync(&ipad, &client, &o, &|_| {}).unwrap();
        assert_eq!((rep.added, rep.failed.len()), (2, 0), "{rep:?}");
        let list = ipad_lib::list(&ipad, &ipad_lib::Query::default()).unwrap();
        assert_eq!(list.len(), 2);
        // File đúng nội dung và đã ack (rời hàng chờ, hiện "có trên iPad").
        let mut ids = ipad_lib::remote_ids(&ipad, &hello.library_id).unwrap();
        ids.sort();
        assert_eq!(ids, vec![1, 2]);
        let first = ipad_lib::series_volumes(&ipad, "").unwrap();
        assert!(first.is_empty());
        let mut sizes: Vec<usize> = [1, 2]
            .iter()
            .map(|&i| std::fs::read(books.join(ipad_lib::get(&ipad, i).unwrap().unwrap().location)).unwrap().len())
            .collect();
        sizes.sort();
        assert_eq!(sizes, vec![1000, 300_000]);
        assert!(queue_list(&conn, pad).unwrap().is_empty());
        assert_eq!(on_device_ids(&conn).unwrap().len(), 2);
        assert_eq!(list_devices(&conn).unwrap()[0].free_bytes, Some(10 << 30));

        // Đồng bộ lần nữa: không có gì mới.
        let rep = sync_client::sync(&ipad, &client, &o, &|_| {}).unwrap();
        assert_eq!((rep.added, rep.already), (0, 0));

        // Phần tạm bị hỏng → SHA không khớp, file tạm bị xoá, không vào thư viện.
        let c3: Vec<u8> = vec![9u8; 5000];
        add_book(&e, 3, "c.epub", &c3);
        queue_add(&conn, &[3], &[pad]).unwrap();
        ensure_outbox_hashes(&conn, None).unwrap();
        let sha_c = to_hex(&Sha256::digest(&c3));
        std::fs::write(books.join(".partial").join(format!("{sha_c}.part")), vec![1u8; 5000]).unwrap();
        let rep = sync_client::sync(&ipad, &client, &o, &|_| {}).unwrap();
        // Phần tạm hỏng bị loại (SHA không khớp) rồi tự tải lại từ đầu ở lần thử kế tiếp.
        assert_eq!((rep.added, rep.failed.len()), (1, 0), "{rep:?}");
        assert!(!books.join(".partial").join(format!("{sha_c}.part")).exists());
        let loc = ipad_lib::get(&ipad, 3).unwrap().unwrap().location;
        assert_eq!(std::fs::read(books.join(loc)).unwrap(), c3);
        srv.stop();
    }

    /// Cờ "không đáng đọc" đồng bộ hai chiều: ai đổi sau thắng, ẩn khỏi thư viện ở cả hai bên.
    #[test]
    fn not_worth_marks_sync_both_ways() {
        use crate::browse::{self, Query};
        use crate::ipad_lib;
        use crate::sync_client::{self, Options};
        use std::sync::atomic::AtomicBool;
        let e = env();
        add_book(&e, 1, "a.epub", b"aaaa");
        add_book(&e, 2, "b.epub", b"bbbb");
        add_book(&e, 3, "c.epub", b"cccc");
        let srv = SyncServer::start(&e.home, "127.0.0.1:0", "PC").unwrap();
        let conn = db::open(&e.home.join("library.db")).unwrap();
        let qr = srv.pair_qr(&srv.new_pair_code(), vec!["127.0.0.1".into()]).unwrap();
        let mut p = sync_client::pair(&qr, "iPhone", "iphone").unwrap();
        let dev = list_devices(&conn).unwrap()[0].id;
        queue_add(&conn, &[1, 2, 3], &[dev]).unwrap();
        ensure_outbox_hashes(&conn, None).unwrap();
        let (client, hello) = sync_client::connect(&mut p).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ipad = ipad_lib::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let o = Options { books_dir: dir.path(), free_bytes: None, cancel: &cancel, only: None };

        // Máy tính đánh dấu cuốn 1 trước khi tải → cuốn vừa tải về đã nằm ở tab "Không đáng đọc".
        assert_eq!(browse::set_not_worth(&conn, &[1], true).unwrap(), 1);
        assert_eq!(browse::set_not_worth(&conn, &[1], true).unwrap(), 0);
        let page = browse::query(&conn, &Query::default()).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(browse::query(&conn, &Query { not_worth: true, ..Default::default() }).unwrap().total, 1);
        assert_eq!(browse::facets(&conn).unwrap().not_worth, 1);
        let rep = sync_client::sync(&ipad, &client, &o, &|_| {}).unwrap();
        assert_eq!((rep.added, rep.marks_changed), (3, 1), "{rep:?}");
        let local = |id: i64| ipad_lib::get(&ipad, id).unwrap().unwrap().not_worth;
        let pad_id = |remote: i64| -> i64 {
            ipad.query_row("SELECT id FROM books WHERE remote_id = ?1", [remote], |r| r.get(0)).unwrap()
        };
        assert!(local(pad_id(1)) && !local(pad_id(2)));
        assert_eq!(ipad_lib::list(&ipad, &ipad_lib::Query::default()).unwrap().len(), 2);
        let nw = ipad_lib::Query { not_worth: true, ..Default::default() };
        assert_eq!(ipad_lib::list(&ipad, &nw).unwrap().len(), 1);
        assert_eq!(ipad_lib::not_worth_count(&ipad).unwrap(), 1);

        // Điện thoại đánh dấu cuốn 2 và đưa cuốn 1 lại thư viện → máy tính nhận theo (mốc sau hơn).
        let t = db::now();
        conn.execute("UPDATE books SET worth_at = ?1 - 10 WHERE id = 1", [t]).unwrap();
        ipad_lib::set_not_worth(&ipad, &[pad_id(2)], true).unwrap();
        ipad_lib::set_not_worth(&ipad, &[pad_id(1)], false).unwrap();
        assert_eq!(sync_client::sync_marks(&ipad, &client, &hello.library_id).unwrap(), 0);
        let flag = |id: i64| -> bool { db::get_book(&conn, id).unwrap().unwrap().not_worth };
        assert!(!flag(1) && flag(2) && !flag(3));

        // Máy tính đổi sau → điện thoại theo ở lần đồng bộ kế tiếp.
        conn.execute("UPDATE books SET worth_at = 0 WHERE id = 3", []).unwrap();
        browse::set_not_worth(&conn, &[3], true).unwrap();
        browse::set_not_worth(&conn, &[2], false).unwrap();
        assert_eq!(sync_client::sync_marks(&ipad, &client, &hello.library_id).unwrap(), 2);
        assert!(!local(pad_id(2)) && local(pad_id(3)));

        // Đồng hồ điện thoại chạy nhanh không thắng mãi: mốc bị kẹp ở "bây giờ" của máy tính.
        conn.execute("UPDATE books SET worth_at = ?1 - 5 WHERE id = 3", [t]).unwrap();
        ipad.execute("UPDATE worth SET not_worth = 0, changed_at = ?1", [t + 100_000]).unwrap();
        sync_client::sync_marks(&ipad, &client, &hello.library_id).unwrap();
        assert!(!flag(3));
        let at: i64 = conn.query_row("SELECT worth_at FROM books WHERE id = 3", [], |r| r.get(0)).unwrap();
        assert!(at <= db::now() + 1);

        // Xoá khỏi máy rồi gửi lại: cờ vẫn còn (khoá theo SHA-256).
        browse::set_not_worth(&conn, &[1], true).unwrap();
        sync_client::sync_marks(&ipad, &client, &hello.library_id).unwrap();
        ipad_lib::remove_books(&ipad, &[pad_id(1)]).unwrap();
        queue_add(&conn, &[1], &[dev]).unwrap();
        let rep = sync_client::sync(&ipad, &client, &o, &|_| {}).unwrap();
        assert_eq!(rep.added, 1, "{rep:?}");
        assert!(local(pad_id(1)));
        srv.stop();
    }

    #[test]
    fn drive_unplugged_is_reported() {
        let e = env();
        add_book(&e, 1, "a.epub", b"xyz");
        let srv = SyncServer::start(&e.home, "127.0.0.1:0", "PC").unwrap();
        let token = pair(&srv);
        std::fs::remove_dir_all(&e.books).unwrap();
        let (_, _, b) = http(srv.port(), "GET", "/api/v1/hello", None, &[], "");
        assert!(!serde_json::from_slice::<Hello>(&b).unwrap().drive_connected);
        assert_eq!(http(srv.port(), "GET", "/api/v1/books/1/file", Some(&token), &[], "").0, 503);
    }
}

#[cfg(test)]
mod browse_tests {
    use super::*;
    use crate::browse::{self, Item, Query};

    #[test]
    fn on_ipad_filter_flag_and_facet() {
        let conn = db::open_in_memory().unwrap();
        db::ensure_root(&conn, "C:/b", "library").unwrap();
        for id in 1..=3 {
            conn.execute(
                "INSERT INTO books (id, root_id, path, rel_path, file_name, format, size, mtime, title, norm_title, added_at, updated_at)
                 VALUES (?1, 1, ?2, ?2, ?2, 'epub', 1, 1, ?2, ?2, 0, 0)",
                params![id, format!("b{id}.epub")],
            )
            .unwrap();
        }
        let tok = add_device(&conn, "iPad", "ipad").unwrap();
        let dev = auth_device(&conn, &tok).unwrap().unwrap();
        conn.execute("UPDATE devices SET on_device = '[2]' WHERE id = ?1", [dev]).unwrap();
        let all = browse::query(&conn, &Query::default()).unwrap();
        let flags: Vec<(i64, bool)> = all
            .items
            .iter()
            .map(|i| match i {
                Item::Book(b) => (b.id, b.on_ipad),
                _ => unreachable!(),
            })
            .collect();
        assert!(flags.contains(&(2, true)) && flags.contains(&(1, false)));
        let only = browse::query(&conn, &Query { on_ipad: true, ..Default::default() }).unwrap();
        assert_eq!(only.total, 1);
        assert_eq!(browse::facets(&conn).unwrap().on_ipad, 1);
    }
}
