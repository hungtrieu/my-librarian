//! Ảnh bìa (E2-07): EPUB (cover trong OPF), MOBI/AZW3 (record ảnh bìa EXTH 201), CBZ/CBR zip & RAR (ảnh đầu tiên),
//! FB2 (coverpage). Thumbnail JPEG lưu cache ở `<home>/covers`, sinh lười khi UI cần.
//! PDF/DJVU chưa render (E2-08) → UI dùng bìa mặc định (E2-09).

use crate::formats::{self, comic::is_image_name, Format};
use crate::normalize::natural_cmp;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const MAX_IMAGE: u64 = 40 * 1024 * 1024;
pub const THUMB_W: u32 = 360;
pub const THUMB_H: u32 = 540;

fn looks_like_image(b: &[u8]) -> bool {
    b.starts_with(&[0xFF, 0xD8])
        || b.starts_with(b"\x89PNG")
        || b.starts_with(b"GIF8")
        || (b.len() > 12 && &b[8..12] == b"WEBP")
        || b.starts_with(b"BM")
}

/// Ảnh bìa gốc (chưa thu nhỏ). `None` = định dạng không có/không lấy được bìa.
pub fn extract_cover(path: &Path, format: Format) -> Option<Vec<u8>> {
    let r = match format {
        Format::Epub => epub_cover(path),
        Format::Mobi | Format::Azw | Format::Azw3 => mobi_cover(path),
        Format::Cbz => zip_first_image(path),
        Format::Cbr => {
            let mut m = [0u8; 4];
            File::open(path).ok()?.read_exact(&mut m).ok()?;
            if &m == b"PK\x03\x04" {
                zip_first_image(path)
            } else if &m == b"Rar!" {
                rar_first_image(path)
            } else {
                None
            }
        }
        Format::Fb2 => fb2_cover(path),
        _ => None,
    };
    r.filter(|b| looks_like_image(b))
}

fn rar_first_image(path: &Path) -> Option<Vec<u8>> {
    let first = crate::reader::rar_image_names(path).ok()?.into_iter().next()?;
    crate::reader::rar_read_entry(path, &first, MAX_IMAGE).ok()
}

fn zip_open(path: &Path) -> Option<zip::ZipArchive<BufReader<File>>> {
    zip::ZipArchive::new(BufReader::new(File::open(path).ok()?)).ok()
}

fn zip_first_image(path: &Path) -> Option<Vec<u8>> {
    let mut z = zip_open(path)?;
    let mut names: Vec<String> = z.file_names().filter(|n| is_image_name(n)).map(str::to_string).collect();
    names.sort_by(|a, b| natural_cmp(a, b));
    let first = names.first()?;
    formats::read_zip_entry(&mut z, first, MAX_IMAGE).ok()
}

