//! Chuẩn hoá tên sách / tác giả (E2-06), tách số tập truyện (D16), sắp xếp tự nhiên.

use regex::Regex;
use std::cmp::Ordering;
use std::sync::OnceLock;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// Bỏ dấu tiếng Việt (và mọi dấu kết hợp), `đ` → `d`, chữ thường.
pub fn fold(s: &str) -> String {
    s.nfd()
        .filter(|c| !is_combining_mark(*c))
        .map(|c| match c {
            'đ' | 'Đ' => 'd',
            'ø' | 'Ø' => 'o',
            'ł' | 'Ł' => 'l',
            'ß' => 's',
            _ => c,
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// `fold` rồi thay mọi ký tự không phải chữ/số bằng khoảng trắng, gộp khoảng trắng.
pub fn simple_key(s: &str) -> String {
    let f = fold(s);
    let spaced: String = f.chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("regex hợp lệ"))
}

/// Từ khoá cho thấy một cụm trong ngoặc là "rác nguồn tải" chứ không phải tên sách.
const JUNK_MARKERS: &[&str] = &[
    "z-lib",
    "zlib",
    "z lib",
    "libgen",
    "ebook",
    "e-book",
    "www",
    ".com",
    ".org",
    ".net",
    ".vn",
    "retail",
    "converted",
    "scan",
    "pdf",
    "epub",
    "mobi",
    "azw",
    "cbz",
    "cbr",
    "djvu",
    "fb2",
    "sachvui",
    "vnthuquan",
    "thuviensach",
    "dtv",
    "waka",
    "anna's archive",
    "annas archive",
];

/// Token rác bị bỏ ở bất kỳ vị trí nào (sau khi đã `simple_key`).
const JUNK_TOKENS: &[&str] = &[
    "ebook",
    "epub",
    "mobi",
    "azw3",
    "azw",
    "pdf",
    "cbz",
    "cbr",
    "fb2",
    "djvu",
    "retail",
    "zlib",
    "libgen",
    "www",
    "com",
    "org",
    "net",
    "sachvui",
    "vnthuquan",
    "thuviensach",
];

fn paren_is_junk(inner_folded: &str, at_end: bool) -> bool {
    let t = inner_folded.trim();
    if t.is_empty() {
        return true;
    }
    if at_end && t.len() <= 2 && t.chars().all(|c| c.is_ascii_digit()) {
        return true; // hậu tố bản sao "(1)"
    }
    if t == "copy" || t == "ban sao" {
        return true;
    }
    JUNK_MARKERS.iter().any(|m| t.contains(m))
}

/// Chuẩn hoá tên sách để so khớp: bỏ dấu, chữ thường, bỏ `[...]`, bỏ `(1)`, `- Copy`,
/// cụm nguồn tải `(z-lib.org)`, đuôi định dạng; GIỮ số (để "Doraemon 1" ≠ "Doraemon 2").
pub fn normalize_title(raw: &str) -> String {
    static BRACKETS: OnceLock<Regex> = OnceLock::new();
    static PARENS: OnceLock<Regex> = OnceLock::new();
    static COPY_TAIL: OnceLock<Regex> = OnceLock::new();

    let mut s = fold(raw);
    // Bỏ đuôi file nếu còn sót.
    if let Some(dot) = s.rfind('.') {
        let ext = &s[dot + 1..];
        if crate::formats::Format::from_ext(ext).is_some() {
            s.truncate(dot);
        }
    }
    // [ebook], [Team XYZ], {abc}: luôn là chú thích.
    s = re(&BRACKETS, r"\[[^\]]*\]|\{[^}]*\}|【[^】]*】").replace_all(&s, " ").into_owned();

    // Bỏ hậu tố bản sao lặp lại: " - copy", " copy (2)", " (1)", " ban sao".
    let copy_tail = re(&COPY_TAIL, r"(?:\s*-?\s*(?:copy|ban sao)(?:\s*\(\d{1,2}\))?|\s*\(\d{1,2}\))\s*$");
    loop {
        let next = copy_tail.replace(s.trim_end(), "").into_owned();
        if next == s.trim_end() {
            break;
        }
        s = next;
    }

    // Ngoặc tròn: bỏ nếu là rác, ngược lại giữ phần bên trong.
    let parens = re(&PARENS, r"\(([^()]*)\)");
    let total = s.trim_end().len();
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for cap in parens.captures_iter(&s) {
        let m = cap.get(0).unwrap();
        out.push_str(&s[last..m.start()]);
        let inner = cap.get(1).unwrap().as_str();
        if !paren_is_junk(inner, m.end() >= total) {
            out.push(' ');
            out.push_str(inner);
            out.push(' ');
        } else {
            out.push(' ');
        }
        last = m.end();
    }
    out.push_str(&s[last..]);

    let key = simple_key(&out);
    let toks: Vec<&str> = key.split(' ').filter(|t| !t.is_empty() && !JUNK_TOKENS.contains(t)).collect();
    toks.join(" ")
}

/// Chuẩn hoá tên tác giả: bỏ dấu, bỏ ký tự lạ, sắp xếp token (để "Ánh, Nguyễn Nhật" ≡ "Nguyễn Nhật Ánh").
pub fn normalize_author(raw: &str) -> String {
    let k = simple_key(raw);
    let mut toks: Vec<&str> = k.split(' ').filter(|t| !t.is_empty()).collect();
    toks.sort_unstable();
    toks.dedup();
    toks.join(" ")
}

/// Hai tác giả (đã `normalize_author`) có mâu thuẫn không? Rỗng = không biết = không mâu thuẫn.
pub fn authors_conflict(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let sa: std::collections::HashSet<&str> = a.split(' ').filter(|t| t.len() > 1).collect();
    let sb: std::collections::HashSet<&str> = b.split(' ').filter(|t| t.len() > 1).collect();
    if sa.is_empty() || sb.is_empty() {
        return false;
    }
    if sa.is_subset(&sb) || sb.is_subset(&sa) {
        return false;
    }
    let inter = sa.intersection(&sb).count() as f64;
    let uni = sa.union(&sb).count() as f64;
    inter / uni < 0.5
}

/// Tên file có hậu tố bản sao (`(1)`, `- Copy`, `bản sao`) không? (BRD 3.4)
pub fn has_copy_suffix(stem: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    let f = fold(stem);
    re(&R, r"(?:\(\d{1,2}\)|\bcopy(?:\s*\(\d{1,2}\))?|\bban sao(?:\s*\(\d{1,2}\))?)\s*$").is_match(f.trim_end())
}

fn vol_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(?:\b(?:volume|vol|tap|quyen|so|book|chapter|chap|ch|v|t)\s*\.?\s*|#\s*)(\d{1,4})\b")
}

fn last_number(s: &str) -> Option<u32> {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(\d{1,4})").captures_iter(s).last().and_then(|c| c[1].parse().ok())
}

/// Số tập truyện (D16).
/// - Có `series_hint` (tên thư mục bộ) và tên file bắt đầu bằng tên bộ → số cuối cùng của phần còn lại
///   (`7 Seeds 4` với bộ `7 seeds` → 4, không phải 7).
/// - Ngược lại: mẫu `Vol 3`, `Vol.3`, `Tập 03`, `#3`, `v3`; cuối cùng là số đứng cuối tên.
pub fn parse_volume(stem: &str, series_hint: Option<&str>) -> Option<u32> {
    let f = fold(stem);
    if let Some(series) = series_hint {
        let sk = simple_key(series);
        let fk = simple_key(stem);
        if !sk.is_empty() && fk.starts_with(&sk) {
            let rest = &fk[sk.len()..];
            if rest.is_empty() || rest.starts_with(' ') {
                return last_number(rest);
            }
        }
    }
    if let Some(c) = vol_pattern().captures_iter(&f).last() {
        return c[1].parse().ok();
    }
    // Số đứng cuối, chỉ khi phía trước còn chữ (tránh coi "1984" là tập).
    let k = simple_key(stem);
    let mut toks: Vec<&str> = k.split(' ').collect();
    let last = toks.pop()?;
    if !toks.is_empty() && last.len() <= 4 && last.chars().all(|c| c.is_ascii_digit()) {
        let n: u32 = last.parse().ok()?;
        if !(1900..=2099).contains(&n) {
            return Some(n);
        }
    }
    None
}

/// Tên bộ suy từ tên file (dùng khi chưa có thư mục bộ): bỏ cụm số tập và số cuối.
/// `Doraemon vol 45` → `doraemon`; `7 Seeds 12` → `7 seeds`; `Conan - Tập 03` → `conan`.
pub fn series_from_name(stem: &str) -> String {
    let t = normalize_title(stem);
    let without = vol_pattern().replace_all(&t, " ");
    let k = simple_key(&without);
    let mut toks: Vec<&str> = k.split(' ').filter(|s| !s.is_empty()).collect();
    if toks.len() > 1 {
        if let Some(last) = toks.last() {
            if last.chars().all(|c| c.is_ascii_digit()) {
                toks.pop();
            }
        }
    }
    toks.join(" ")
}

/// Tách token cho tìm kiếm / phân loại (không dấu, ≥ 2 ký tự hoặc là số).
pub fn tokens(s: &str) -> Vec<String> {
    simple_key(s).split(' ').filter(|t| !t.is_empty()).map(|t| t.to_string()).collect()
}

/// Thứ tự các tập trong một bộ: tập có số đứng trước (theo số, trùng số thì theo tên), tập không số đứng sau (theo tên).
/// Phải là thứ tự toàn phần: bản cũ so số khi cả hai có số mà so tên khi thiếu số, tạo vòng
/// (Vol 1 "c" < Vol 2 "a" < "b" < Vol 1 "c") → `sort_by` panic từ Rust 1.81.
pub fn volume_cmp(a_vol: Option<i64>, a_name: &str, b_vol: Option<i64>, b_name: &str) -> Ordering {
    match (a_vol, b_vol) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| natural_cmp(a_name, b_name)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => natural_cmp(a_name, b_name),
    }
}

/// So sánh "tự nhiên": `Vol 2` < `Vol 10` (E3-07).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    natural_cmp_folded(&fold(a), &fold(b)).then_with(|| a.cmp(b))
}

