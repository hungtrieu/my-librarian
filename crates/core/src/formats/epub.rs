//! EPUB: đọc `META-INF/container.xml` → OPF → `<metadata>` (E2-01).

use super::{decode_utf8, read_zip_entry, xml_options, Metadata};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

const XML_LIMIT: u64 = 4 * 1024 * 1024;

pub fn read(path: &Path) -> Result<Metadata, String> {
    let f = File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(BufReader::new(f)).map_err(|e| format!("zip hỏng: {e}"))?;
    let container = decode_utf8(&read_zip_entry(&mut zip, "META-INF/container.xml", XML_LIMIT)?);
    let doc = roxmltree::Document::parse_with_options(&container, xml_options())
        .map_err(|e| format!("container.xml: {e}"))?;
    let opf_path = doc
        .descendants()
        .find(|n| n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .ok_or("thiếu rootfile")?
        .to_string();
    let opf = decode_utf8(&read_zip_entry(&mut zip, &opf_path, XML_LIMIT)?);
    let mut md = parse_opf(&opf)?;
    // Font obfuscation cũng dùng encryption.xml; chỉ coi là DRM khi có thuật toán khác font.
    if let Ok(b) = read_zip_entry(&mut zip, "META-INF/encryption.xml", XML_LIMIT) {
        let s = decode_utf8(&b);
        md.drm = s.contains("EncryptedData")
            && !s.contains("http://www.idpf.org/2008/embedding")
            && !s.contains("http://ns.adobe.com/pdf/enc#RC");
    }
    Ok(md)
}

pub fn parse_opf(opf: &str) -> Result<Metadata, String> {
    let doc = roxmltree::Document::parse_with_options(opf, xml_options()).map_err(|e| format!("OPF: {e}"))?;
    let meta = doc.descendants().find(|n| n.tag_name().name() == "metadata").ok_or("OPF thiếu <metadata>")?;
    let mut md = Metadata::default();
    let mut isbn_candidates = Vec::new();
    for n in meta.descendants().filter(|n| n.is_element()) {
        let text = || n.text().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        match n.tag_name().name() {
            "title" if md.title.is_none() => md.title = text(),
            "creator" => {
                let role = n.attributes().find(|a| a.name() == "role").map(|a| a.value().to_string());
                if role.as_deref().is_none_or(|r| r == "aut") {
                    if let Some(t) = text() {
                        md.authors.push(t);
                    }
                }
            }
            "language" if md.language.is_none() => md.language = text(),
            "subject" => {
                if let Some(t) = text() {
                    md.subjects.push(t);
                }
            }
            "identifier" => {
                if let Some(t) = text() {
                    let scheme = n.attributes().find(|a| a.name() == "scheme").map(|a| a.value().to_ascii_lowercase());
                    let is_isbn = scheme.as_deref() == Some("isbn") || t.to_ascii_lowercase().contains("isbn");
                    isbn_candidates.push((is_isbn, t));
                }
            }
            "meta" => {
                let name = n.attribute("name");
                let content = n.attribute("content").map(|s| s.to_string());
                let prop = n.attribute("property");
                match (name, prop) {
                    (Some("calibre:series"), _) => md.series = content,
                    (Some("calibre:series_index"), _) => {
                        md.volume = content.and_then(|c| c.parse::<f32>().ok()).map(|f| f as u32)
                    }
                    (_, Some("belongs-to-collection")) if md.series.is_none() => {
                        md.series = text();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    // Ưu tiên identifier được khai báo là ISBN, sau đó bất kỳ identifier nào trông như ISBN.
    isbn_candidates.sort_by_key(|(is, _)| !is);
    md.isbn = isbn_candidates.into_iter().find_map(|(_, t)| super::normalize_isbn(&t));
    Ok(md)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opf_basic() {
        let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
 <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
  <dc:title>Mắt Biếc</dc:title>
  <dc:creator opf:role="aut">Nguyễn Nhật Ánh</dc:creator>
  <dc:creator opf:role="edt">Biên tập viên</dc:creator>
  <dc:language>vi</dc:language>
  <dc:subject>Văn học</dc:subject>
  <dc:identifier opf:scheme="uuid">urn:uuid:123</dc:identifier>
  <dc:identifier opf:scheme="ISBN">978-604-1-00000-1</dc:identifier>
  <meta name="calibre:series" content="Truyện dài"/>
  <meta name="calibre:series_index" content="3.0"/>
 </metadata>
</package>"#;
        let md = parse_opf(opf).unwrap();
        assert_eq!(md.title.as_deref(), Some("Mắt Biếc"));
        assert_eq!(md.authors, vec!["Nguyễn Nhật Ánh"]);
        assert_eq!(md.language.as_deref(), Some("vi"));
        assert_eq!(md.subjects, vec!["Văn học"]);
        assert_eq!(md.isbn.as_deref(), Some("9786041000001"));
        assert_eq!(md.series.as_deref(), Some("Truyện dài"));
        assert_eq!(md.volume, Some(3));
    }

    #[test]
    fn opf_broken() {
        assert!(parse_opf("<package><metadata>").is_err());
    }
}
