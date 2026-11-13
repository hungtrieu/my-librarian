//! Nhận diện định dạng và trích metadata (E2-01..E2-05). Chỉ đọc phần đầu/cuối file, không đọc cả file.

pub mod comic;
pub mod epub;
pub mod fb2;
pub mod mobi;
pub mod pdf;

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Epub,
    Mobi,
    Azw,
    Azw3,
    Cbz,
    Cbr,
    Pdf,
    Fb2,
    Djvu,
    Txt,
}

impl Format {
    pub fn from_ext(ext: &str) -> Option<Format> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "epub" => Format::Epub,
            "mobi" | "prc" => Format::Mobi,
            "azw" => Format::Azw,
            "azw3" | "kf8" => Format::Azw3,
            "cbz" => Format::Cbz,
            "cbr" => Format::Cbr,
            "pdf" => Format::Pdf,
            "fb2" => Format::Fb2,
            "djvu" | "djv" => Format::Djvu,
            "txt" => Format::Txt,
            _ => return None,
        })
    }

    pub fn from_path(p: &Path) -> Option<Format> {
        p.extension().and_then(|e| e.to_str()).and_then(Format::from_ext)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Format::Epub => "epub",
            Format::Mobi => "mobi",
            Format::Azw => "azw",
            Format::Azw3 => "azw3",
            Format::Cbz => "cbz",
            Format::Cbr => "cbr",
            Format::Pdf => "pdf",
            Format::Fb2 => "fb2",
            Format::Djvu => "djvu",
            Format::Txt => "txt",
        }
    }

    pub fn parse(s: &str) -> Option<Format> {
        Format::from_ext(s)
    }

    pub fn is_comic_archive(self) -> bool {
        matches!(self, Format::Cbz | Format::Cbr)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub language: Option<String>,
    pub subjects: Vec<String>,
    pub isbn: Option<String>,
    pub series: Option<String>,
    pub volume: Option<u32>,
    pub page_count: Option<u32>,
    pub drm: bool,
}

impl Metadata {
    fn clean(mut self) -> Self {
        fn c(s: Option<String>) -> Option<String> {
            s.map(|v| v.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|v| !v.is_empty())
        }
        self.title = c(self.title);
        self.language = c(self.language);
        self.series = c(self.series);
        self.isbn = self.isbn.and_then(|i| normalize_isbn(&i));
        self.authors = self.authors.into_iter().filter_map(|a| c(Some(a))).collect();
        self.subjects = self.subjects.into_iter().filter_map(|a| c(Some(a))).collect();
        self
    }
}

/// Lấy ISBN-10/13 (chỉ chữ số, `X` cuối) từ chuỗi bất kỳ.
pub fn normalize_isbn(s: &str) -> Option<String> {
    let digits: String =
        s.chars().filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x').map(|c| c.to_ascii_uppercase()).collect();
    match digits.len() {
        13 if digits.starts_with("978") || digits.starts_with("979") => Some(digits),
        10 if digits[..9].chars().all(|c| c.is_ascii_digit()) => Some(digits),
        _ => None,
    }
}

/// Trích metadata. Lỗi = file hỏng/lạ; người gọi sẽ fallback dùng tên file (BRD 4).
pub fn extract(path: &Path, format: Format) -> Result<Metadata, String> {
    let md = match format {
        Format::Epub => epub::read(path),
        Format::Mobi | Format::Azw | Format::Azw3 => mobi::read(path),
        Format::Cbz => comic::read_cbz(path),
        Format::Cbr => comic::read_cbr(path),
        Format::Pdf => pdf::read(path),
        Format::Fb2 => fb2::read(path),
        Format::Djvu | Format::Txt => Ok(Metadata::default()),
    }?;
    Ok(md.clean())
}

/// Đọc tối đa `limit` byte của một entry trong zip (chống zip bomb).
pub(crate) fn read_zip_entry<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let f = zip.by_name(name).map_err(|e| format!("{name}: {e}"))?;
    let mut buf = Vec::new();
    f.take(limit).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

pub(crate) fn xml_options() -> roxmltree::ParsingOptions {
    roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }
}

/// Bỏ BOM, giải mã UTF-8 (lossy).
pub(crate) fn decode_utf8(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    String::from_utf8_lossy(b).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isbn() {
        assert_eq!(normalize_isbn("urn:isbn:978-604-1-00000-1").as_deref(), Some("9786041000001"));
        assert_eq!(normalize_isbn("ISBN 0-306-40615-2").as_deref(), Some("0306406152"));
        assert_eq!(normalize_isbn("uuid:1234"), None);
    }
}
