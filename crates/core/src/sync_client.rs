//! Phía iPad của đồng bộ LAN (I4): ghép nối, đọc hàng chờ, tải file bằng Range (tải tiếp được), kiểm SHA-256
//! rồi mới đưa vào thư viện và `ack` về máy tính. HTTP thuần (không TLS) trong mạng nội bộ.

use crate::error::{Error, Result};
use crate::ipad_lib::{self, NewBook};
use crate::sync_proto::*;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// Chỗ trống chừa lại cho iPad (không tải đầy tới sát đáy).
pub const FREE_MARGIN: i64 = 512 * 1024 * 1024;
const TRIES: usize = 3;
/// Số lần hỏi lại outbox khi máy tính còn đang tính hash (mỗi lần ~1 giây).
const HASH_WAIT: usize = 120;

/// Máy tính đã ghép (lưu trên iPad).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pairing {
    pub addrs: Vec<String>,
    pub port: u16,
    pub token: String,
    pub library_id: String,
    pub pc_name: String,
}

pub struct Client {
    agent: ureq::Agent,
    base: String,
    token: Option<String>,
}

fn net<E: std::fmt::Display>(e: E) -> Error {
    Error::msg(format!("Lỗi mạng: {e}"))
}

fn http_err(e: ureq::Error) -> Error {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<ErrorBody>(&body).map(|b| b.error).unwrap_or(body);
            Error::msg(match code {
                401 => "Máy tính không nhận iPad này nữa – hãy ghép nối lại".to_string(),
                _ => format!("Máy tính trả lỗi {code}: {msg}"),
            })
        }
        other => net(other),
    }
}

impl Client {
    pub fn new(addr: &str, port: u16, token: Option<&str>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(4))
            .timeout_read(Duration::from_secs(30))
            .build();
        let host = if addr.contains(':') && !addr.starts_with('[') { format!("[{addr}]") } else { addr.to_string() };
        Client { agent, base: format!("http://{host}:{port}/api/v1"), token: token.map(String::from) }
    }

    fn req(&self, method: &str, path: &str) -> ureq::Request {
        let r = self.agent.request(method, &format!("{}/{path}", self.base));
        match &self.token {
            Some(t) => r.set("Authorization", &format!("Bearer {t}")),
            None => r,
        }
    }

    fn get_json<T: for<'a> Deserialize<'a>>(&self, path: &str) -> Result<T> {
        let s = self.req("GET", path).call().map_err(http_err)?.into_string().map_err(net)?;
        serde_json::from_str(&s).map_err(|e| Error::msg(format!("Phản hồi không hợp lệ: {e}")))
    }

    fn post_json<B: Serialize>(&self, path: &str, body: &B) -> Result<String> {
        let b = serde_json::to_string(body).map_err(|e| Error::msg(e.to_string()))?;
        self.req("POST", path)
            .set("Content-Type", "application/json")
            .send_string(&b)
            .map_err(http_err)?
            .into_string()
            .map_err(net)
    }

    pub fn hello(&self) -> Result<Hello> {
        self.get_json("hello")
    }

    pub fn pair(&self, code: &str, device_name: &str, device_kind: &str) -> Result<PairResponse> {
        let s = self.post_json(
            "pair",
            &PairRequest {
                code: code.trim().to_string(),
                device_name: device_name.to_string(),
                device_kind: device_kind.to_string(),
            },
        )?;
        serde_json::from_str(&s).map_err(|e| Error::msg(format!("Phản hồi không hợp lệ: {e}")))
    }

    pub fn outbox(&self) -> Result<Outbox> {
        self.get_json("outbox")
    }

    pub fn ack(&self, book_id: i64, sha256: &str) -> Result<()> {
        self.post_json("outbox/ack", &AckRequest { book_id, sha256: sha256.to_string() }).map(|_| ())
    }

    pub fn status(&self, free_bytes: i64, books: Vec<i64>) -> Result<()> {
        self.post_json("status", &StatusRequest { free_bytes, books }).map(|_| ())
    }

    /// Đổi cờ "không đáng đọc". `Ok(None)`: máy tính bản cũ chưa có tính năng này.
    pub fn marks(&self, marks: Vec<WorthMark>) -> Result<Option<Vec<WorthMark>>> {
        let b = serde_json::to_string(&MarksRequest { marks }).map_err(|e| Error::msg(e.to_string()))?;
        match self.req("POST", "marks").set("Content-Type", "application/json").send_string(&b) {
            Ok(r) => {
                let s = r.into_string().map_err(net)?;
                let m: MarksResponse =
                    serde_json::from_str(&s).map_err(|e| Error::msg(format!("Phản hồi không hợp lệ: {e}")))?;
                Ok(Some(m.marks))
            }
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(e) => Err(http_err(e)),
        }
    }
}