/// Nối `href` (tương đối với OPF) thành đường dẫn trong zip, xử lý `../` và `%20`.
fn resolve(base_dir: &str, href: &str) -> String {
    let href = percent_decode(href.split('#').next().unwrap_or(href));
    let mut parts: Vec<&str> =
        if href.starts_with('/') { Vec::new() } else { base_dir.split('/').filter(|s| !s.is_empty()).collect() };
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn epub_cover(path: &Path) -> Option<Vec<u8>> {
    let mut z = zip_open(path)?;
    let container = formats::decode_utf8(&formats::read_zip_entry(&mut z, "META-INF/container.xml", 1 << 20).ok()?);
    let cdoc = roxmltree::Document::parse_with_options(&container, formats::xml_options()).ok()?;
    let opf_path = cdoc.descendants().find(|n| n.tag_name().name() == "rootfile")?.attribute("full-path")?.to_string();
    let opf = formats::decode_utf8(&formats::read_zip_entry(&mut z, &opf_path, 4 << 20).ok()?);
    let href = epub_cover_href(&opf)?;
    let dir = opf_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let full = resolve(dir, &href);
    formats::read_zip_entry(&mut z, &full, MAX_IMAGE).ok()
}

/// Tìm href ảnh bìa trong OPF: EPUB3 `properties="cover-image"` → EPUB2 `<meta name="cover">` →
/// item ảnh có "cover" trong id/href → ảnh đầu tiên trong manifest.
pub fn epub_cover_href(opf: &str) -> Option<String> {
    let doc = roxmltree::Document::parse_with_options(opf, formats::xml_options()).ok()?;
    let items: Vec<roxmltree::Node> = doc.descendants().filter(|n| n.tag_name().name() == "item").collect();
    let is_img = |n: &roxmltree::Node| n.attribute("media-type").is_some_and(|m| m.starts_with("image/"));
    if let Some(n) =
        items.iter().find(|n| n.attribute("properties").is_some_and(|p| p.split(' ').any(|x| x == "cover-image")))
    {
        return n.attribute("href").map(str::to_string);
    }
    let meta_id = doc
        .descendants()
        .find(|n| n.tag_name().name() == "meta" && n.attribute("name") == Some("cover"))
        .and_then(|n| n.attribute("content"));
    if let Some(id) = meta_id {
        if let Some(n) = items.iter().find(|n| n.attribute("id") == Some(id) && is_img(n)) {
            return n.attribute("href").map(str::to_string);
        }
    }
    let named = items.iter().find(|n| {
        is_img(n)
            && (n.attribute("id").is_some_and(|s| s.to_ascii_lowercase().contains("cover"))
                || n.attribute("href").is_some_and(|s| s.to_ascii_lowercase().contains("cover")))
    });
    named.or_else(|| items.iter().find(|n| is_img(n))).and_then(|n| n.attribute("href")).map(str::to_string)
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn mobi_cover(path: &Path) -> Option<Vec<u8>> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut head = vec![0u8; 78];
    f.read_exact(&mut head).ok()?;
    if &head[60..68] != b"BOOKMOBI" {
        return None;
    }
    let n = u16::from_be_bytes([head[76], head[77]]) as usize;
    let mut list = vec![0u8; n * 8];
    f.read_exact(&mut list).ok()?;
    let off = |i: usize| -> Option<u64> {
        if i < n {
            be32(&list, i * 8).map(|v| v as u64)
        } else if i == n {
            Some(len)
        } else {
            None
        }
    };
    let r0 = off(0)?;
    let r1 = off(1)?;
    f.seek(SeekFrom::Start(r0)).ok()?;
    let mut rec0 = vec![0u8; (r1 - r0).min(1 << 20) as usize];
    f.read_exact(&mut rec0).ok()?;
    let first_image = be32(&rec0, 108)? as usize;
    let mobi_len = be32(&rec0, 20)? as usize;
    let mut cover_off = None;
    if be32(&rec0, 128).unwrap_or(0) & 0x40 != 0 {
        let start = 16 + mobi_len;
        if rec0.get(start..start + 4) == Some(b"EXTH") {
            let count = be32(&rec0, start + 8)? as usize;
            let mut p = start + 12;
            for _ in 0..count.min(1000) {
                let (t, l) = (be32(&rec0, p)?, be32(&rec0, p + 4)? as usize);
                if l < 8 {
                    break;
                }
                if t == 201 {
                    cover_off = be32(&rec0, p + 8);
                }
                p += l;
            }
        }
    }
    let read_rec = |f: &mut File, i: usize| -> Option<Vec<u8>> {
        let (a, b) = (off(i)?, off(i + 1)?);
        if b <= a || b - a > MAX_IMAGE {
            return None;
        }
        f.seek(SeekFrom::Start(a)).ok()?;
        let mut v = vec![0u8; (b - a) as usize];
        f.read_exact(&mut v).ok()?;
        Some(v)
    };
    if first_image == 0 || first_image == 0xFFFF_FFFF || first_image >= n {
        return None;
    }
    if let Some(c) = cover_off.filter(|c| *c != 0xFFFF_FFFF) {
        if let Some(v) = read_rec(&mut f, first_image + c as usize).filter(|v| looks_like_image(v)) {
            return Some(v);
        }
    }
    // Không khai báo bìa: lấy ảnh đầu tiên.
    (first_image..n.min(first_image + 5)).find_map(|i| read_rec(&mut f, i).filter(|v| looks_like_image(v)))
}

fn fb2_cover(path: &Path) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    File::open(path).ok()?.take(MAX_IMAGE).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let cp = text.find("<coverpage>")?;
    let seg = &text[cp..(cp + 1000).min(text.len())];
    let href_pos = seg.find("href=\"#")? + 7;
    let id = &seg[href_pos..href_pos + seg[href_pos..].find('"')?];
    let tag = format!("id=\"{id}\"");
    let bpos = text.find(&tag)?;
    let start = bpos + text[bpos..].find('>')? + 1;
    let end = start + text[start..].find("</binary>")?;
    base64_decode(&text[start..end])
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Thu nhỏ thành JPEG ≤ 360×540.
pub fn thumbnail(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let t = img.thumbnail(THUMB_W, THUMB_H).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82).encode_image(&t).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Bộ nhớ đệm thumbnail. Khoá theo (id, size, mtime) để file đổi thì bìa tự làm mới.
pub struct CoverCache {
    dir: PathBuf,
}

impl CoverCache {
    pub fn new(home: &Path) -> CoverCache {
        CoverCache { dir: home.join("covers") }
    }

    fn base(&self, id: i64, size: i64, mtime: i64) -> PathBuf {
        self.dir.join(format!("{:02x}", id & 0xff)).join(format!("{id}-{size:x}-{mtime:x}"))
    }

    /// Trả về JPEG thumbnail, sinh nếu chưa có. `Ok(None)` = sách không có bìa (đã ghi nhớ, không thử lại).
    pub fn get(&self, id: i64, path: &Path, format: Format, size: i64, mtime: i64) -> std::io::Result<Option<Vec<u8>>> {
        let base = self.base(id, size, mtime);
        let jpg = base.with_extension("jpg");
        let none = base.with_extension("none");
        if let Ok(b) = std::fs::read(&jpg) {
            return Ok(Some(b));
        }
        if none.exists() {
            return Ok(None);
        }
        if let Some(d) = jpg.parent() {
            std::fs::create_dir_all(d)?;
            // Dọn bìa cũ của cùng sách (file đã đổi).
            if let Ok(rd) = std::fs::read_dir(d) {
                let prefix = format!("{id}-");
                for e in rd.flatten() {
                    if e.file_name().to_string_lossy().starts_with(&prefix) {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
        }
        match extract_cover(path, format).and_then(|raw| thumbnail(&raw).ok()) {
            Some(t) => {
                let tmp = base.with_extension("tmp");
                std::fs::write(&tmp, &t)?;
                std::fs::rename(&tmp, &jpg)?;
                Ok(Some(t))
            }
            None => {
                std::fs::write(&none, b"")?;
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opf_cover_variants() {
        let e3 = r#"<package><manifest><item id="a" href="img/a.png" media-type="image/png"/><item id="c" href="img/c.jpg" media-type="image/jpeg" properties="cover-image"/></manifest></package>"#;
        assert_eq!(epub_cover_href(e3).as_deref(), Some("img/c.jpg"));
        let e2 = r#"<package><metadata><meta name="cover" content="x"/></metadata><manifest><item id="x" href="../Images/b%20c.jpg" media-type="image/jpeg"/></manifest></package>"#;
        assert_eq!(epub_cover_href(e2).as_deref(), Some("../Images/b%20c.jpg"));
        assert_eq!(resolve("OEBPS/Text", "../Images/b%20c.jpg"), "OEBPS/Images/b c.jpg");
    }
    #[test]
    fn b64() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVs\nbG8").unwrap(), b"hello");
    }
}
