//! Sửa thông tin sách (tiêu đề, tác giả, tag, bộ + số tập, ngôn ngữ, ISBN) trong app.
//!
//! - **Thư viện (DB)**: luôn sửa. Giá trị người dùng nhập được lưu ở `user_meta` và thắng thông tin đọc từ file ở
//!   mọi lần quét sau – kể cả với định dạng không ghi được vào file.
//! - **Nội dung file** (khi chọn và định dạng hỗ trợ): EPUB (OPF: `dc:title`, `dc:creator`, `dc:subject`,
//!   `dc:language`, `dc:identifier` ISBN, `calibre:series` / `calibre:series_index`, `belongs-to-collection`) và FB2
//!   UTF-8 (`<title-info>`: `book-title`, `author`, `genre`, `lang`, `sequence`). Ghi ra file tạm cạnh bản gốc, đọc
//!   lại để xác minh, rồi mới đưa bản gốc vào Thùng rác (khôi phục được) và đặt bản mới vào chỗ – lúc nào nội dung
//!   sách cũng còn nguyên ở ít nhất một nơi. File có DRM: chỉ sửa trong thư viện.
//! - PDF, MOBI/AZW, CBZ/CBR, DJVU, TXT: chỉ sửa trong thư viện. (PDF / MOBI về kỹ thuật ghi được – PDF bằng bản cập
//!   nhật nối thêm, MOBI bằng bản ghi EXTH – nhưng dễ làm hỏng file hơn hẳn; chưa làm.)

use crate::config::Config;
use crate::db::{self, mtime_ms, now, Book};
use crate::error::{Error, Result};
use crate::formats::{self, Format};
use crate::normalize::{normalize_author, normalize_title};
use crate::trash::Trasher;
use regex::Regex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// Lô trong Hoạt động (không hoàn tác theo lô – bản gốc nằm trong Thùng rác nếu đã ghi vào file).
pub const KIND: &str = "metaedit";

/// Thông tin người dùng nhập (cả form – trường trống = không có).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetaEdit {
    pub title: String,
    pub authors: Vec<String>,
    pub tags: Vec<String>,
    pub series: String,
    pub volume: Option<u32>,
    pub language: String,
    pub isbn: String,
}

impl MetaEdit {
    /// Gọn khoảng trắng, bỏ mục rỗng / trùng.
    pub fn cleaned(&self) -> Result<MetaEdit> {
        let one = |s: &str| {
            let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
            s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect::<String>()
        };
        let list = |v: &[String]| {
            let mut out: Vec<String> = Vec::new();
            for x in v.iter().flat_map(|x| x.split([';', '\n'])).map(one).filter(|x| !x.is_empty()) {
                if !out.iter().any(|o| o.to_lowercase() == x.to_lowercase()) {
                    out.push(x);
                }
            }
            out
        };
        let m = MetaEdit {
            title: one(&self.title),
            authors: list(&self.authors),
            tags: list(&self.tags),
            series: one(&self.series),
            volume: self.volume.filter(|_| !self.series.trim().is_empty()),
            language: one(&self.language),
            isbn: one(&self.isbn),
        };
        if m.title.is_empty() {
            return Err(Error::msg("Tiêu đề không được để trống"));
        }
        if !m.isbn.is_empty() && formats::normalize_isbn(&m.isbn).is_none() {
            return Err(Error::msg(format!("ISBN “{}” không hợp lệ", m.isbn)));
        }
        Ok(m)
    }
}

/// Định dạng ghi được thông tin vào nội dung file.
pub fn can_write(format: &str) -> bool {
    matches!(format, "epub" | "fb2")
}

#[derive(Debug, Clone, Serialize)]
pub struct EditReport {
    pub book_id: i64,
    /// Đã ghi vào nội dung file (bản gốc vào Thùng rác).
    pub written: bool,
    /// Vì sao chỉ sửa trong thư viện (định dạng chưa hỗ trợ, DRM, file lạ…).
    pub note: Option<String>,
}

