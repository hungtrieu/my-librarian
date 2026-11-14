//! Sửa tiêu đề sách (app Windows).
//!
//! Ba lớp, từ an toàn nhất:
//! 1. **DB**: tiêu đề mới được ghi vào `title` + `user_title` và `norm_title` tính lại ngay → tìm kiếm, sắp xếp,
//!    gộp "một tác phẩm nhiều định dạng" và **so nghi trùng** dùng tên mới từ lần kế tiếp. `user_title` là tiêu đề
//!    người dùng đặt: các lần quét sau (kể cả khi file đổi nội dung) không ghi đè nó bằng tiêu đề trong file.
//! 2. **Tên file**: đổi thành `<tiêu đề>.<đuôi cũ>` trong cùng thư mục (bỏ ký tự Windows cấm, trùng tên thêm `(1)`,
//!    không ghi đè).
//! 3. **Nội dung sách** (EPUB: `dc:title` trong OPF; FB2 UTF-8: `<book-title>`): ghi ra file tạm cạnh file gốc,
//!    đọc lại để xác minh tiêu đề, rồi mới đưa bản gốc vào Thùng rác Windows (khôi phục được) và đặt bản mới vào
//!    chỗ. Lúc nào nội dung sách cũng còn nguyên ở ít nhất một nơi. Định dạng khác (PDF, MOBI/AZW, CBZ…) chỉ sửa
//!    ở lớp 1–2.
//!
//! Ghi vào nội dung làm đổi băm của file: quyết định "không trùng / giữ cả hai" đã ghi cho file đó được chuyển
//! sang dấu vân tay mới, nên nghi trùng đã duyệt không hiện lại.