/// Đồng bộ hai chiều cờ "không đáng đọc" với máy tính (nhẹ: chỉ siêu dữ liệu). Trả về số cuốn trên máy này
/// đổi theo máy tính. Máy tính bản cũ → `0`.
pub fn sync_marks(conn: &Connection, c: &Client, library_id: &str) -> Result<usize> {
    let mine = ipad_lib::worth_marks(conn, library_id)?;
    if mine.is_empty() {
        return Ok(0);
    }
    match c.marks(mine.clone())? {
        Some(theirs) => ipad_lib::apply_worth_marks(conn, library_id, &theirs, &mine),
        None => Ok(0),
    }
}

/// Ghép nối từ nội dung mã QR (hoặc IP + mã gõ tay: `addrs` một phần tử). Thử từng địa chỉ tới khi có máy trả lời.
pub fn pair(qr: &PairQr, device_name: &str, device_kind: &str) -> Result<Pairing> {
    let mut last = Error::msg("Không có địa chỉ nào để thử");
    for a in &qr.addrs {
        let c = Client::new(a, qr.port, None);
        let hello = match c.hello() {
            Ok(h) => h,
            Err(e) => {
                last = e;
                continue;
            }
        };
        if hello.protocol != PROTOCOL_VERSION {
            return Err(Error::msg("Phiên bản app máy tính và iPad không khớp – hãy cập nhật cả hai"));
        }
        let r = c.pair(&qr.code, device_name, device_kind)?;
        let mut addrs = vec![a.clone()];
        addrs.extend(qr.addrs.iter().filter(|x| *x != a).cloned());
        return Ok(Pairing { addrs, port: qr.port, token: r.token, library_id: r.library_id, pc_name: hello.name });
    }
    Err(Error::msg(format!("Không kết nối được máy tính ({last}). Hai máy cần cùng mạng Wi-Fi.")))
}

/// Tìm địa chỉ đang trả lời của máy đã ghép; địa chỉ chạy được đưa lên đầu danh sách.
pub fn connect(p: &mut Pairing) -> Result<(Client, Hello)> {
    let mut last = Error::msg("Không có địa chỉ");
    for i in 0..p.addrs.len() {
        let c = Client::new(&p.addrs[i], p.port, Some(&p.token));
        match c.hello() {
            Ok(h) if h.library_id == p.library_id => {
                p.addrs.swap(0, i);
                return Ok((c, h));
            }
            Ok(_) => last = Error::msg("Đây là máy tính khác với máy đã ghép"),
            Err(e) => last = e,
        }
    }
    Err(Error::msg(format!("Không thấy máy tính {} ({last})", p.pc_name)))
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub book_id: i64,
    pub title: String,
    /// Số cuốn đã xong / tổng số cuốn cần tải.
    pub done: usize,
    pub total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub added: usize,
    /// Có sẵn trên iPad (cùng SHA-256) – chỉ báo lại cho máy tính.
    pub already: usize,
    pub failed: Vec<(String, String)>,
    pub bytes: u64,
    pub cancelled: bool,
    /// Máy tính báo ổ sách chưa cắm → một số cuốn không tải được.
    pub drive_connected: bool,
    /// Số cuốn trên thiết bị đổi cờ "không đáng đọc" theo máy tính.
    pub marks_changed: usize,
}

pub fn fmt_bytes(b: i64) -> String {
    let (mut x, mut i) = (b.max(0) as f64, 0);
    while x >= 1024.0 && i < 4 {
        x /= 1024.0;
        i += 1;
    }
    format!("{:.1} {}", x, ["B", "KB", "MB", "GB", "TB"][i])
}

fn safe_name(s: &str) -> String {
    let t: String =
        s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == ' ' { c } else { '_' }).take(60).collect();
    t.trim().to_string()
}

