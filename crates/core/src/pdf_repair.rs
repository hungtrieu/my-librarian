//! Sửa "ảo" bảng xref của PDF hỏng khi đọc trong app – không ghi gì vào file gốc.
//!
//! Một số công cụ ghi vị trí byte bằng số nguyên 32-bit có dấu: với PDF > 2 GB, `startxref` thành số âm
//! và các object nằm sau mốc 2 GB bị ghi vị trí 0. pdf.js khi đó phải quét lại cả file để dựng xref
//! (không làm được với file vài GB). Ở đây ta tìm lại vị trí thật của các object hỏng (chỉ quét phần
//! file sau object hợp lệ cuối cùng), rồi tạo một "đuôi" gồm bảng xref đúng + trailer. App phục vụ file
//! gốc nối với đuôi này, nên pdf.js đọc như một PDF bình thường (cập nhật kiểu incremental).

use crate::error::{Error, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Bảng xref cổ điển lớn nhất chấp nhận đọc (21 nghìn object ≈ 420 KB; 4 triệu object ≈ 80 MB).
const MAX_XREF_BYTES: u64 = 96 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    offset: u64,
    gen: u32,
    used: bool,
}

/// Trả `Some(đuôi)` nếu PDF cần sửa xref (và sửa được), `None` nếu file bình thường hoặc không sửa được.
pub fn repair_tail(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    if len < 64 {
        return Ok(None);
    }
    // startxref nằm trong ~1 KB cuối.
    let tail_len = len.min(2048);
    let tail = read_at(&mut f, len - tail_len, tail_len as usize)?;
    let Some(sx) = find_last(&tail, b"startxref") else { return Ok(None) };
    let num: String = String::from_utf8_lossy(&tail[sx + 9..])
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    let Ok(raw) = num.parse::<i64>() else { return Ok(None) };
    let startxref_ok = raw >= 0 && (raw as u64) < len;
    // Số âm do tràn 32-bit: cộng lại 2^32 (có thể nhiều lần với file > 4 GB – thử lần lượt).
    let mut candidates = Vec::new();
    if startxref_ok {
        candidates.push(raw as u64);
    }
    let mut c = raw;
    for _ in 0..4 {
        c += 1 << 32;
        if c >= 0 && (c as u64) < len {
            candidates.push(c as u64);
        }
    }
    let Some(xref_at) = candidates.into_iter().find(|&o| starts_with_at(&mut f, o, b"xref")) else {
        // Không thấy bảng xref cổ điển (có thể là xref stream) – để pdf.js tự xử lý.
        return Ok(None);
    };
    let xref_len = (len - xref_at).min(MAX_XREF_BYTES);
    let xref = read_at(&mut f, xref_at, xref_len as usize)?;
    let Some((mut entries, trailer)) = parse_xref(&xref) else { return Ok(None) };

    let bad = |e: &Entry| e.used && (e.offset == 0 || e.offset >= xref_at);
    let missing: Vec<usize> = entries.iter().enumerate().filter(|(_, e)| bad(e)).map(|(i, _)| i).collect();
    if startxref_ok && missing.is_empty() {
        return Ok(None);
    }
    if !missing.is_empty() {
        // Các object hỏng nằm sau object hợp lệ xa nhất – chỉ quét đoạn đó.
        let from = entries.iter().filter(|e| e.used && !bad(e)).map(|e| e.offset).max().unwrap_or(0);
        let found = scan_objects(&mut f, from, xref_at, &missing, &entries)?;
        for (i, off) in found {
            entries[i].offset = off;
        }
        let still = entries.iter().filter(|e| bad(e)).count();
        if still > 0 {
            tracing::warn!("sửa xref {}: còn {still} object không tìm thấy", path.display());
        }
    }
    let mut out = Vec::with_capacity(entries.len() * 20 + trailer.len() + 64);
    out.extend_from_slice(b"\n");
    let new_xref = len + out.len() as u64;
    out.extend_from_slice(format!("xref\n0 {}\n", entries.len()).as_bytes());
    for (i, e) in entries.iter().enumerate() {
        let line = if e.used && !bad(e) {
            format!("{:010} {:05} n\r\n", e.offset, e.gen)
        } else if i == 0 {
            "0000000000 65535 f\r\n".to_string()
        } else {
            // Không dùng hoặc không tìm thấy: đánh dấu free để pdf.js bỏ qua thay vì đọc sai chỗ.
            format!("0000000000 {:05} f\r\n", e.gen.max(1))
        };
        out.extend_from_slice(line.as_bytes());
    }
    out.extend_from_slice(b"trailer\n");
    out.extend_from_slice(&trailer);
    out.extend_from_slice(format!("\nstartxref\n{new_xref}\n%%EOF\n").as_bytes());
    Ok(Some(out))
}