/// Như [`natural_cmp`] nhưng nhận chuỗi **đã** `fold` (để sắp hàng chục nghìn mục mà không bỏ dấu lại
/// ở mỗi lần so sánh). Bằng nhau thì trả `Equal`; người gọi tự phân xử bằng chuỗi gốc.
pub fn natural_cmp_folded(fa: &str, fb: &str) -> Ordering {
    let mut ia = fa.chars().peekable();
    let mut ib = fb.chars().peekable();
    loop {
        match (ia.peek().copied(), ib.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut na = String::new();
                    while let Some(c) = ia.peek().copied().filter(|c| c.is_ascii_digit()) {
                        na.push(c);
                        ia.next();
                    }
                    let mut nb = String::new();
                    while let Some(c) = ib.peek().copied().filter(|c| c.is_ascii_digit()) {
                        nb.push(c);
                        ib.next();
                    }
                    let ta = na.trim_start_matches('0');
                    let tb = nb.trim_start_matches('0');
                    let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if o != Ordering::Equal {
                        return o;
                    }
                } else {
                    let o = ca.cmp(&cb);
                    if o != Ordering::Equal {
                        return o;
                    }
                    ia.next();
                    ib.next();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_cmp_is_total_order() {
        // Bộ trộn tập có số và không số – trước đây tạo vòng và làm sort_by panic.
        let mut v = vec![(Some(1), "c"), (None, "b"), (Some(2), "a"), (None, "a"), (Some(1), "a"), (Some(10), "z")];
        for _ in 0..3 {
            v.sort_by(|a, b| volume_cmp(a.0, a.1, b.0, b.1));
        }
        assert_eq!(v, vec![(Some(1), "a"), (Some(1), "c"), (Some(2), "a"), (Some(10), "z"), (None, "a"), (None, "b")]);
        let items: Vec<(Option<i64>, String)> =
            (0..40).map(|i| (if i % 3 == 0 { None } else { Some(i % 7) }, format!("Vol {}", (i * 13) % 17))).collect();
        for a in &items {
            for b in &items {
                for c in &items {
                    let ab = volume_cmp(a.0, &a.1, b.0, &b.1);
                    let bc = volume_cmp(b.0, &b.1, c.0, &c.1);
                    if ab != Ordering::Greater && bc != Ordering::Greater {
                        assert_ne!(volume_cmp(a.0, &a.1, c.0, &c.1), Ordering::Greater, "{a:?} {b:?} {c:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn fold_vietnamese() {
        assert_eq!(fold("Mắt Biếc"), "mat biec");
        assert_eq!(fold("ĐẮC NHÂN TÂM"), "dac nhan tam");
        assert_eq!(fold("Tiểu sử - Hồi ký"), "tieu su - hoi ky");
    }

    #[test]
    fn normalize_title_table() {
        let cases: &[(&str, &str)] = &[
            ("Mắt Biếc", "mat biec"),
            ("Mắt Biếc (1)", "mat biec"),
            ("Mắt Biếc - Copy", "mat biec"),
            ("Mắt Biếc - Copy (2)", "mat biec"),
            ("Mắt biếc (z-lib.org)", "mat biec"),
            ("[ebook] Mắt Biếc", "mat biec"),
            ("Mắt_Biếc", "mat biec"),
            ("Mắt.Biếc.epub", "mat biec"),
            ("MẮT BIẾC.pdf", "mat biec"),
            ("Mắt Biếc bản sao", "mat biec"),
            ("Đắc Nhân Tâm", "dac nhan tam"),
            ("Dac Nhan Tam [sachvui.com]", "dac nhan tam"),
            ("Đắc nhân tâm (Sachvui.Com)", "dac nhan tam"),
            ("Đắc Nhân Tâm (ebook)", "dac nhan tam"),
            ("Đắc Nhân Tâm (Retail)", "dac nhan tam"),
            ("Đắc-Nhân-Tâm", "dac nhan tam"),
            ("Sapiens (libgen.rs)", "sapiens"),
            ("Sapiens [Anna's Archive]", "sapiens"),
            ("Sapiens   ", "sapiens"),
            ("Sapiens: Lược Sử Loài Người", "sapiens luoc su loai nguoi"),
            ("Sapiens - Lược sử loài người", "sapiens luoc su loai nguoi"),
            ("Doraemon 1", "doraemon 1"),
            ("Doraemon 2", "doraemon 2"),
            ("Doraemon (Tập 2)", "doraemon tap 2"),
            ("Doraemon Tập 2 (1)", "doraemon tap 2"),
            ("Doraemon vol.45", "doraemon vol 45"),
            ("7 Seeds 12", "7 seeds 12"),
            ("7 Seeds 12 - Copy", "7 seeds 12"),
            ("1984", "1984"),
            ("1984 (1)", "1984"),
            ("Harry Potter and the Philosopher's Stone", "harry potter and the philosopher s stone"),
            ("Harry Potter (Book 1)", "harry potter book 1"),
            ("Clean Code (www.allitebooks.com)", "clean code"),
            ("Clean_Code_[ebook]", "clean code"),
            ("Clean Code {retail}", "clean code"),
            ("Clean Code (PDF)", "clean code"),
            ("Clean Code (epub)", "clean code"),
            ("Clean Code.mobi", "clean code"),
            ("Clean Code.azw3", "clean code"),
            ("Clean Code copy", "clean code"),
            ("Nhà Giả Kim", "nha gia kim"),
            ("Nhà giả kim (Paulo Coelho)", "nha gia kim paulo coelho"),
            ("NHÀ  GIẢ   KIM", "nha gia kim"),
            ("Nhà-giả-kim (2)", "nha gia kim"),
            ("Tôi thấy hoa vàng trên cỏ xanh", "toi thay hoa vang tren co xanh"),
            ("Toi Thay Hoa Vang Tren Co Xanh (vnthuquan)", "toi thay hoa vang tren co xanh"),
            ("Dế Mèn Phiêu Lưu Ký", "de men phieu luu ky"),
            ("De_Men_Phieu_Luu_Ky.fb2", "de men phieu luu ky"),
            ("Số Đỏ – Vũ Trọng Phụng", "so do vu trong phung"),
            ("Truyện Kiều", "truyen kieu"),
            ("Truyen Kieu (1) (2)", "truyen kieu"),
            ("One Piece Vol. 100 (Digital)", "one piece vol 100 digital"),
            ("", ""),
        ];
        for (input, want) in cases {
            assert_eq!(&normalize_title(input), want, "input: {input:?}");
        }
        assert!(cases.len() >= 50);
    }

    #[test]
    fn author_rules() {
        assert_eq!(normalize_author("Nguyễn Nhật Ánh"), normalize_author("Ánh, Nguyễn Nhật"));
        assert!(!authors_conflict(&normalize_author("Nguyễn Nhật Ánh"), &normalize_author("Nhật Ánh")));
        assert!(!authors_conflict("", &normalize_author("Ai đó")));
        assert!(authors_conflict(&normalize_author("Nguyễn Nhật Ánh"), &normalize_author("Nguyễn Du")));
        assert!(authors_conflict(&normalize_author("Robert C. Martin"), &normalize_author("Martin Fowler")));
        assert!(!authors_conflict(&normalize_author("Robert C. Martin"), &normalize_author("Robert Martin")));
    }

    #[test]
    fn copy_suffix() {
        assert!(has_copy_suffix("Mắt Biếc (1)"));
        assert!(has_copy_suffix("Mắt Biếc - Copy"));
        assert!(has_copy_suffix("Mắt Biếc - Copy (3)"));
        assert!(has_copy_suffix("Mắt Biếc - Bản sao"));
        assert!(!has_copy_suffix("Mắt Biếc"));
        assert!(!has_copy_suffix("Copywriting 101"));
    }

    #[test]
    fn volumes() {
        assert_eq!(parse_volume("7 Seeds 4", Some("7 seeds")), Some(4));
        assert_eq!(parse_volume("7 Seeds 12", None), Some(12));
        assert_eq!(parse_volume("Doraemon vol 45", None), Some(45));
        assert_eq!(parse_volume("Doraemon Vol.3", None), Some(3));
        assert_eq!(parse_volume("Doraemon - Tập 03", None), Some(3));
        assert_eq!(parse_volume("Doraemon #3", None), Some(3));
        assert_eq!(parse_volume("Doraemon v3", None), Some(3));
        assert_eq!(parse_volume("Conan Tap 100", Some("Thám tử lừng danh Conan")), Some(100));
        assert_eq!(parse_volume("1984", None), None);
        assert_eq!(parse_volume("Mắt Biếc", None), None);
        assert_eq!(parse_volume("Báo cáo 2020", None), None);
    }

    #[test]
    fn series_names() {
        assert_eq!(series_from_name("Doraemon vol 45"), "doraemon");
        assert_eq!(series_from_name("7 Seeds 12"), "7 seeds");
        assert_eq!(series_from_name("Doraemon - Tập 03"), "doraemon");
        assert_eq!(series_from_name("Doraemon Vol.3"), "doraemon");
        assert_eq!(series_from_name("Doraemon #3"), "doraemon");
        assert_eq!(series_from_name("One Piece 1000 (1)"), "one piece");
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["Vol 10", "Vol 2", "Vol 1", "vol 02b", "Tập 3"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["Tập 3", "Vol 1", "Vol 2", "vol 02b", "Vol 10"]);
    }
}
