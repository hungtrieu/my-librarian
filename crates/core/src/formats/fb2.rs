//! FB2: đọc phần `<description><title-info>` ở đầu file (E2-05).

use super::Metadata;
use regex::Regex;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

pub fn read(path: &Path) -> Result<Metadata, String> {
    let mut buf = Vec::new();
    File::open(path).map_err(|e| e.to_string())?.take(512 * 1024).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let head = String::from_utf8_lossy(&buf[..buf.len().min(200)]).to_ascii_lowercase();
    let text = if head.contains("windows-1251") { decode_cp1251(&buf) } else { super::decode_utf8(&buf) };
    if !text.contains("<FictionBook") {
        return Err("không phải FictionBook".into());
    }
    Ok(parse(&text))
}

fn rx(cell: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(p).unwrap())
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}

fn tag<'a>(block: &'a str, name: &str) -> Vec<&'a str> {
    let re = Regex::new(&format!(r"(?s)<{name}(?:\s[^>]*)?>(.*?)</{name}>")).unwrap();
    re.captures_iter(block).map(|c| c.get(1).unwrap().as_str()).collect()
}

pub fn parse(text: &str) -> Metadata {
    static TI: OnceLock<Regex> = OnceLock::new();
    static SEQ: OnceLock<Regex> = OnceLock::new();
    let mut md = Metadata::default();
    let Some(ti) = rx(&TI, r"(?s)<title-info>(.*?)</title-info>").captures(text) else { return md };
    let ti = ti.get(1).unwrap().as_str();
    md.title = tag(ti, "book-title").first().map(|s| unescape(s));
    for a in tag(ti, "author") {
        let parts: Vec<String> = ["first-name", "middle-name", "last-name"]
            .iter()
            .filter_map(|t| tag(a, t).first().map(|s| unescape(s)))
            .filter(|s| !s.is_empty())
            .collect();
        let name = if parts.is_empty() {
            tag(a, "nickname").first().map(|s| unescape(s)).unwrap_or_default()
        } else {
            parts.join(" ")
        };
        if !name.is_empty() {
            md.authors.push(name);
        }
    }
    md.subjects = tag(ti, "genre").into_iter().map(unescape).collect();
    md.language = tag(ti, "lang").first().map(|s| unescape(s));
    if let Some(c) = rx(&SEQ, r#"<sequence\s+name="([^"]*)"(?:\s+number="(\d+)")?"#).captures(ti) {
        md.series = Some(unescape(&c[1]));
        md.volume = c.get(2).and_then(|m| m.as_str().parse().ok());
    }
    md
}

fn decode_cp1251(b: &[u8]) -> String {
    b.iter()
        .map(|&c| match c {
            0x00..=0x7F => c as char,
            0xC0..=0xFF => char::from_u32(0x410 + (c as u32 - 0xC0)).unwrap(),
            0xA8 => 'Ё',
            0xB8 => 'ё',
            _ => '?',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fb2() {
        let t = r#"<?xml version="1.0" encoding="utf-8"?><FictionBook><description><title-info>
<genre>prose</genre><author><first-name>Tô</first-name><last-name>Hoài</last-name></author>
<book-title>Dế Mèn Phiêu Lưu Ký</book-title><lang>vi</lang><sequence name="Văn học thiếu nhi" number="2"/>
</title-info></description></FictionBook>"#;
        let md = parse(t);
        assert_eq!(md.title.as_deref(), Some("Dế Mèn Phiêu Lưu Ký"));
        assert_eq!(md.authors, vec!["Tô Hoài"]);
        assert_eq!(md.subjects, vec!["prose"]);
        assert_eq!(md.volume, Some(2));
    }
}
