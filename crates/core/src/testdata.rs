//! Bộ dữ liệu mẫu tự sinh (E0-04): epub, mobi, azw3, cbz, cbr, pdf, fb2, djvu, txt, bản trùng,
//! tên Unicode, đường dẫn dài. Dùng chung cho test lõi và lệnh `librarian gen-sample`.

use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

pub struct EpubSpec<'a> {
    pub title: &'a str,
    pub authors: &'a [&'a str],
    pub subjects: &'a [&'a str],
    pub isbn: Option<&'a str>,
    pub language: &'a str,
    pub body: &'a str,
}

/// Ảnh bìa PNG thật (giải mã được) với màu suy ra từ `seed`.
pub fn sample_png(seed: &str, w: u32, h: u32) -> Vec<u8> {
    let hsh = seed.bytes().fold(2166136261u32, |a, b| (a ^ b as u32).wrapping_mul(16777619));
    let (r, g, b) = ((hsh & 0xff) as u8, ((hsh >> 8) & 0xff) as u8, ((hsh >> 16) & 0xff) as u8);
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        if y > h / 3 && y < h / 3 + h / 8 && x > w / 8 && x < w - w / 8 {
            image::Rgb([250, 250, 245])
        } else {
            image::Rgb([r, g, b])
        }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).expect("encode png");
    out.into_inner()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn parent(p: &Path) -> std::io::Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    Ok(())
}