/// Sửa thông tin cuốn `id`. `write_file`: ghi luôn vào nội dung file nếu định dạng hỗ trợ.
pub fn edit(
    conn: &Connection,
    cfg: &Config,
    trasher: &dyn Trasher,
    id: i64,
    raw: &MetaEdit,
    write_file: bool,
) -> Result<EditReport> {
    let m = raw.cleaned()?;
    let b = db::get_book(conn, id)?.ok_or_else(|| Error::msg("Không có sách này"))?;
    if b.status != "present" {
        return Err(Error::msg("Sách không còn trong thư viện"));
    }
    if write_file && cfg.read_only {
        return Err(Error::ReadOnly);
    }
    let src = PathBuf::from(&b.path);
    let mut rep = EditReport { book_id: id, written: false, note: None };
    if write_file {
        if b.drm {
            rep.note = Some("File có DRM – chỉ sửa trong thư viện".into());
        } else if !can_write(&b.format) {
            rep.note = Some(format!("{} chưa ghi được vào file – chỉ sửa trong thư viện", b.format.to_uppercase()));
        } else if !src.is_file() {
            return Err(Error::msg("Không tìm thấy file – hãy quét lại thư viện"));
        } else {
            match rewrite(&src, Format::parse(&b.format).expect("epub/fb2"), &m) {
                Ok(tmp) => {
                    if let Err(e) = trasher.trash(&src) {
                        if src.exists() {
                            let _ = std::fs::remove_file(&tmp);
                            return Err(Error::msg(format!("Không đưa được bản gốc vào Thùng rác: {e}")));
                        }
                    }
                    if let Err(e) = std::fs::rename(&tmp, &src) {
                        return Err(Error::msg(format!(
                            "Không đặt được bản đã sửa vào chỗ ({e}); bản đó nằm ở {}, bản gốc trong Thùng rác",
                            tmp.display()
                        )));
                    }
                    rep.written = true;
                }
                Err(e) => {
                    tracing::warn!("ghi thông tin vào {}: {e}", src.display());
                    rep.note = Some(format!("Không ghi được vào file ({e}) – chỉ sửa trong thư viện"));
                }
            }
        }
    }
    let old_fp = crate::dedupe::fingerprint_of(&b);
    let tx = db::write_tx(conn)?;
    let batch = db::new_batch(&tx, KIND, Some(&m.title))?;
    if rep.written {
        if let Ok(md) = std::fs::metadata(&src) {
            let mt = md.modified().map(mtime_ms).unwrap_or(b.mtime);
            tx.execute(
                "UPDATE books SET size = ?2, mtime = ?3, quick_hash = NULL, full_hash = NULL, hash_size = NULL,
                   hash_mtime = NULL WHERE id = ?1",
                params![id, md.len() as i64, mt],
            )?;
        }
    }
    apply_db(&tx, id, &m)?;
    tx.execute("UPDATE books SET user_meta = ?2 WHERE id = ?1", params![id, serde_json::to_string(&m)?])?;
    let what =
        if rep.written { "ghi vào file; bản gốc vào Thùng rác" } else { "chỉ sửa trong thư viện" };
    db::log_op(&tx, batch, KIND, Some(id), &b.path, Some(&b.path), Some(b.size), "done", Some(what))?;
    tx.commit()?;
    if rep.written {
        if let (Some(old), Some(q)) = (old_fp, crate::hash::ensure_quick(conn, id)?) {
            let size: i64 = conn.query_row("SELECT size FROM books WHERE id = ?1", [id], |r| r.get(0))?;
            crate::dedupe::replace_fingerprint(conn, &old, &format!("q:{size}:{q}"))?;
        }
    }
    Ok(rep)
}

