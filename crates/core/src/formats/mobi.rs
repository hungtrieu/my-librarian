//! MOBI / AZW / AZW3: PalmDB header → record 0 (PalmDOC + MOBI header) → full name + EXTH (E2-02).
//! Chỉ đọc phần đầu file và record 0.

use super::Metadata;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

fn be16(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_be_bytes([s[0], s[1]]))
}
fn be32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn read(path: &Path) -> Result<Metadata, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let mut head = vec![0u8; 78];
    f.read_exact(&mut head).map_err(|_| "file quá ngắn")?;
    let n = be16(&head, 76).unwrap_or(0) as usize;
    if n == 0 {
        return Err("PalmDB không có record".into());
    }
    let mut list = vec![0u8; n.min(2) * 8];
    f.read_exact(&mut list).map_err(|_| "thiếu bảng record")?;
    let r0 = be32(&list, 0).unwrap() as u64;
    let r1 = if n >= 2 { be32(&list, 8).unwrap() as u64 } else { len };
    if r0 >= len || r1 <= r0 {
        return Err("offset record 0 sai".into());
    }
    let size = (r1 - r0).min(1024 * 1024) as usize;
    f.seek(SeekFrom::Start(r0)).map_err(|e| e.to_string())?;
    let mut rec0 = vec![0u8; size];
    f.read_exact(&mut rec0).map_err(|_| "record 0 cụt")?;
    parse(&head, &rec0)
}

/// `head` = 78 byte PalmDB header, `rec0` = nội dung record 0.
pub fn parse(head: &[u8], rec0: &[u8]) -> Result<Metadata, String> {
    let kind = head.get(60..68).ok_or("header ngắn")?;
    if kind != b"BOOKMOBI" && kind != b"TEXtREAd" {
        return Err(format!("không phải MOBI ({})", String::from_utf8_lossy(kind)));
    }
    let palm_name = {
        let raw = &head[..32];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(32);
        String::from_utf8_lossy(&raw[..end]).replace('_', " ")
    };
    let mut md = Metadata { drm: be16(rec0, 12).is_some_and(|e| e != 0), ..Default::default() };
    if rec0.get(16..20) != Some(b"MOBI") {
        md.title = Some(palm_name);
        return Ok(md);
    }
    let mobi_len = be32(rec0, 20).unwrap_or(0) as usize;
    let encoding = be32(rec0, 28).unwrap_or(1252);
    let decode = |b: &[u8]| -> String {
        if encoding == 65001 {
            String::from_utf8_lossy(b).into_owned()
        } else {
            decode_cp1252(b)
        }
    };
    let full_off = be32(rec0, 84).unwrap_or(0) as usize;
    let full_len = be32(rec0, 88).unwrap_or(0) as usize;
    let full_name = rec0.get(full_off..full_off.saturating_add(full_len)).filter(|_| full_len > 0).map(decode);
    let exth_flags = be32(rec0, 128).unwrap_or(0);
    let mut updated_title = None;
    if exth_flags & 0x40 != 0 {
        let start = 16 + mobi_len;
        if rec0.get(start..start + 4) == Some(b"EXTH") {
            let count = be32(rec0, start + 8).unwrap_or(0) as usize;
            let mut p = start + 12;
            for _ in 0..count.min(1000) {
                let (Some(t), Some(l)) = (be32(rec0, p), be32(rec0, p + 4)) else { break };
                let l = l as usize;
                if l < 8 || p + l > rec0.len() {
                    break;
                }
                let data = &rec0[p + 8..p + l];
                let s = decode(data).trim_end_matches('\0').trim().to_string();
                match t {
                    100 => md.authors.extend(s.split(['&', ';']).map(|a| a.trim().to_string())),
                    104 => md.isbn = Some(s),
                    105 => md.subjects.push(s),
                    503 => updated_title = Some(s),
                    524 => md.language = Some(s),
                    _ => {}
                }
                p += l;
            }
        }
    }
    md.title = updated_title.filter(|s| !s.is_empty()).or(full_name).or(Some(palm_name));
    Ok(md)
}

/// Giải mã Windows-1252 (bảng 0x80–0x9F, còn lại = Latin-1).
pub fn decode_cp1252(b: &[u8]) -> String {
    const HI: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘',
        '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    b.iter()
        .map(|&c| match c {
            0x80..=0x9F => HI[(c - 0x80) as usize],
            _ => c as char,
        })
        .collect()
}