pub fn write_epub(path: &Path, s: &EpubSpec) -> std::io::Result<()> {
    parent(path)?;
    let mut z = zip::ZipWriter::new(std::fs::File::create(path)?);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    z.start_file("mimetype", stored)?;
    z.write_all(b"application/epub+zip")?;
    z.start_file("META-INF/container.xml", deflated)?;
    z.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#)?;
    let mut meta = format!("<dc:title>{}</dc:title><dc:language>{}</dc:language>", esc(s.title), esc(s.language));
    for a in s.authors {
        meta.push_str(&format!(r#"<dc:creator opf:role="aut">{}</dc:creator>"#, esc(a)));
    }
    for x in s.subjects {
        meta.push_str(&format!("<dc:subject>{}</dc:subject>", esc(x)));
    }
    if let Some(i) = s.isbn {
        meta.push_str(&format!(r#"<dc:identifier opf:scheme="ISBN">{}</dc:identifier>"#, esc(i)));
    }
    let opf = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">{meta}</metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="cov" href="Images/cover.png" media-type="image/png" properties="cover-image"/></manifest><spine><itemref idref="c1"/></spine></package>"#
    );
    z.start_file("OEBPS/content.opf", deflated)?;
    z.write_all(opf.as_bytes())?;
    z.start_file("OEBPS/Images/cover.png", stored)?;
    z.write_all(&sample_png(s.title, 60, 90))?;
    z.start_file("OEBPS/c1.xhtml", deflated)?;
    z.write_all(
        format!(
            r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><body><p>{}</p></body></html>"#,
            esc(s.body)
        )
        .as_bytes(),
    )?;
    z.finish()?;
    Ok(())
}

/// MOBI tối giản: PalmDB + PalmDOC + MOBI header + EXTH (100 tác giả, 104 ISBN, 105 chủ đề, 503 tiêu đề).
pub fn write_mobi(
    path: &Path,
    title: &str,
    author: &str,
    isbn: Option<&str>,
    subject: Option<&str>,
    body: &str,
) -> std::io::Result<()> {
    parent(path)?;
    let mut exth_recs: Vec<(u32, Vec<u8>)> =
        vec![(100, author.as_bytes().to_vec()), (503, title.as_bytes().to_vec()), (201, 0u32.to_be_bytes().to_vec())];
    if let Some(i) = isbn {
        exth_recs.push((104, i.as_bytes().to_vec()));
    }
    if let Some(s) = subject {
        exth_recs.push((105, s.as_bytes().to_vec()));
    }
    let mut exth = Vec::new();
    for (t, d) in &exth_recs {
        exth.extend_from_slice(&t.to_be_bytes());
        exth.extend_from_slice(&((d.len() + 8) as u32).to_be_bytes());
        exth.extend_from_slice(d);
    }
    let exth_len = 12 + exth.len();
    let pad = (4 - exth_len % 4) % 4;
    let mut exth_block = b"EXTH".to_vec();
    exth_block.extend_from_slice(&((exth_len + pad) as u32).to_be_bytes());
    exth_block.extend_from_slice(&(exth_recs.len() as u32).to_be_bytes());
    exth_block.extend_from_slice(&exth);
    exth_block.extend(std::iter::repeat_n(0, pad));

    const MOBI_LEN: usize = 232;
    let mut rec0 = vec![0u8; 16 + MOBI_LEN];
    rec0[0..2].copy_from_slice(&1u16.to_be_bytes()); // không nén
    rec0[4..8].copy_from_slice(&(body.len() as u32).to_be_bytes());
    rec0[8..10].copy_from_slice(&1u16.to_be_bytes());
    rec0[10..12].copy_from_slice(&4096u16.to_be_bytes());
    rec0[16..20].copy_from_slice(b"MOBI");
    rec0[20..24].copy_from_slice(&(MOBI_LEN as u32).to_be_bytes());
    rec0[24..28].copy_from_slice(&2u32.to_be_bytes());
    rec0[28..32].copy_from_slice(&65001u32.to_be_bytes());
    let full_off = rec0.len() + exth_block.len();
    rec0[84..88].copy_from_slice(&(full_off as u32).to_be_bytes());
    rec0[88..92].copy_from_slice(&(title.len() as u32).to_be_bytes());
    rec0[108..112].copy_from_slice(&2u32.to_be_bytes()); // record ảnh đầu tiên
    rec0[128..132].copy_from_slice(&0x40u32.to_be_bytes());
    rec0.extend_from_slice(&exth_block);
    rec0.extend_from_slice(title.as_bytes());
    rec0.extend_from_slice(&[0, 0]);

    let mut head = vec![0u8; 78];
    let short: String = title.chars().filter(|c| c.is_ascii_alphanumeric()).take(31).collect();
    head[..short.len()].copy_from_slice(short.as_bytes());
    head[60..68].copy_from_slice(b"BOOKMOBI");
    head[76..78].copy_from_slice(&3u16.to_be_bytes());
    let cover = sample_png(title, 60, 90);
    let r0 = 78 + 3 * 8 + 2;
    let r1 = r0 + rec0.len();
    let r2 = r1 + body.len();
    let mut out = head;
    for (i, off) in [r0, r1, r2].iter().enumerate() {
        out.extend_from_slice(&(*off as u32).to_be_bytes());
        out.extend_from_slice(&(2 * i as u32).to_be_bytes());
    }
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&rec0);
    out.extend_from_slice(body.as_bytes());
    out.extend_from_slice(&cover);
    std::fs::write(path, out)
}

fn pdf_text(s: &str) -> String {
    let mut h = String::from("<FEFF");
    for u in s.encode_utf16() {
        h.push_str(&format!("{u:04X}"));
    }
    h.push('>');
    h
}

/// PDF hợp lệ tối giản với bảng xref cổ điển và Info (Title/Author dạng UTF-16BE).
pub fn write_pdf(path: &Path, title: Option<&str>, author: Option<&str>, filler: &str) -> std::io::Result<()> {
    parent(path)?;
    let mut info = String::from("<<");
    if let Some(t) = title {
        info.push_str(&format!(" /Title {}", pdf_text(t)));
    }
    if let Some(a) = author {
        info.push_str(&format!(" /Author {}", pdf_text(a)));
    }
    info.push_str(" /Producer (My Librarian test) >>");
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R >>".to_string(),
        info,
        format!("<< /Length {} >>\nstream\n{}\nendstream", filler.len() + 1, filler),
    ];
    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, o).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R /Info 4 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1)
            .as_bytes(),
    );
    std::fs::write(path, out)
}