fn sha_of(path: &Path, cancel: &AtomicBool) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let (mut h, mut buf) = (Sha256::new(), vec![0u8; 1 << 20]);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::msg("Đã huỷ"));
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|x| format!("{x:02x}")).collect())
}

/// Tải một cuốn về `part` (tải tiếp nếu đã có phần đầu), kiểm SHA-256 rồi đổi tên thành `dest`.
fn download(
    c: &Client,
    item: &OutboxItem,
    sha: &str,
    part: &Path,
    dest: &Path,
    cancel: &AtomicBool,
    on_bytes: &dyn Fn(i64),
) -> Result<()> {
    let size = item.size.max(0) as u64;
    let mut have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if have > size {
        let _ = std::fs::remove_file(part);
        have = 0;
    }
    on_bytes(have as i64);
    if have < size {
        let mut rq = c.req("GET", &format!("books/{}/file", item.id));
        if have > 0 {
            rq = rq.set("Range", &format!("bytes={have}-"));
        }
        let resp = rq.call().map_err(http_err)?;
        let resumed = resp.status() == 206;
        if !resumed && have > 0 {
            on_bytes(-(have as i64)); // máy tính không hỗ trợ Range → tải lại từ đầu
            have = 0;
        }
        let mut f =
            std::fs::OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(part)?;
        let mut r = resp.into_reader();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::msg("Đã huỷ"));
            }
            let n = r.read(&mut buf).map_err(net)?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n])?;
            have += n as u64;
            on_bytes(n as i64);
        }
        f.flush()?;
        if have < size {
            return Err(Error::msg("Kết nối bị ngắt giữa chừng"));
        }
    }
    if sha_of(part, cancel)? != sha.to_ascii_lowercase() {
        let _ = std::fs::remove_file(part);
        return Err(Error::msg("File tải về bị hỏng (SHA-256 không khớp) – sẽ tải lại ở lần sau"));
    }
    std::fs::rename(part, dest)?;
    Ok(())
}

pub struct Options<'a> {
    pub books_dir: &'a Path,
    pub free_bytes: Option<i64>,
    pub cancel: &'a AtomicBool,
    /// Chỉ tải các cuốn có định dạng thoả điều kiện này (iPhone: chỉ sách chữ). Cuốn bị bỏ qua vẫn nằm
    /// trong hàng chờ của máy tính cho thiết bị khác. `None` = tải hết.
    pub only: Option<fn(&str) -> bool>,
}