/// Ghi `m` vào các cột của sách trong DB (không đụng file).
fn apply_db(conn: &Connection, id: i64, m: &MetaEdit) -> Result<()> {
    let mut norm = normalize_title(&m.title);
    if norm.is_empty() {
        norm = crate::normalize::simple_key(&m.title);
    }
    let isbn = formats::normalize_isbn(&m.isbn);
    conn.execute(
        "UPDATE books SET title = ?2, user_title = ?2, title_from_meta = 1, norm_title = ?3, authors = ?4,
           norm_author = ?5, subjects = ?6, series = ?7, volume = ?8, language = ?9, isbn = ?10, updated_at = ?11
         WHERE id = ?1",
        params![
            id,
            m.title,
            norm,
            m.authors.join("; "),
            normalize_author(&m.authors.join(" ")),
            m.tags.join("; "),
            (!m.series.is_empty()).then_some(&m.series),
            m.volume.map(|v| v as i64),
            (!m.language.is_empty()).then_some(&m.language),
            isbn,
            now()
        ],
    )?;
    db::fts_upsert(conn, id)
}

/// Lần quét đọc lại file: thông tin người dùng đã sửa (`user_meta`) thắng thông tin trong file.
pub(crate) fn reapply(conn: &Connection, id: i64) -> Result<()> {
    let raw: Option<String> = conn.query_row("SELECT user_meta FROM books WHERE id = ?1", [id], |r| r.get(0))?;
    let Some(m) = raw.and_then(|s| serde_json::from_str::<MetaEdit>(&s).ok()) else { return Ok(()) };
    apply_db(conn, id, &m)
}

/// Thông tin đang có của một cuốn, để điền sẵn vào form.
pub fn current(b: &Book) -> MetaEdit {
    let split = |s: &str| s.split(';').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>();
    MetaEdit {
        title: b.title.clone(),
        authors: split(&b.authors),
        tags: split(&b.subjects),
        series: b.series.clone().unwrap_or_default(),
        volume: b.volume.and_then(|v| u32::try_from(v).ok()),
        language: b.language.clone().unwrap_or_default(),
        isbn: b.isbn.clone().unwrap_or_default(),
    }
}

// ------------------------------------------------------------------ ghi vào file

fn tmp_of(src: &Path) -> PathBuf {
    let name = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    src.with_file_name(format!(".~{name}.librarian-tmp"))
}