pub fn write_cbz(
    path: &Path,
    series: Option<&str>,
    number: Option<u32>,
    pages: u32,
    marker: &str,
) -> std::io::Result<()> {
    parent(path)?;
    let mut z = zip::ZipWriter::new(std::fs::File::create(path)?);
    let opt = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    if let (Some(s), Some(n)) = (series, number) {
        z.start_file("ComicInfo.xml", opt)?;
        z.write_all(
            format!(r#"<?xml version="1.0"?><ComicInfo><Series>{}</Series><Number>{n}</Number><PageCount>{pages}</PageCount></ComicInfo>"#, esc(s))
                .as_bytes(),
        )?;
    }
    for p in 1..=pages {
        z.start_file(format!("{p:03}.png"), opt)?;
        z.write_all(&sample_png(&format!("{marker}-{p}"), 40, 60))?;
    }
    z.finish()?;
    Ok(())
}

pub fn write_fb2(path: &Path, title: &str, first: &str, last: &str, genre: &str) -> std::io::Result<()> {
    parent(path)?;
    std::fs::write(
        path,
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0"><description><title-info><genre>{}</genre><author><first-name>{}</first-name><last-name>{}</last-name></author><book-title>{}</book-title><lang>vi</lang></title-info></description><body><section><p>{}</p></section></body></FictionBook>"#,
            esc(genre),
            esc(first),
            esc(last),
            esc(title),
            esc(title)
        ),
    )
}

/// Thông tin về bộ mẫu để test kiểm chứng.
pub struct Sample {
    pub books: PathBuf,
    pub inbox: PathBuf,
    /// Số file sách hợp lệ (không tính file trong thư mục ẩn / thùng rác / file không phải sách).
    pub book_files: usize,
    pub long_path: PathBuf,
}

