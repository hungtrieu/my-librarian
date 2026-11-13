//! PDF: chỉ đọc trailer + bảng xref + object Info (E2-04). Không đọc cả file.
//! PDF dùng xref stream nén / Info nằm trong object stream → không lấy được, fallback tên file.

use super::Metadata;
use regex::bytes::Regex;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::OnceLock;

const TAIL: u64 = 64 * 1024;
const SCAN_WINDOW: u64 = 4 * 1024 * 1024;

fn read_at(f: &mut File, off: u64, len: u64) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(off))?;
    let mut buf = Vec::new();
    f.by_ref().take(len).read_to_end(&mut buf)?;
    Ok(buf)
}

fn find_last(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

fn info_ref(buf: &[u8]) -> Option<(u32, u32)> {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = R.get_or_init(|| Regex::new(r"/Info\s+(\d+)\s+(\d+)\s+R").unwrap());
    let c = r.captures_iter(buf).last()?;
    Some((std::str::from_utf8(&c[1]).ok()?.parse().ok()?, std::str::from_utf8(&c[2]).ok()?.parse().ok()?))
}

pub fn read(path: &Path) -> Result<Metadata, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let head = read_at(&mut f, 0, 1024).map_err(|e| e.to_string())?;
    if !head.windows(5).any(|w| w == b"%PDF-") {
        return Err("không có header %PDF".into());
    }
    let tail_off = len.saturating_sub(TAIL);
    let tail = read_at(&mut f, tail_off, TAIL).map_err(|e| e.to_string())?;
    let mut md = Metadata::default();
    if find_last(&tail, b"/Encrypt").is_some() {
        md.drm = true; // chuỗi Info bị mã hoá → không đọc được tiêu đề
        return Ok(md);
    }
    let startxref = find_last(&tail, b"startxref").and_then(|p| {
        let s = String::from_utf8_lossy(&tail[p + 9..(p + 40).min(tail.len())]).into_owned();
        s.split_whitespace().next()?.parse::<u64>().ok()
    });
    let mut info = info_ref(&tail);
    if info.is_none() {
        if let Some(sx) = startxref {
            if let Ok(b) = read_at(&mut f, sx, 8192) {
                info = info_ref(&b);
            }
        }
    }
    let Some((num, gen)) = info else { return Ok(md) };
    let offset = startxref
        .and_then(|sx| locate_in_xref(&mut f, sx, num).ok().flatten())
        .or_else(|| scan_for_object(&mut f, len, num, gen));
    let Some(offset) = offset else { return Ok(md) };
    let obj = read_at(&mut f, offset, 16 * 1024).map_err(|e| e.to_string())?;
    let end = obj.windows(6).position(|w| w == b"endobj").unwrap_or(obj.len());
    let dict = &obj[..end];
    md.title = string_value(dict, b"/Title");
    if let Some(a) = string_value(dict, b"/Author") {
        md.authors = a.split([';', '&']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    }
    if let Some(s) = string_value(dict, b"/Subject") {
        md.subjects.push(s);
    }
    // Tiêu đề rác do phần mềm sinh ra → bỏ để fallback tên file.
    if let Some(t) = &md.title {
        let l = t.to_ascii_lowercase();
        if l.starts_with("microsoft word") || l.ends_with(".doc") || l.ends_with(".docx") || l == "untitled" {
            md.title = None;
        }
    }
    Ok(md)
}

/// Tra bảng xref cổ điển (có chuỗi `/Prev`) để lấy offset object `num`.
fn locate_in_xref(f: &mut File, startxref: u64, num: u32) -> std::io::Result<Option<u64>> {
    let mut pos = startxref;
    for _ in 0..16 {
        f.seek(SeekFrom::Start(pos))?;
        let mut r = BufReader::new(&mut *f);
        let mut line = Vec::new();
        r.read_until(b'\n', &mut line)?;
        if !line.trim_ascii().starts_with(b"xref") {
            return Ok(None); // xref stream → không hỗ trợ
        }
        loop {
            line.clear();
            if r.read_until(b'\n', &mut line)? == 0 {
                return Ok(None);
            }
            let t = line.trim_ascii();
            if t.is_empty() {
                continue;
            }
            if t.starts_with(b"trailer") {
                break;
            }
            let s = String::from_utf8_lossy(t).into_owned();
            let mut it = s.split_whitespace();
            let (Some(a), Some(b)) = (it.next(), it.next()) else { return Ok(None) };
            let (Ok(start), Ok(count)) = (a.parse::<u64>(), b.parse::<u64>()) else { return Ok(None) };
            if (num as u64) >= start && (num as u64) < start + count {
                r.seek_relative(((num as u64 - start) * 20) as i64)?;
                let mut e = [0u8; 20];
                r.read_exact(&mut e)?;
                let es = String::from_utf8_lossy(&e).into_owned();
                let mut p = es.split_whitespace();
                let off = p.next().and_then(|x| x.parse::<u64>().ok());
                let _gen = p.next();
                if p.next() == Some("n") {
                    return Ok(off);
                }
                return Ok(None);
            }
            r.seek_relative((count * 20) as i64)?;
        }
        // Đọc trailer để tìm /Prev.
        let mut trailer = Vec::new();
        r.take(4096).read_to_end(&mut trailer)?;
        static PREV: OnceLock<Regex> = OnceLock::new();
        #[allow(clippy::regex_creation_in_loops)] // OnceLock: chỉ biên dịch một lần
        let re = PREV.get_or_init(|| Regex::new(r"/Prev\s+(\d+)").unwrap());
        match re.captures(&trailer).and_then(|c| std::str::from_utf8(&c[1]).ok()?.parse().ok()) {
            Some(p) => pos = p,
            None => return Ok(None),
        }
    }
    Ok(None)
}

/// Dự phòng: tìm `N G obj` trong 4 MB đầu và 4 MB cuối.
fn scan_for_object(f: &mut File, len: u64, num: u32, gen: u32) -> Option<u64> {
    let pat = Regex::new(&format!(r"(?:^|[\r\n\s]){num}\s+{gen}\s+obj")).ok()?;
    let mut windows = vec![(0u64, SCAN_WINDOW.min(len))];
    if len > SCAN_WINDOW {
        let s = len.saturating_sub(SCAN_WINDOW).max(SCAN_WINDOW);
        windows.push((s, len - s));
    }
    for (off, l) in windows {
        let buf = read_at(f, off, l).ok()?;
        if let Some(m) = pat.find_iter(&buf).last() {
            let start = m.start() + buf[m.start()..].iter().take_while(|b| b.is_ascii_whitespace()).count();
            return Some(off + start as u64);
        }
    }
    None
}

/// Lấy giá trị chuỗi của khoá trong dictionary (`(literal)` hoặc `<hex>`).
pub fn string_value(dict: &[u8], key: &[u8]) -> Option<String> {
    let mut from = 0;
    while let Some(rel) = dict[from..].windows(key.len()).position(|w| w == key) {
        let mut i = from + rel + key.len();
        from = i;
        if dict.get(i).is_some_and(|c| c.is_ascii_alphanumeric()) {
            continue; // /TitleX
        }
        while dict.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
            i += 1;
        }
        let bytes = match dict.get(i)? {
            b'(' => parse_literal(&dict[i + 1..])?,
            b'<' if dict.get(i + 1) != Some(&b'<') => parse_hex(&dict[i + 1..])?,
            _ => return None,
        };
        let s = decode_pdf_string(&bytes);
        let s = s.trim().to_string();
        return if s.is_empty() { None } else { Some(s) };
    }
    None
}