use crate::config::Config;
use crate::db::{self, mtime_ms, now, Book};
use crate::error::{Error, Result};
use crate::formats::{self, Format};
use crate::moves::{move_file, unique_dest, MoveFailure};
use crate::normalize::normalize_title;
use crate::trash::Trasher;
use regex::Regex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
pub struct RetitleOptions {
    /// Đổi tên file theo tiêu đề mới.
    pub rename_file: bool,
    /// Ghi tiêu đề vào nội dung sách (chỉ EPUB / FB2; định dạng khác bỏ qua kèm ghi chú).
    pub write_content: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetitleDone {
    pub book_id: i64,
    pub old_path: String,
    pub new_path: String,
    pub renamed: bool,
    pub content_written: bool,
    /// Vì sao không ghi được vào nội dung dù được yêu cầu (định dạng chưa hỗ trợ, DRM, file lạ…).
    pub note: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct RetitleReport {
    pub batch_id: i64,
    pub title: String,
    pub done: Vec<RetitleDone>,
    pub failures: Vec<MoveFailure>,
}

/// Định dạng ghi được tiêu đề vào nội dung.
pub fn can_write_content(format: &str) -> bool {
    matches!(format, "epub" | "fb2")
}

/// Tiêu đề người dùng nhập: gộp khoảng trắng, bỏ ký tự điều khiển; rỗng → `None`.
pub fn clean_title(raw: &str) -> Option<String> {
    let s: String = raw.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s: String = s.chars().take(300).collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Tên file (không đuôi) hợp lệ trên Windows từ một tiêu đề: `:` → ` -`, `/ \ |` → `-`, bỏ `? * < > "`,
/// bỏ dấu chấm / khoảng trắng ở cuối và dấu chấm ở đầu (file ẩn), tối đa 150 ký tự, tránh tên dành riêng (CON, NUL…).
pub fn file_stem_for(title: &str) -> Option<String> {
    let mut s = String::with_capacity(title.len());
    for c in title.chars() {
        match c {
            ':' => s.push_str(" -"),
            '/' | '\\' | '|' => s.push('-'),
            '"' => s.push('\''),
            '?' | '*' | '<' | '>' => {}
            c if c.is_control() => s.push(' '),
            c => s.push(c),
        }
    }
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut s: String = s.chars().take(150).collect();
    s = s.trim_start_matches(['.', ' ']).trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() {
        return None;
    }
    let base = s.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit());
    if reserved {
        s.push('_');
    }
    Some(s)
}

fn ext_of(file_name: &str) -> &str {
    match file_name.rfind('.') {
        Some(i) if i > 0 => &file_name[i..],
        _ => "",
    }
}

/// Sửa tiêu đề cho các sách `ids` (vd. mọi định dạng của một tác phẩm). Lỗi từng cuốn không dừng cả lô.
pub fn retitle_books(
    conn: &Connection,
    cfg: &Config,
    trasher: &dyn Trasher,
    ids: &[i64],
    raw_title: &str,
    opts: RetitleOptions,
) -> Result<RetitleReport> {
    if cfg.read_only && (opts.rename_file || opts.write_content) {
        return Err(Error::ReadOnly);
    }
    let title = clean_title(raw_title).ok_or_else(|| Error::msg("Tiêu đề không được để trống"))?;
    let batch = db::new_batch(conn, "retitle", Some(&title))?;
    let mut rep = RetitleReport { batch_id: batch, title: title.clone(), ..Default::default() };
    for &id in ids {
        let Some(b) = db::get_book(conn, id)? else { continue };
        crate::progress::working_on(Path::new(&b.path));
        match retitle_one(conn, cfg, trasher, batch, &b, &title, opts) {
            Ok(d) => rep.done.push(d),
            Err(e) => {
                let msg = e.to_string();
                db::log_op(conn, batch, "retitle", Some(b.id), &b.path, None, Some(b.size), "failed", Some(&msg))?;
                rep.failures.push(MoveFailure { book_id: b.id, path: b.path.clone(), message: msg });
            }
        }
    }
    Ok(rep)
}

fn retitle_one(
    conn: &Connection,
    cfg: &Config,
    trasher: &dyn Trasher,
    batch: i64,
    b: &Book,
    title: &str,
    opts: RetitleOptions,
) -> Result<RetitleDone> {
    if b.status != "present" {
        return Err(Error::msg("Sách không còn trong thư viện"));
    }
    let src = PathBuf::from(&b.path);
    let dir = src.parent().ok_or_else(|| Error::msg("Đường dẫn không hợp lệ"))?.to_path_buf();
    let wants_file_change = opts.rename_file || opts.write_content;
    if wants_file_change && !src.is_file() {
        return Err(Error::msg("Không tìm thấy file – hãy quét lại thư viện"));
    }
    let new_name = match file_stem_for(title) {
        Some(stem) if opts.rename_file => format!("{stem}{}", ext_of(&b.file_name)),
        _ => b.file_name.clone(),
    };
    let mut note = None;
    let mut content_written = false;
    let mut dst = src.clone();

    let format = Format::parse(&b.format);
    let content_ok = opts.write_content && can_write_content(&b.format) && !b.drm;
    if opts.write_content && !content_ok {
        note = Some(if b.drm {
            "File có DRM – chỉ sửa tiêu đề trong thư viện và tên file".to_string()
        } else {
            format!(
                "{} chưa ghi được tiêu đề vào nội dung – chỉ sửa trong thư viện và tên file",
                b.format.to_uppercase()
            )
        });
    }
    if content_ok {
        let fmt = format.expect("epub/fb2");
        match rewrite_content(&src, &dir, fmt, title) {
            Ok(tmp) => {
                // Bản mới đã xác minh: bản gốc vào Thùng rác (khôi phục được), rồi đặt bản mới vào chỗ.
                if let Err(e) = trasher.trash(&src) {
                    if src.exists() {
                        let _ = std::fs::remove_file(&tmp);
                        return Err(Error::msg(format!("Không đưa được bản gốc vào Thùng rác: {e}")));
                    }
                    tracing::warn!("không chắc bản gốc {} đã vào Thùng rác: {e}", src.display());
                }
                dst = if new_name == b.file_name { src.clone() } else { unique_dest(&dir, &new_name) };
                if let Err(e) = std::fs::rename(&tmp, &dst) {
                    let back = std::fs::rename(&tmp, &src);
                    return Err(Error::msg(match back {
                        Ok(()) => format!("Không đặt được tên mới ({e}); bản đã sửa nằm ở {}", src.display()),
                        Err(_) => format!("Không đặt được bản đã sửa vào chỗ ({e}); bản đó nằm ở {}", tmp.display()),
                    }));
                }
                content_written = true;
            }
            Err(e) => {
                tracing::warn!("ghi tiêu đề vào {}: {e}", src.display());
                note = Some(format!("Không ghi được vào nội dung ({e}) – chỉ sửa trong thư viện và tên file"));
            }
        }
    }
    if !content_written && new_name != b.file_name {
        dst = if new_name.to_lowercase() == b.file_name.to_lowercase() {
            // Chỉ đổi hoa / thường: trên Windows `exists()` thấy chính nó → đổi qua một tên tạm.
            let tmp = dir.join(format!(".~{}.librarian-tmp", b.id));
            move_file(&src, &tmp).map_err(|e| Error::msg(format!("Không đổi tên được: {e}")))?;
            let d = dir.join(&new_name);
            if let Err(e) = move_file(&tmp, &d) {
                let _ = move_file(&tmp, &src);
                return Err(Error::msg(format!("Không đổi tên được: {e}")));
            }
            d
        } else {
            let d = unique_dest(&dir, &new_name);
            move_file(&src, &d).map_err(|e| Error::msg(format!("Không đổi tên được: {e}")))?;
            d
        };
    }

    let old_fp = crate::dedupe::fingerprint_of(b);
    let tx = db::write_tx(conn)?;
    if dst != src {
        crate::scan::relocate(&tx, cfg, b.id, &dst)?;
    }
    if content_written || dst != src {
        if let Ok(m) = std::fs::metadata(&dst) {
            let mt = m.modified().map(mtime_ms).unwrap_or(b.mtime);
            if content_written {
                tx.execute(
                    "UPDATE books SET size = ?2, mtime = ?3, quick_hash = NULL, full_hash = NULL, hash_size = NULL,
                       hash_mtime = NULL WHERE id = ?1",
                    params![b.id, m.len() as i64, mt],
                )?;
            } else {
                tx.execute(
                    "UPDATE books SET mtime = ?2, hash_mtime = CASE WHEN hash_mtime = ?3 THEN ?2 ELSE hash_mtime END
                     WHERE id = ?1",
                    params![b.id, mt, b.mtime],
                )?;
            }
        }
    }
    let stem = dst.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut norm = normalize_title(title);
    if norm.is_empty() {
        norm = normalize_title(&stem);
    }
    tx.execute(
        "UPDATE books SET title = ?2, user_title = ?2, title_from_meta = 1, norm_title = ?3, updated_at = ?4 WHERE id = ?1",
        params![b.id, title, norm, now()],
    )?;
    db::fts_upsert(&tx, b.id)?;
    let dst_s = dst.to_string_lossy().into_owned();
    let what = match (content_written, dst != src) {
        (true, _) => "ghi tiêu đề vào nội dung; bản gốc vào Thùng rác",
        (false, true) => "đổi tên file",
        (false, false) => "chỉ sửa tiêu đề trong thư viện",
    };
    let msg = format!("“{}” → “{title}”: {what}", b.title);
    db::log_op(&tx, batch, "retitle", Some(b.id), &b.path, Some(&dst_s), Some(b.size), "done", Some(&msg))?;
    tx.commit()?;

    if content_written {
        // Nội dung đổi → băm đổi: chuyển các quyết định duyệt nghi trùng của file này sang dấu vân tay mới.
        if let (Some(old), Some(q)) = (old_fp, crate::hash::ensure_quick(conn, b.id)?) {
            let size: i64 = conn.query_row("SELECT size FROM books WHERE id = ?1", [b.id], |r| r.get(0))?;
            crate::dedupe::replace_fingerprint(conn, &old, &format!("q:{size}:{q}"))?;
        }
    }
    Ok(RetitleDone {
        book_id: b.id,
        old_path: b.path.clone(),
        new_path: dst_s,
        renamed: dst != src,
        content_written,
        note,
    })
}

// ------------------------------------------------------------------ ghi vào nội dung

/// Ghi bản đã sửa ra file tạm (ẩn, cùng thư mục), đọc lại để xác minh. Trả đường dẫn file tạm.
fn rewrite_content(src: &Path, dir: &Path, fmt: Format, title: &str) -> std::result::Result<PathBuf, String> {
    let tmp = dir.join(format!(
        ".~{}.librarian-tmp",
        src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    let _ = std::fs::remove_file(&tmp); // sót từ lần trước bị ngắt
    let r = match fmt {
        Format::Epub => epub_set_title(src, &tmp, title),
        Format::Fb2 => fb2_set_title(src, &tmp, title),
        _ => Err("định dạng chưa hỗ trợ".into()),
    }
    .and_then(|()| {
        let md = formats::extract(&tmp, fmt).map_err(|e| format!("bản đã sửa đọc lỗi: {e}"))?;
        if md.title.as_deref() == Some(title) {
            Ok(())
        } else {
            Err(format!("đọc lại được tiêu đề {:?}", md.title))
        }
    });
    match r {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn re(cell: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(p).unwrap())
}

/// Thay nội dung `<dc:title>` đầu tiên trong `<metadata>` của OPF (đúng phần trình đọc lấy làm tiêu đề);
/// chưa có thì thêm một `<dc:title>` (cần OPF đã khai báo tiền tố `dc`).
pub fn set_opf_title(opf: &str, title: &str) -> std::result::Result<String, String> {
    static META: OnceLock<Regex> = OnceLock::new();
    static TITLE: OnceLock<Regex> = OnceLock::new();
    let meta = re(&META, r"(?s)<(?:[\w.-]+:)?metadata\b[^>]*>(.*?)</(?:[\w.-]+:)?metadata\s*>")
        .captures(opf)
        .and_then(|c| c.get(1))
        .ok_or("OPF thiếu <metadata>")?;
    let body = meta.as_str();
    let esc = xml_escape(title);
    if let Some(c) = re(&TITLE, r"(?s)<([\w.-]+:)?title(\s[^>]*)?>(.*?)</([\w.-]+:)?title\s*>").captures(body) {
        let (open_p, close_p) = (c.get(1).map(|m| m.as_str()), c.get(4).map(|m| m.as_str()));
        if open_p != close_p {
            return Err("thẻ title trong OPF không khớp".into());
        }
        let inner = c.get(3).unwrap();
        let (a, b) = (meta.start() + inner.start(), meta.start() + inner.end());
        return Ok(format!("{}{esc}{}", &opf[..a], &opf[b..]));
    }
    if !opf.contains("xmlns:dc=") {
        return Err("OPF không có dc:title".into());
    }
    let at = meta.start();
    Ok(format!("{}\n    <dc:title>{esc}</dc:title>{}", &opf[..at], &opf[at..]))
}

fn epub_set_title(src: &Path, dst: &Path, title: &str) -> std::result::Result<(), String> {
    use zip::write::SimpleFileOptions;
    const LIMIT: u64 = 4 * 1024 * 1024;
    let f = std::fs::File::open(src).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(BufReader::new(f)).map_err(|e| format!("zip hỏng: {e}"))?;
    let container = formats::decode_utf8(&formats::read_zip_entry(&mut zip, "META-INF/container.xml", LIMIT)?);
    let opf_path = formats::epub::opf_path(&container)?;
    let raw = formats::read_zip_entry(&mut zip, &opf_path, LIMIT)?;
    let raw = raw.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&raw);
    let opf = std::str::from_utf8(raw).map_err(|_| "OPF không phải UTF-8".to_string())?;
    let new_opf = set_opf_title(opf, title)?;

    let out = std::fs::File::create_new(dst).map_err(|e| e.to_string())?;
    let mut w = zip::ZipWriter::new(BufWriter::new(out));
    let r = (|| -> zip::result::ZipResult<()> {
        for i in 0..zip.len() {
            crate::progress::pulse();
            let e = zip.by_index_raw(i)?;
            if e.name() == opf_path {
                let mut o = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
                if let Some(t) = e.last_modified() {
                    o = o.last_modified_time(t);
                }
                drop(e);
                w.start_file(opf_path.as_str(), o)?;
                w.write_all(new_opf.as_bytes())?;
            } else {
                // giữ nguyên byte nén, thứ tự (mimetype đầu tiên, không nén) và thuộc tính của mọi mục khác
                w.raw_copy_file(e)?;
            }
        }
        let mut bw = w.finish()?;
        bw.flush()?;
        bw.get_ref().sync_all()?;
        Ok(())
    })();
    if let Err(e) = r {
        let _ = std::fs::remove_file(dst);
        return Err(format!("ghi EPUB lỗi: {e}"));
    }
    Ok(())
}

/// Thay `<book-title>` trong `<title-info>`; các byte khác giữ nguyên.
pub fn set_fb2_title(text: &str, title: &str) -> std::result::Result<String, String> {
    static TI: OnceLock<Regex> = OnceLock::new();
    static BT: OnceLock<Regex> = OnceLock::new();
    let ti = re(&TI, r"(?s)<title-info>(.*?)</title-info>")
        .captures(text)
        .and_then(|c| c.get(1))
        .ok_or("thiếu <title-info>")?;
    let c =
        re(&BT, r"(?s)<book-title(?:\s[^>]*)?>(.*?)</book-title>").captures(ti.as_str()).ok_or("thiếu <book-title>")?;
    let inner = c.get(1).unwrap();
    let (a, b) = (ti.start() + inner.start(), ti.start() + inner.end());
    Ok(format!("{}{}{}", &text[..a], xml_escape(title), &text[b..]))
}

fn fb2_set_title(src: &Path, dst: &Path, title: &str) -> std::result::Result<(), String> {
    let mut buf = Vec::new();
    std::fs::File::open(src).and_then(|mut f| f.read_to_end(&mut buf)).map_err(|e| e.to_string())?;
    let head = String::from_utf8_lossy(&buf[..buf.len().min(200)]).to_ascii_lowercase();
    if head.contains("windows-1251") {
        return Err("FB2 mã windows-1251 – chưa hỗ trợ ghi".into());
    }
    let (bom, body) = match buf.strip_prefix(b"\xEF\xBB\xBF") {
        Some(rest) => (&b"\xEF\xBB\xBF"[..], rest),
        None => (&b""[..], &buf[..]),
    };
    let text = std::str::from_utf8(body).map_err(|_| "FB2 không phải UTF-8".to_string())?;
    let new_text = set_fb2_title(text, title)?;
    let r = std::fs::File::create_new(dst).and_then(|mut f| {
        f.write_all(bom)?;
        f.write_all(new_text.as_bytes())?;
        f.sync_all()
    });
    if let Err(e) = r {
        let _ = std::fs::remove_file(dst);
        return Err(e.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems() {
        assert_eq!(file_stem_for("Harry Potter: Hòn đá phù thuỷ").as_deref(), Some("Harry Potter - Hòn đá phù thuỷ"));
        assert_eq!(file_stem_for("A/B\\C|D").as_deref(), Some("A-B-C-D"));
        assert_eq!(file_stem_for("Why? *Now* <x> \"q\"").as_deref(), Some("Why Now x 'q'"));
        assert_eq!(file_stem_for("...hidden...").as_deref(), Some("hidden"));
        assert_eq!(file_stem_for("CON").as_deref(), Some("CON_"));
        assert_eq!(file_stem_for("com1").as_deref(), Some("com1_"));
        assert_eq!(file_stem_for("Comic").as_deref(), Some("Comic"));
        assert_eq!(file_stem_for("???"), None);
        assert_eq!(file_stem_for(&"x".repeat(400)).unwrap().chars().count(), 150);
    }

    #[test]
    fn titles() {
        assert_eq!(clean_title("  Mắt \t Biếc \n").as_deref(), Some("Mắt Biếc"));
        assert_eq!(clean_title("   "), None);
    }

    #[test]
    fn opf_title_replaced_and_inserted() {
        let opf = r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:title id="t">Old &amp; tired</dc:title><dc:creator>A</dc:creator></metadata><guide><reference title="Cover"/></guide></package>"#;
        let out = set_opf_title(opf, "Mới & <đẹp>").unwrap();
        assert!(out.contains(r#"<dc:title id="t">Mới &amp; &lt;đẹp&gt;</dc:title>"#), "{out}");
        assert_eq!(formats::epub::parse_opf(&out).unwrap().title.as_deref(), Some("Mới & <đẹp>"));
        assert!(out.contains(r#"<reference title="Cover"/>"#));

        let none = r#"<package><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:creator>A</dc:creator></metadata></package>"#;
        let out = set_opf_title(none, "X").unwrap();
        assert_eq!(formats::epub::parse_opf(&out).unwrap().title.as_deref(), Some("X"));
    }

    #[test]
    fn fb2_title_replaced() {
        let t = r#"<?xml version="1.0" encoding="utf-8"?><FictionBook><description><title-info><genre>prose</genre>
<book-title>Cũ</book-title><lang>vi</lang></title-info><publish-info><book-name>Giữ</book-name></publish-info></description></FictionBook>"#;
        let out = set_fb2_title(t, "Dế Mèn & bạn").unwrap();
        assert_eq!(formats::fb2::parse(&out).title.as_deref(), Some("Dế Mèn & bạn"));
        assert!(out.contains("<book-name>Giữ</book-name>"));
    }
}
