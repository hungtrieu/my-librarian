//! CBZ (zip + ComicInfo.xml) và CBR (RAR; nhiều file .cbr thực chất là zip) – E2-03, E2-05.

use super::{decode_utf8, read_zip_entry, xml_options, Metadata};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif", "jxl"];

pub fn is_image_name(name: &str) -> bool {
    name.rsplit('.').next().map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)
        && !name.starts_with("__MACOSX")
}

pub fn read_cbz(path: &Path) -> Result<Metadata, String> {
    let f = File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(BufReader::new(f)).map_err(|e| format!("zip hỏng: {e}"))?;
    let mut info_name = None;
    let mut pages = 0u32;
    for name in zip.file_names() {
        if name.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case("ComicInfo.xml")) {
            info_name = Some(name.to_string());
        } else if is_image_name(name) {
            pages += 1;
        }
    }
    let mut md = Metadata::default();
    if let Some(n) = info_name {
        if let Ok(bytes) = read_zip_entry(&mut zip, &n, 1024 * 1024) {
            md = parse_comic_info(&decode_utf8(&bytes)).unwrap_or_default();
        }
    }
    if md.page_count.is_none() && pages > 0 {
        md.page_count = Some(pages);
    }
    Ok(md)
}

pub fn read_cbr(path: &Path) -> Result<Metadata, String> {
    let mut magic = [0u8; 4];
    File::open(path).and_then(|mut f| f.read_exact(&mut magic)).map_err(|e| e.to_string())?;
    if &magic == b"PK\x03\x04" {
        return read_cbz(path);
    }
    if &magic[..] == b"Rar!" {
        // RAR: chỉ đếm trang (liệt kê, không giải nén); ComicInfo trong RAR hiếm nên bỏ qua.
        let pages = crate::reader::rar_image_names(path).map_err(|e| e.to_string())?.len() as u32;
        return Ok(Metadata { page_count: (pages > 0).then_some(pages), ..Default::default() });
    }
    Err("CBR không phải RAR/ZIP".into())
}

pub fn parse_comic_info(xml: &str) -> Result<Metadata, String> {
    let doc = roxmltree::Document::parse_with_options(xml, xml_options()).map_err(|e| e.to_string())?;
    let mut md = Metadata::default();
    let mut number = None;
    for n in doc.root_element().children().filter(|n| n.is_element()) {
        let t = n.text().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        match n.tag_name().name() {
            "Title" => md.title = t,
            "Series" => md.series = t,
            "Number" => number = t,
            "Volume" if number.is_none() => number = t,
            "Writer" => {
                if let Some(w) = t {
                    md.authors.extend(w.split(',').map(|s| s.trim().to_string()));
                }
            }
            "Genre" => {
                if let Some(g) = t {
                    md.subjects.extend(g.split(',').map(|s| s.trim().to_string()));
                }
            }
            "LanguageISO" => md.language = t,
            "PageCount" => md.page_count = t.and_then(|s| s.parse().ok()),
            "GTIN" => md.isbn = t,
            _ => {}
        }
    }
    md.volume = number.and_then(|s| s.parse::<f32>().ok()).map(|f| f as u32);
    Ok(md)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comic_info() {
        let xml = r#"<?xml version="1.0"?><ComicInfo><Title>Bảo bối</Title><Series>Doraemon</Series><Number>45</Number><Writer>Fujiko F. Fujio</Writer><PageCount>190</PageCount></ComicInfo>"#;
        let md = parse_comic_info(xml).unwrap();
        assert_eq!(md.series.as_deref(), Some("Doraemon"));
        assert_eq!(md.volume, Some(45));
        assert_eq!(md.page_count, Some(190));
        assert_eq!(md.authors, vec!["Fujiko F. Fujio"]);
    }
}