/// Sinh thư viện mẫu tại `dir/books`:
/// thể loại có sẵn, `truyen tranh/<bộ>/`, hộp thư đến có bản trùng, nghi trùng, sách mới, truyện mới.
pub fn generate_sample(dir: &Path) -> std::io::Result<Sample> {
    let b = dir.join("books");
    let vh = b.join("Văn học");
    let ts = b.join("Tiểu sử - Hồi ký");
    let cn = b.join("Công nghệ");
    let tt = b.join("truyen tranh");
    let inbox = b.join(crate::config::DEFAULT_INBOX);
    let mut n = 0;
    macro_rules! count {
        ($e:expr) => {{
            $e?;
            n += 1;
        }};
    }
    let nna = &["Nguyễn Nhật Ánh"];
    count!(write_epub(
        &vh.join("Mắt Biếc.epub"),
        &EpubSpec {
            title: "Mắt Biếc",
            authors: nna,
            subjects: &["Văn học Việt Nam", "Tiểu thuyết"],
            isbn: Some("978-604-1-00001-8"),
            language: "vi",
            body: "Ngạn và Hà Lan"
        }
    ));
    count!(write_epub(
        &vh.join("Tôi thấy hoa vàng trên cỏ xanh.epub"),
        &EpubSpec {
            title: "Tôi thấy hoa vàng trên cỏ xanh",
            authors: nna,
            subjects: &["Văn học Việt Nam", "Tiểu thuyết"],
            isbn: None,
            language: "vi",
            body: "Thiều và Tường"
        }
    ));
    count!(write_fb2(&vh.join("Số Đỏ.fb2"), "Số Đỏ", "Vũ Trọng", "Phụng", "Tiểu thuyết trào phúng"));
    count!(write_pdf(
        &vh.join("Truyện Kiều.pdf"),
        Some("Truyện Kiều"),
        Some("Nguyễn Du"),
        "Trăm năm trong cõi người ta"
    ));
    count!(write_epub(
        &ts.join("Steve Jobs.epub"),
        &EpubSpec {
            title: "Steve Jobs",
            authors: &["Walter Isaacson"],
            subjects: &["Tiểu sử", "Biography"],
            isbn: None,
            language: "vi",
            body: "Apple"
        }
    ));
    count!(write_pdf(
        &ts.join("Đời viết văn của tôi.pdf"),
        Some("Đời viết văn của tôi - Hồi ký"),
        Some("Nguyễn Hiến Lê"),
        "hồi ký"
    ));
    count!(write_epub(
        &cn.join("Clean Code.epub"),
        &EpubSpec {
            title: "Clean Code",
            authors: &["Robert C. Martin"],
            subjects: &["Lập trình", "Programming", "Software"],
            isbn: None,
            language: "en",
            body: "functions"
        }
    ));
    count!(write_mobi(
        &cn.join("The Pragmatic Programmer.azw3"),
        "The Pragmatic Programmer",
        "Andrew Hunt",
        None,
        Some("Programming"),
        "tips"
    ));
    count!(write_mobi(&tt.join("7 seeds").join("7 Seeds 1.mobi"), "7 Seeds 1", "Tamura Yumi", None, None, "vol1"));
    count!(write_mobi(&tt.join("7 seeds").join("7 Seeds 2.mobi"), "7 Seeds 2", "Tamura Yumi", None, None, "vol2"));
    for v in [1u32, 2, 10] {
        count!(write_cbz(
            &tt.join("Doraemon").join(format!("Doraemon vol {v}.cbz")),
            Some("Doraemon"),
            Some(v),
            3,
            &format!("dora{v}")
        ));
    }
    // Hộp thư đến.
    std::fs::create_dir_all(&inbox)?;
    std::fs::copy(vh.join("Mắt Biếc.epub"), inbox.join("Mắt Biếc (1).epub"))?; // trùng chắc chắn
    n += 1;
    count!(write_pdf(&inbox.join("Mat Biec.pdf"), None, None, "ban scan khac")); // nghi trùng (tên)
    count!(write_epub(
        &inbox.join("Ngày xưa có một chuyện tình.epub"),
        &EpubSpec {
            title: "Ngày xưa có một chuyện tình",
            authors: nna,
            subjects: &[],
            isbn: None,
            language: "vi",
            body: "Vinh"
        }
    ));
    count!(write_epub(
        &inbox.join("Refactoring.epub"),
        &EpubSpec {
            title: "Refactoring",
            authors: &["Martin Fowler"],
            subjects: &["Programming", "Software"],
            isbn: None,
            language: "en",
            body: "code smells"
        }
    ));
    count!(write_mobi(&inbox.join("7 Seeds 3.mobi"), "7 Seeds 3", "Tamura Yumi", None, None, "vol3"));
    count!(write_cbz(&inbox.join("Doraemon Tập 45.cbz"), None, None, 2, "dora45"));
    count!(write_cbz(&inbox.join("One Piece 1.cbz"), None, None, 2, "op1"));
    std::fs::write(inbox.join("Không rõ.txt"), "???")?;
    n += 1;
    // Không phải sách, thư mục ẩn, thùng rác → bị bỏ qua.
    std::fs::write(inbox.join("ghi chú.docx"), "x")?;
    write_epub(
        &b.join(".hidden").join("an.epub"),
        &EpubSpec { title: "Ẩn", authors: &[], subjects: &[], isbn: None, language: "vi", body: "" },
    )?;
    write_epub(
        &b.join("$RECYCLE.BIN").join("rac.epub"),
        &EpubSpec { title: "Rác", authors: &[], subjects: &[], isbn: None, language: "vi", body: "" },
    )?;
    // CBR thực chất là zip, DJVU (chỉ quản lý).
    write_cbz(&tt.join("Doraemon").join("Doraemon vol 3.cbr"), None, None, 1, "dora3")?;
    n += 1;
    std::fs::write(vh.join("Bản đồ cổ.djvu"), b"AT&TFORM\0\0\0\0DJVU")?;
    n += 1;
    // Đường dẫn dài > 260 ký tự.
    let seg = "Thư mục có tên rất dài để kiểm tra đường dẫn vượt quá giới hạn MAX_PATH của Windows";
    let long_dir = vh.join(seg).join(seg).join(seg);
    let long_path = long_dir.join("Sách ở đường dẫn rất dài.epub");
    count!(write_epub(
        &long_path,
        &EpubSpec {
            title: "Sách ở đường dẫn rất dài",
            authors: &["Ẩn danh"],
            subjects: &["Văn học"],
            isbn: None,
            language: "vi",
            body: "dài"
        }
    ));
    Ok(Sample { books: b, inbox, book_files: n, long_path })
}