fn read_at(f: &mut File, at: u64, n: usize) -> Result<Vec<u8>> {
    f.seek(SeekFrom::Start(at))?;
    let mut buf = Vec::with_capacity(n);
    f.take(n as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

fn starts_with_at(f: &mut File, at: u64, what: &[u8]) -> bool {
    read_at(f, at, what.len() + 8)
        .map(|b| {
            let s = b.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(0);
            b[s..].starts_with(what)
        })
        .unwrap_or(false)
}

fn find_last(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// Đọc bảng xref cổ điển (nhiều tiểu mục) và từ điển trailer `<< … >>`.
fn parse_xref(buf: &[u8]) -> Option<(Vec<Entry>, Vec<u8>)> {
    let text = buf;
    let mut pos = text.windows(4).position(|w| w == b"xref")? + 4;
    let mut entries: Vec<Entry> = Vec::new();
    let next_token = |pos: &mut usize| -> Option<(usize, usize)> {
        while *pos < text.len() && text[*pos].is_ascii_whitespace() {
            *pos += 1;
        }
        let s = *pos;
        while *pos < text.len() && !text[*pos].is_ascii_whitespace() {
            *pos += 1;
        }
        (s < *pos).then_some((s, *pos))
    };
    let num = |r: (usize, usize)| std::str::from_utf8(&text[r.0..r.1]).ok()?.parse::<i64>().ok();
    loop {
        let t = next_token(&mut pos)?;
        if &text[t.0..t.1] == b"trailer" {
            break;
        }
        let start = num(t)?;
        let count = num(next_token(&mut pos)?)?;
        if start < 0 || !(0..=10_000_000).contains(&count) {
            return None;
        }
        for k in 0..count as usize {
            let off = num(next_token(&mut pos)?)?;
            let gen = num(next_token(&mut pos)?)?;
            let kind = next_token(&mut pos)?;
            let idx = start as usize + k;
            if entries.len() <= idx {
                entries.resize(idx + 1, Entry { offset: 0, gen: 0, used: false });
            }
            entries[idx] = Entry {
                offset: off.max(0) as u64,
                gen: gen.clamp(0, 65535) as u32,
                used: &text[kind.0..kind.1] == b"n",
            };
        }
    }
    // Từ điển trailer: từ "<<" đầu tiên tới ">>" khớp.
    let rest = &text[pos..];
    let s = rest.windows(2).position(|w| w == b"<<")?;
    let mut depth = 0i32;
    let mut i = s;
    while i + 1 < rest.len() {
        match &rest[i..i + 2] {
            b"<<" => {
                depth += 1;
                i += 2;
            }
            b">>" => {
                depth -= 1;
                i += 2;
                if depth == 0 {
                    let dict = rest[s..i].to_vec();
                    return (!entries.is_empty()).then_some((entries, strip_prev(dict)));
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// Bỏ `/Prev n` (trỏ tới bảng xref cũ – có thể cũng hỏng) khỏi trailer.
fn strip_prev(dict: Vec<u8>) -> Vec<u8> {
    let re = regex::bytes::Regex::new(r"/Prev\s+-?\d+").expect("regex");
    re.replace_all(&dict, &b""[..]).into_owned()
}

/// Quét [from, to) tìm tiêu đề "N G obj" của các object còn thiếu; trả (chỉ số, vị trí).
fn scan_objects(f: &mut File, from: u64, to: u64, missing: &[usize], entries: &[Entry]) -> Result<Vec<(usize, u64)>> {
    let re = regex::bytes::Regex::new(r"(?-u)(?:^|[\r\n\x20%>])(\d{1,10})[\x20\r\n]+(\d{1,5})[\x20\r\n]+obj\b")
        .map_err(|e| Error::msg(e.to_string()))?;
    let want: std::collections::HashSet<usize> = missing.iter().copied().collect();
    let mut found: std::collections::HashMap<usize, u64> = std::collections::HashMap::new();
    const CHUNK: usize = 16 * 1024 * 1024;
    const OVERLAP: usize = 64;
    let mut at = from;
    f.seek(SeekFrom::Start(at))?;
    let mut buf = vec![0u8; CHUNK + OVERLAP];
    let mut carry = 0usize;
    while at < to && found.len() < want.len() {
        let n = ((to - at) as usize).min(CHUNK);
        f.read_exact(&mut buf[carry..carry + n])?;
        let base = at - carry as u64;
        let data = &buf[..carry + n];
        for c in re.captures_iter(data) {
            let (Some(m), Some(g)) = (c.get(1), c.get(2)) else { continue };
            let id: usize = std::str::from_utf8(m.as_bytes()).ok().and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
            let gen: u32 = std::str::from_utf8(g.as_bytes()).ok().and_then(|s| s.parse().ok()).unwrap_or(u32::MAX);
            if want.contains(&id) && entries[id].gen == gen {
                // Lần xuất hiện cuối cùng thắng (giống cập nhật incremental).
                found.insert(id, base + m.start() as u64);
            }
        }
        at += n as u64;
        carry = OVERLAP.min(data.len());
        let keep_from = data.len() - carry;
        buf.copy_within(keep_from..keep_from + carry, 0);
    }
    Ok(found.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PDF nhỏ có bảng xref như công cụ lỗi ghi: startxref âm (tràn 32-bit) và object ghi vị trí 0.
    fn broken_pdf(path: &Path) -> (Vec<u8>, usize) {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>".to_string(),
            "<< /Length 8 >>\nstream\n0 0 m S\n\nendstream".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offs = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offs.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, o).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for (i, o) in offs.iter().enumerate() {
            // Object 3 và 4 "nằm sau mốc 2 GB" → ghi 0.
            let o = if i >= 2 { 0 } else { *o };
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        let neg = xref as i64 - (1i64 << 32);
        out.extend_from_slice(
            format!("trailer\n<< /Size {} /Root 1 0 R /Prev 99 >>\nstartxref\n{neg}\n%%EOF\n", objs.len() + 1)
                .as_bytes(),
        );
        std::fs::write(path, &out).unwrap();
        (out, offs[3])
    }

    #[test]
    fn repairs_negative_startxref_and_zero_offsets() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.pdf");
        let (orig, off4) = broken_pdf(&p);
        let tail = repair_tail(&p).unwrap().expect("cần sửa");
        let s = String::from_utf8(tail.clone()).unwrap();
        assert!(s.contains(&format!("{off4:010} 00000 n")), "{s}");
        assert!(!s.contains("/Prev"));
        assert!(s.contains("/Root 1 0 R"));
        // startxref mới trỏ đúng vào "xref" của đuôi khi nối sau file gốc.
        let mut all = orig.clone();
        all.extend_from_slice(&tail);
        let sx: usize = s.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        assert!(all[sx..].starts_with(b"xref"));
        // Mọi object "n" trỏ đúng tiêu đề "N 0 obj".
        for (i, line) in s.lines().skip(3).take(5).enumerate() {
            if line.ends_with(" n") {
                let o: usize = line[..10].parse().unwrap();
                assert!(all[o..].starts_with(format!("{i} 0 obj").as_bytes()), "object {i}");
            }
        }
    }

    #[test]
    fn healthy_pdf_needs_no_repair() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("ok.pdf");
        crate::testdata::write_pdf(&p, Some("T"), None, "x").unwrap();
        assert!(repair_tail(&p).unwrap().is_none());
    }
}

/// Thử trên file thật: `LIBRARIAN_BROKEN_PDF=... cargo test -p librarian-core --release real_broken_pdf -- --ignored --nocapture`
#[cfg(test)]
#[test]
#[ignore]
fn real_broken_pdf() {
    let p = std::env::var("LIBRARIAN_BROKEN_PDF").expect("đặt LIBRARIAN_BROKEN_PDF");
    let t = std::time::Instant::now();
    let tail = repair_tail(Path::new(&p)).unwrap();
    let s = tail.as_deref().map(String::from_utf8_lossy).unwrap_or_default();
    let free = s.lines().filter(|l| l.ends_with(" f")).count();
    eprintln!(
        "{:?}: đuôi {} B, {} dòng free, mất {:?}",
        tail.is_some(),
        tail.as_ref().map_or(0, |t| t.len()),
        free,
        t.elapsed()
    );
    eprintln!("{}", s.rsplit("trailer").next().unwrap_or(""));
    if let (Ok(out), Some(t)) = (std::env::var("LIBRARIAN_BROKEN_PDF_OUT"), &tail) {
        std::fs::write(out, t).unwrap();
    }
}