/// Một lần đồng bộ: tải các cuốn trong hàng chờ chưa có trên iPad. Gọi từ luồng nền.
pub fn sync(conn: &Connection, c: &Client, o: &Options, on: &(dyn Fn(Progress) + Sync)) -> Result<Report> {
    let mut ob = c.outbox()?;
    // Máy tính tính hash nền: hỏi lại tới khi có đủ.
    for _ in 0..HASH_WAIT {
        if ob.items.iter().filter(|i| o.only.is_none_or(|f| f(&i.format))).all(|i| i.sha256.is_some())
            || o.cancel.load(Ordering::Relaxed)
        {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
        ob = c.outbox()?;
    }
    let lib_id = ob.library_id.clone();
    let mut report = Report { drive_connected: ob.drive_connected, ..Default::default() };
    let mut todo = vec![];
    for it in ob.items {
        if o.only.is_some_and(|f| !f(&it.format)) {
            continue;
        }
        let Some(sha) = it.sha256.clone() else { continue };
        let have = conn.query_row("SELECT 1 FROM books WHERE sha256 = ?1", [&sha], |_| Ok(())).optional()?.is_some();
        if have {
            report.already += 1;
            let _ = c.ack(it.id, &sha);
        } else {
            todo.push((it, sha));
        }
    }
    let bytes_total: u64 = todo.iter().map(|(i, _)| i.size.max(0) as u64).sum();
    if let Some(free) = o.free_bytes {
        let need = bytes_total as i64 + FREE_MARGIN;
        if need > free {
            return Err(Error::msg(format!(
                "iPad không đủ chỗ: cần {} (kèm {} dự phòng), còn trống {}. Hãy xoá bớt sách đã đọc rồi thử lại.",
                fmt_bytes(need),
                fmt_bytes(FREE_MARGIN),
                fmt_bytes(free)
            )));
        }
    }
    let tmp = o.books_dir.join(".partial");
    std::fs::create_dir_all(&tmp)?;
    let total = todo.len();
    let next = AtomicUsize::new(0);
    let got = AtomicU64::new(0);
    let (tx, rx) = mpsc::channel::<(usize, std::result::Result<PathBuf, String>)>();
    let mut done = 0usize;
    let dest_of =
        |it: &OutboxItem, sha: &str| format!("{}-{}.{}", &sha[..12.min(sha.len())], safe_name(&it.title), it.format);

    std::thread::scope(|s| {
        for _ in 0..2 {
            let tx = tx.clone();
            let (todo, next, got, tmp) = (&todo, &next, &got, &tmp);
            let dest_of = &dest_of;
            s.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= todo.len() || o.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let (it, sha) = &todo[i];
                let (part, dest) = (tmp.join(format!("{sha}.part")), o.books_dir.join(dest_of(it, sha)));
                let mut res = Err(Error::msg("chưa chạy"));
                for t in 0..TRIES {
                    // Thử lại: bỏ phần đã cộng của lần trước (download cộng lại phần file tạm đã có).
                    let delta = std::cell::Cell::new(0i64);
                    let r = download(c, it, sha, &part, &dest, o.cancel, &|n| {
                        delta.set(delta.get() + n);
                        if n >= 0 {
                            got.fetch_add(n as u64, Ordering::Relaxed);
                        } else {
                            got.fetch_sub(n.unsigned_abs(), Ordering::Relaxed);
                        }
                        on(Progress {
                            book_id: it.id,
                            title: it.title.clone(),
                            done: 0,
                            total,
                            bytes_done: got.load(Ordering::Relaxed),
                            bytes_total,
                        });
                    });
                    res = r;
                    if res.is_ok() || o.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    got.fetch_sub(delta.get().max(0) as u64, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_secs(1 + t as u64));
                }
                let _ = tx.send((i, res.map(|_| dest).map_err(|e| e.to_string())));
            });
        }
        drop(tx);
        for (i, res) in rx {
            let (it, sha) = &todo[i];
            match res {
                Ok(dest) => {
                    let loc = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let nb = NewBook {
                        library_id: Some(lib_id.clone()),
                        remote_id: Some(it.id),
                        title: it.title.clone(),
                        authors: it.authors.clone(),
                        series: it.series.clone(),
                        volume: it.volume,
                        format: it.format.clone(),
                        size: it.size,
                        sha256: sha.clone(),
                        location: loc,
                        in_place: false,
                    };
                    match ipad_lib::add_book(conn, &nb) {
                        Ok(_) => {
                            report.added += 1;
                            report.bytes += it.size.max(0) as u64;
                            // Ack hỏng không sao: lần sau thấy đã có sẽ ack lại.
                            let _ = c.ack(it.id, sha);
                        }
                        Err(e) => report.failed.push((it.title.clone(), e.to_string())),
                    }
                }
                Err(e) => report.failed.push((it.title.clone(), e)),
            }
            done += 1;
            on(Progress {
                book_id: it.id,
                title: it.title.clone(),
                done,
                total,
                bytes_done: got.load(Ordering::Relaxed),
                bytes_total,
            });
        }
    });
    report.cancelled = o.cancel.load(Ordering::Relaxed);
    // Cờ "không đáng đọc" đi kèm mỗi lần đồng bộ (cả cho các cuốn vừa tải). Lỗi ở đây không làm hỏng lần tải.
    report.marks_changed = sync_marks(conn, c, &lib_id).unwrap_or_else(|e| {
        tracing::warn!("đồng bộ cờ không đáng đọc: {e}");
        0
    });
    if let Some(free) = o.free_bytes {
        let _ = c.status(free, ipad_lib::remote_ids(conn, &lib_id)?);
    }
    Ok(report)
}

/// Báo trạng thái (dung lượng + sách đang có) mà không tải gì – dùng khi mở màn Đồng bộ.
pub fn report_status(conn: &Connection, c: &Client, library_id: &str, free: i64) -> Result<()> {
    let _ = sync_marks(conn, c, library_id);
    c.status(free, ipad_lib::remote_ids(conn, library_id)?)
}