fn parse_literal(b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'\\' => {
                i += 1;
                let e = *b.get(i)?;
                match e {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'\r' => {
                        if b.get(i + 1) == Some(&b'\n') {
                            i += 1;
                        }
                    }
                    b'\n' => {}
                    b'0'..=b'7' => {
                        let mut v = 0u32;
                        let mut k = 0;
                        while k < 3 && b.get(i).is_some_and(|d| (b'0'..=b'7').contains(d)) {
                            v = v * 8 + (b[i] - b'0') as u32;
                            i += 1;
                            k += 1;
                        }
                        out.push(v as u8);
                        continue;
                    }
                    other => out.push(other),
                }
            }
            b'(' => {
                depth += 1;
                out.push(c);
            }
            b')' => {
                if depth == 0 {
                    return Some(out);
                }
                depth -= 1;
                out.push(c);
            }
            _ => out.push(c),
        }
        i += 1;
    }
    None
}

fn parse_hex(b: &[u8]) -> Option<Vec<u8>> {
    let end = b.iter().position(|&c| c == b'>')?;
    let digits: Vec<u8> = b[..end].iter().copied().filter(|c| c.is_ascii_hexdigit()).collect();
    let mut out = Vec::new();
    for ch in digits.chunks(2) {
        let s = std::str::from_utf8(ch).ok()?;
        let v = if s.len() == 1 { u8::from_str_radix(&format!("{s}0"), 16) } else { u8::from_str_radix(s, 16) };
        out.push(v.ok()?);
    }
    Some(out)
}

fn decode_pdf_string(b: &[u8]) -> String {
    if b.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> =
            b[2..].chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&b[3..]).into_owned();
    }
    // PDFDocEncoding ~ Windows-1252 cho phần lớn ký tự thường gặp.
    super::mobi::decode_cp1252(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strings() {
        assert_eq!(string_value(b"<< /Title (Hello \\(World\\)) >>", b"/Title").as_deref(), Some("Hello (World)"));
        assert_eq!(string_value(b"<</Title<FEFF004D1EAF0074>>>", b"/Title").as_deref(), Some("Mắt"));
        assert_eq!(string_value(b"<</TitleX (a) /Title (b)>>", b"/Title").as_deref(), Some("b"));
        assert_eq!(string_value(b"<</Title 12 0 R>>", b"/Title"), None);
    }
}