/// Ghi bản đã sửa ra file tạm cạnh `src`, đọc lại để xác minh. Trả đường dẫn file tạm.
fn rewrite(src: &Path, fmt: Format, m: &MetaEdit) -> std::result::Result<PathBuf, String> {
    let tmp = tmp_of(src);
    let _ = std::fs::remove_file(&tmp);
    let r = match fmt {
        Format::Epub => epub_rewrite(src, &tmp, m),
        Format::Fb2 => fb2_rewrite(src, &tmp, m),
        _ => Err("định dạng chưa hỗ trợ".into()),
    }
    .and_then(|()| {
        let md = formats::extract(&tmp, fmt).map_err(|e| format!("bản đã sửa đọc lỗi: {e}"))?;
        let same = |a: &[String], b: &[String]| {
            let k = |v: &[String]| {
                let mut v: Vec<String> = v.iter().map(|x| normalize_author(x)).collect();
                v.sort();
                v
            };
            k(a) == k(b)
        };
        if md.title.as_deref() != Some(m.title.as_str()) {
            return Err(format!("đọc lại được tiêu đề {:?}", md.title));
        }
        if !same(&md.authors, &m.authors) {
            return Err(format!("đọc lại được tác giả {:?}", md.authors));
        }
        if !same(&md.subjects, &m.tags) {
            return Err(format!("đọc lại được tag {:?}", md.subjects));
        }
        if md.series.as_deref().unwrap_or("") != m.series {
            return Err(format!("đọc lại được bộ {:?}", md.series));
        }
        Ok(())
    });
    match r {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn rx(p: &str) -> Regex {
    Regex::new(p).expect("regex hợp lệ")
}

/// Sửa phần `<metadata>` của OPF theo `m` (xem mô tả module). Các thẻ khác giữ nguyên từng byte.
pub fn set_opf_metadata(opf: &str, m: &MetaEdit) -> std::result::Result<String, String> {
    let meta_re = rx(r"(?s)(<(?:[\w.-]+:)?metadata\b[^>]*>)(.*?)(</(?:[\w.-]+:)?metadata\s*>)");
    let c = meta_re.captures(opf).ok_or("OPF thiếu <metadata>")?;
    let (whole, open, body, close) = (c.get(0).unwrap(), c.get(1).unwrap(), c.get(2).unwrap(), c.get(3).unwrap());
    let mut body = body.as_str().to_string();
    // Tiền tố Dublin Core đang dùng (`dc:` thường gặp; có file dùng tiền tố khác hoặc không tiền tố).
    let prefix = rx(r"<([\w.-]+:)?title\b")
        .captures(&body)
        .map(|c| c.get(1).map(|p| p.as_str().to_string()).unwrap_or_default())
        .unwrap_or_else(|| "dc:".to_string());
    if prefix == "dc:" && !opf.contains("xmlns:dc=") {
        return Err("OPF không khai báo tiền tố dc".into());
    }
    let el = |name: &str| rx(&format!(r"(?s)\s*<(?:[\w.-]+:)?{name}\b([^>]*?)(?:/>|>(.*?)</(?:[\w.-]+:)?{name}\s*>)"));
    // Tiêu đề: thay nội dung thẻ đầu tiên.
    body = crate::retitle::set_opf_title(&format!("{}{}{}", open.as_str(), body, close.as_str()), &m.title)
        .map(|s| s[open.as_str().len()..s.len() - close.as_str().len()].to_string())?;
    // Tác giả: bỏ các creator vai trò tác giả (hoặc không ghi vai trò), giữ biên tập / dịch giả…
    let role_re = rx(r#"role\s*=\s*"([^"]*)""#);
    body = el("creator")
        .replace_all(&body, |c: &regex::Captures| {
            let attrs = c.get(1).map_or("", |a| a.as_str());
            match role_re.captures(attrs) {
                Some(r) if &r[1] != "aut" => c[0].to_string(),
                _ => String::new(),
            }
        })
        .into_owned();
    body = el("subject").replace_all(&body, "").into_owned();
    body = el("language").replace_all(&body, "").into_owned();
    body = rx(r#"(?s)\s*<meta\b[^>]*name\s*=\s*"calibre:series(?:_index)?"[^>]*/?>(?:\s*</meta>)?"#)
        .replace_all(&body, "")
        .into_owned();
    // EPUB 3: tên bộ trong `belongs-to-collection` – thay nội dung (hoặc bỏ nếu xoá bộ).
    let coll = rx(r#"(?s)(<meta\b[^>]*property\s*=\s*"belongs-to-collection"[^>]*>)(.*?)(</meta\s*>)"#);
    body = if m.series.is_empty() {
        coll.replace_all(&body, "").into_owned()
    } else {
        coll.replace_all(&body, |c: &regex::Captures| format!("{}{}{}", &c[1], esc(&m.series), &c[3])).into_owned()
    };
    // ISBN: thay identifier ISBN có sẵn, không có thì thêm.
    let mut isbn_done = m.isbn.is_empty();
    if !isbn_done {
        let id_re = rx(r"(?s)(<(?:[\w.-]+:)?identifier\b[^>]*>)(.*?)(</(?:[\w.-]+:)?identifier\s*>)");
        body = id_re
            .replace_all(&body, |c: &regex::Captures| {
                let is_isbn = c[1].to_ascii_lowercase().contains("isbn")
                    || c[2].to_ascii_lowercase().contains("isbn")
                    || formats::normalize_isbn(&c[2]).is_some();
                if is_isbn && !isbn_done {
                    isbn_done = true;
                    let v = if c[2].to_ascii_lowercase().starts_with("urn:isbn:") {
                        format!("urn:isbn:{}", esc(&m.isbn))
                    } else {
                        esc(&m.isbn)
                    };
                    format!("{}{v}{}", &c[1], &c[3])
                } else {
                    c[0].to_string()
                }
            })
            .into_owned();
    }
    let mut add = String::new();
    let p = &prefix;
    for a in &m.authors {
        add.push_str(&format!("\n    <{p}creator>{}</{p}creator>", esc(a)));
    }
    for t in &m.tags {
        add.push_str(&format!("\n    <{p}subject>{}</{p}subject>", esc(t)));
    }
    if !m.language.is_empty() {
        add.push_str(&format!("\n    <{p}language>{}</{p}language>", esc(&m.language)));
    }
    if !isbn_done {
        add.push_str(&format!("\n    <{p}identifier>urn:isbn:{}</{p}identifier>", esc(&m.isbn)));
    }
    if !m.series.is_empty() {
        add.push_str(&format!("\n    <meta name=\"calibre:series\" content=\"{}\"/>", esc(&m.series)));
        if let Some(v) = m.volume {
            add.push_str(&format!("\n    <meta name=\"calibre:series_index\" content=\"{v}\"/>"));
        }
    }
    let body = format!("{}{add}\n  ", body.trim_end());
    Ok(format!("{}{}{}{}{}", &opf[..whole.start()], open.as_str(), body, close.as_str(), &opf[whole.end()..]))
}

fn epub_rewrite(src: &Path, dst: &Path, m: &MetaEdit) -> std::result::Result<(), String> {
    use zip::write::SimpleFileOptions;
    const LIMIT: u64 = 4 * 1024 * 1024;
    let f = std::fs::File::open(src).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(BufReader::new(f)).map_err(|e| format!("zip hỏng: {e}"))?;
    let container = formats::decode_utf8(&formats::read_zip_entry(&mut zip, "META-INF/container.xml", LIMIT)?);
    let opf_path = formats::epub::opf_path(&container)?;
    let raw = formats::read_zip_entry(&mut zip, &opf_path, LIMIT)?;
    let raw = raw.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&raw);
    let opf = std::str::from_utf8(raw).map_err(|_| "OPF không phải UTF-8".to_string())?;
    let new_opf = set_opf_metadata(opf, m)?;
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

/// Tên một người thành `<first-name>` + `<last-name>` (từ cuối là `last-name`; đọc lại ghép đúng thứ tự cũ).
fn fb2_author(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    match words.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!(
            "<author><first-name>{}</first-name><last-name>{}</last-name></author>",
            esc(&rest.join(" ")),
            esc(last)
        ),
        _ => format!("<author><first-name>{}</first-name></author>", esc(name)),
    }
}

/// Sửa `<title-info>` của FB2 theo `m`; các byte khác giữ nguyên.
pub fn set_fb2_metadata(text: &str, m: &MetaEdit) -> std::result::Result<String, String> {
    let ti_re = rx(r"(?s)(<title-info>)(.*?)(</title-info>)");
    let c = ti_re.captures(text).ok_or("thiếu <title-info>")?;
    let (whole, body) = (c.get(0).unwrap(), c.get(2).unwrap().as_str());
    let mut body = body.to_string();
    for name in ["genre", "author", "book-title", "lang", "sequence"] {
        body = rx(&format!(r"(?s)\s*<{name}\b[^>]*?(?:/>|>.*?</{name}>)")).replace_all(&body, "").into_owned();
    }
    let mut head = String::new();
    for t in &m.tags {
        head.push_str(&format!("\n<genre>{}</genre>", esc(t)));
    }
    for a in &m.authors {
        head.push_str(&format!("\n{}", fb2_author(a)));
    }
    head.push_str(&format!("\n<book-title>{}</book-title>", esc(&m.title)));
    let mut tail = String::new();
    if !m.language.is_empty() {
        tail.push_str(&format!("\n<lang>{}</lang>", esc(&m.language)));
    }
    if !m.series.is_empty() {
        match m.volume {
            Some(v) => tail.push_str(&format!("\n<sequence name=\"{}\" number=\"{v}\"/>", esc(&m.series))),
            None => tail.push_str(&format!("\n<sequence name=\"{}\"/>", esc(&m.series))),
        }
    }
    Ok(format!(
        "{}<title-info>{head}{}{tail}\n</title-info>{}",
        &text[..whole.start()],
        body.trim_end(),
        &text[whole.end()..]
    ))
}

fn fb2_rewrite(src: &Path, dst: &Path, m: &MetaEdit) -> std::result::Result<(), String> {
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
    let new_text = set_fb2_metadata(text, m)?;
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

    fn me() -> MetaEdit {
        MetaEdit {
            title: "Mắt biếc & em".into(),
            authors: vec!["Nguyễn Nhật Ánh".into(), "Tác giả Hai".into()],
            tags: vec!["Văn học".into(), "Tuổi học trò".into()],
            series: "Truyện dài".into(),
            volume: Some(3),
            language: "vi".into(),
            isbn: "978-604-1-00000-1".into(),
        }
    }

    #[test]
    fn opf_round_trips_and_keeps_other_roles() {
        let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf" version="2.0">
  <metadata>
    <dc:title>Cũ</dc:title>
    <dc:creator opf:role="aut">Ai đó</dc:creator>
    <dc:creator opf:role="trl">Dịch giả</dc:creator>
    <dc:subject>Cũ</dc:subject>
    <dc:language>en</dc:language>
    <dc:identifier opf:scheme="uuid">urn:uuid:1</dc:identifier>
    <meta name="calibre:series" content="Bộ cũ"/>
    <meta name="cover" content="c"/>
  </metadata>
  <manifest/>
</package>"#;
        let out = set_opf_metadata(opf, &me()).unwrap();
        let md = formats::epub::parse_opf(&out).unwrap();
        assert_eq!(md.title.as_deref(), Some("Mắt biếc & em"));
        assert_eq!(md.authors, vec!["Nguyễn Nhật Ánh", "Tác giả Hai"]);
        assert_eq!(md.subjects, vec!["Văn học", "Tuổi học trò"]);
        assert_eq!(md.language.as_deref(), Some("vi"));
        assert_eq!(md.series.as_deref(), Some("Truyện dài"));
        assert_eq!(md.volume, Some(3));
        assert_eq!(md.isbn.as_deref(), Some("9786041000001"));
        assert!(out.contains("Dịch giả") && out.contains(r#"name="cover""#) && out.contains("urn:uuid:1"));
        // Xoá bộ / tag.
        let out = set_opf_metadata(&out, &MetaEdit { series: String::new(), tags: vec![], ..me() }).unwrap();
        let md = formats::epub::parse_opf(&out).unwrap();
        assert_eq!((md.series, md.subjects.len()), (None, 0));
    }

    #[test]
    fn fb2_round_trips() {
        let fb2 = "<?xml version=\"1.0\" encoding=\"utf-8\"?><FictionBook><description><title-info><genre>prose</genre><author><first-name>A</first-name><last-name>B</last-name></author><book-title>Cũ</book-title><annotation><p>x</p></annotation><lang>ru</lang></title-info></description><body/></FictionBook>";
        let out = set_fb2_metadata(fb2, &me()).unwrap();
        let md = formats::fb2::parse(&out);
        assert_eq!(md.title.as_deref(), Some("Mắt biếc & em"));
        assert_eq!(md.authors, vec!["Nguyễn Nhật Ánh", "Tác giả Hai"]);
        assert_eq!(md.subjects, vec!["Văn học", "Tuổi học trò"]);
        assert_eq!((md.series.as_deref(), md.volume), (Some("Truyện dài"), Some(3)));
        assert_eq!(md.language.as_deref(), Some("vi"));
        assert!(out.contains("<annotation><p>x</p></annotation>"));
    }

    #[test]
    fn input_is_cleaned() {
        let m = MetaEdit { title: "  a  b ".into(), authors: vec!["X; Y".into(), "x".into()], ..Default::default() };
        let c = m.cleaned().unwrap();
        assert_eq!((c.title.as_str(), c.authors.clone()), ("a b", vec!["X".to_string(), "Y".to_string()]));
        assert!(MetaEdit::default().cleaned().is_err());
        assert!(MetaEdit { title: "t".into(), isbn: "123".into(), ..Default::default() }.cleaned().is_err());
    }
}
