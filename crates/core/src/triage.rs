//! Tự phân loại hộp thư đến (`Chưa phân loại`) cho cả bộ sưu tập thả nguyên thư mục (kiểu Calibre Library):
//! - đọc `metadata.opf` (tag/tiêu đề/mô tả của Calibre) làm bằng chứng thể loại;
//! - làm phẳng: mỗi tác phẩm (mọi định dạng cùng tên) chuyển thẳng vào thư mục thể loại;
//! - tag của tác phẩm đã được người dùng ánh xạ vào một thư mục (`tag_map`, xem `tagmap.rs`) → vào đúng thư mục đó,
//!   trước mọi luật bên dưới;
//! - biết chắc thể loại → dùng thư mục có sẵn, chưa có thì **tạo mới**; chưa chắc → thư mục `Khác`;
//! - thư mục thể loại người dùng đã tự chia nhỏ (vd. `Ngôn tình - Đam mỹ/Đam mỹ`, `…/Ngôn tình`, nhiều cấp):
//!   tác phẩm khớp rõ tên một thư mục con (hoặc từ đồng nghĩa, xem [`SUB_ALIASES`]) đi tiếp vào đó;
//! - thư mục đích bị chặn (`blocked_categories`, xem `blocked.rs`) → tác phẩm vào thẳng Thùng rác, thư mục không
//!   bao giờ được tạo lại;
//! - xong thì đưa MỌI file không phải sách (ảnh bìa, .opf, .db, âm thanh, video…) và mọi thư mục con không còn
//!   sách vào Thùng rác.
//!
//! An toàn: có thể tắt bằng `auto_triage`; không chạy khi `read_only`; không ghi đè (trùng tên thêm `(n)`);
//! bản trùng y hệt (cùng tên, cùng dung lượng) vào Thùng rác; truyện tranh (.cbz/.cbr) để nguyên cho luồng
//! "bộ truyện"; thư mục còn sách chưa chuyển được hoặc có file nén (.zip/.rar/.7z) thì không bị xoá.
//! Mọi lần di chuyển được ghi vào nhật ký JSONL (`from`, `to`) để hoàn tác bằng tay nếu cần.

use crate::classify;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::moves;
use crate::normalize::{fold, simple_key};
use crate::trash::Trasher;
use crate::OnStep;
use crate::Step;
use rayon::prelude::*;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;

pub const OTHER_DIR: &str = "Khác";

/// Thư mục con của `Khác` cho các bộ "chưa chắc" nhiều dồn vào một chỗ (thường hàng chục nghìn file):
/// sách được xếp theo **chữ cái đầu** của tên file (đã bỏ dấu), tối đa 28 thư mục: `A`…`Z`; bắt đầu bằng số →
/// `0-9`; còn lại (chữ không phải Latin, ký hiệu) → `#`. Trong mỗi chữ, sách nằm trong các **ngăn** đánh số, mỗi ngăn
/// tối đa [`OTHER_SHELF_MAX`] file: `Khác/T/T-01/Thiên long bát bộ.epub`, `Khác/T/T-02/…`. Sách mới vào ngăn cuối,
/// đầy thì mở ngăn tiếp theo – không bao giờ phải xáo lại file đã xếp. Explorer/Finder mỗi lần mở chỉ thấy vài chục
/// mục hoặc tối đa 500 file.
pub fn other_bucket(file_name: &str) -> String {
    let stem = match file_name.rfind('.') {
        Some(i) if i > 0 => &file_name[..i],
        _ => file_name,
    };
    match fold(stem).chars().find(|c| c.is_alphanumeric()) {
        Some(c) if c.is_ascii_digit() => "0-9".to_string(),
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
        _ => "#".to_string(),
    }
}

/// Số file tối đa trong một ngăn của `Khác` (một tác phẩm nhiều định dạng không bị tách khỏi nhau).
pub const OTHER_SHELF_MAX: usize = 500;

/// Tên ngăn thứ `n` của chữ `letter`: `T-01`, `0-9-02`, `#-01`.
pub fn other_shelf_name(letter: &str, n: usize) -> String {
    format!("{letter}-{n:02}")
}

/// Số thứ tự nếu `name` là một ngăn của chữ `letter` (`T-03` → 3), không phân biệt hoa thường.
pub fn other_shelf_number(letter: &str, name: &str) -> Option<usize> {
    let head = name.get(..letter.len())?;
    if !head.eq_ignore_ascii_case(letter) {
        return None;
    }
    let num = name[letter.len()..].strip_prefix('-')?;
    if num.is_empty() || !num.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    num.parse().ok()
}

/// `name` có phải tên một thư mục chữ cái do [`other_bucket`] tạo ra không.
pub fn is_other_bucket(name: &str) -> bool {
    let b = name.as_bytes();
    name == "0-9" || name == "#" || (b.len() == 1 && b[0].is_ascii_alphabetic())
}

/// `name` có phải thư mục `Khác` không (so bỏ dấu, không phân biệt hoa thường).
pub fn is_other_dir(name: &str) -> bool {
    fold(name) == fold(OTHER_DIR)
}

/// Định dạng được coi là sách: không bao giờ bị xoá. Mọi file khác (ảnh, âm thanh, video, .opf, .db…) trong hộp thư đến bị dọn.
const BOOK_EXT: &[&str] = &[
    "epub", "mobi", "azw", "azw1", "azw3", "azw4", "prc", "pdf", "docx", "doc", "odt", "txt", "rtf", "fb2", "djvu",
    "lit", "lrf", "pdb", "chm", "htmlz", "snb", "xps", "oxps", "kfx", "cbz", "cbr", "cb7", "cbt",
];
const COMIC_EXT: &[&str] = &["cbz", "cbr", "cb7", "cbt"];
const ARCHIVE_EXT: &[&str] = &["zip", "rar", "7z"];
const JUNK_DIRS: &[&str] = &["caltrash", ".caltrash"];
const NOISE_TAGS: &[&str] =
    &["scan", "convert", "update", "full", "true pdf", "sách scan", "ai dịch", "khác", "general", "adult", "lang:vi"];
/// Từ ngắn dễ nhầm: khớp có dấu.
const SHORT_DIAC: &[&str] = &["thơ", "sủng", "ln", "thiền"];
const NOT_POETRY: &[&str] =
    &["tuổi thơ", "ấu thơ", "thơ ấu", "ngây thơ", "thơ ngây", "trẻ thơ", "con thơ", "thơ dại", "nhà thơ", "thơ thẩn"];
const MIN_SCORE: f64 = 2.0;
const MIN_MARGIN: f64 = 1.0;

struct RuleDef {
    name: &'static str,
    /// Khoá (đã bỏ dấu) để nhận ra thư mục có sẵn cùng thể loại, vd. "2. Kinh tế-Quản lý" ↔ "kinh te".
    aliases: &'static [&'static str],
    mult: f64,
    kws: &'static [&'static str],
}

const RULES: &[RuleDef] = &[
    RuleDef {
        name: "Ngôn tình - Đam mỹ",
        aliases: &["ngon tinh", "dam my"],
        mult: 1.0,
        kws: &[
            "ngôn tình",
            "đam mỹ",
            "romance",
            "xuyên không",
            "sủng",
            "trọng sinh",
            "tổng tài",
            "contemporary romance",
            "love story",
            "lãng mạn",
        ],
    },
    RuleDef {
        name: "Trinh thám - Hình sự",
        aliases: &["trinh tham", "hinh su"],
        mult: 1.0,
        kws: &[
            "trinh thám",
            "mystery",
            "detective",
            "hình sự",
            "suspense",
            "thriller",
            "crime",
            "sherlock",
            "bí ẩn",
            "vụ án",
        ],
    },
    RuleDef {
        name: "Kinh dị - Huyền bí",
        aliases: &["kinh di", "huyen bi"],
        mult: 1.0,
        kws: &["kinh dị", "horror", "ma quỷ", "huyền bí", "ghost stories"],
    },
    RuleDef {
        name: "Kiếm hiệp - Tiên hiệp",
        aliases: &["kiem hiep", "tien hiep", "vo hiep"],
        mult: 1.0,
        kws: &[
            "kiếm hiệp",
            "võ hiệp",
            "tiên hiệp",
            "wuxia",
            "xianxia",
            "tu tiên",
            "huyền huyễn",
            "kim dung",
            "cổ long",
        ],
    },
    RuleDef { name: "Light novel", aliases: &["light novel"], mult: 1.0, kws: &["light novel", "light-novel", "ln"] },
    RuleDef {
        name: "Khoa học viễn tưởng - Giả tưởng",
        aliases: &["vien tuong", "gia tuong"],
        mult: 1.0,
        kws: &["science fiction", "sci-fi", "fantasy", "giả tưởng", "viễn tưởng", "khoa học viễn tưởng"],
    },
    RuleDef {
        name: "Bút Ký - Truyện Ngắn - Tản Văn",
        aliases: &["truyen ngan", "tan van", "but ky"],
        mult: 1.0,
        kws: &[
            "truyện ngắn",
            "tản văn",
            "bút ký",
            "tạp văn",
            "tùy bút",
            "tuỳ bút",
            "short stories",
            "short story",
            "essays",
        ],
    },
    RuleDef {
        name: "Bút Ký - Hồi Ký",
        aliases: &["hoi ky"],
        mult: 1.0,
        kws: &["hồi ký", "hồi kí", "memoir", "memoirs", "autobiography", "tự truyện", "hồi ức"],
    },
    RuleDef {
        name: "Tiểu sử",
        aliases: &["tieu su"],
        mult: 1.0,
        kws: &["tiểu sử", "biography", "biographies", "chân dung"],
    },
    RuleDef {
        name: "Thơ",
        aliases: &["tho"],
        mult: 1.0,
        kws: &["thơ", "tập thơ", "thi tập", "thi ca", "trường ca", "poetry", "poems"],
    },
    RuleDef {
        name: "Tôn giáo - Tâm linh",
        aliases: &["ton giao", "tam linh", "phat giao"],
        mult: 1.0,
        kws: &[
            "buddhism",
            "phật giáo",
            "phật pháp",
            "thiền",
            "tôn giáo",
            "religion",
            "kinh thánh",
            "christian",
            "tâm linh",
            "phong thủy",
            "tử vi",
            "chiêm tinh",
        ],
    },
    RuleDef {
        name: "Lịch sử - Quân sự",
        aliases: &["lich su", "quan su"],
        mult: 1.0,
        kws: &["lịch sử", "history", "chiến tranh", "quân sự", "military", "dã sử", "sử việt"],
    },
    RuleDef {
        name: "Kinh tế - Quản lý",
        aliases: &["kinh te", "quan ly"],
        mult: 1.0,
        kws: &[
            "kinh tế",
            "economics",
            "quản trị",
            "quản lý",
            "business",
            "đầu tư",
            "tài chính",
            "khởi nghiệp",
            "chứng khoán",
            "kế toán",
            "lãnh đạo",
            "leadership",
            "management",
            "finance",
        ],
    },
    RuleDef {
        name: "Marketing - Sale",
        aliases: &["marketing"],
        mult: 1.0,
        kws: &["marketing", "bán hàng", "tiếp thị", "sales", "thương hiệu", "quảng cáo"],
    },
    RuleDef {
        name: "Tâm lý - Kỹ năng sống",
        aliases: &["tam ly", "ky nang"],
        mult: 1.0,
        kws: &[
            "tâm lý",
            "psychology",
            "self-help",
            "self help",
            "kỹ năng",
            "phát triển bản thân",
            "nghệ thuật sống",
            "đắc nhân tâm",
            "hạnh phúc",
        ],
    },
    RuleDef {
        name: "Khoa học - Công nghệ",
        aliases: &["khoa hoc", "cong nghe"],
        mult: 1.0,
        kws: &[
            "khoa học",
            "science",
            "công nghệ",
            "technology",
            "lập trình",
            "programming",
            "computer",
            "toán học",
            "vật lý",
            "sinh học",
            "mathematics",
        ],
    },
    RuleDef {
        name: "Y học - Sức khỏe - Nấu ăn",
        aliases: &["y hoc", "suc khoe", "nau an"],
        mult: 1.0,
        kws: &[
            "sức khỏe",
            "y học",
            "dinh dưỡng",
            "nấu ăn",
            "món ăn",
            "health",
            "cooking",
            "medicine",
            "làm đẹp",
            "yoga",
        ],
    },
    RuleDef {
        name: "Ngoại ngữ - Giáo dục",
        aliases: &["ngoai ngu", "giao duc"],
        mult: 1.0,
        kws: &[
            "học tiếng",
            "tiếng anh",
            "english",
            "ielts",
            "toeic",
            "từ điển",
            "dictionary",
            "giáo dục",
            "giáo trình",
            "ngữ pháp",
        ],
    },
    RuleDef {
        name: "Thiếu nhi",
        aliases: &["thieu nhi"],
        mult: 1.0,
        kws: &["thiếu nhi", "children", "trẻ em", "cổ tích", "juvenile"],
    },
    RuleDef {
        name: "Tiểu thuyết",
        aliases: &["tieu thuyet"],
        mult: 0.7,
        kws: &[
            "tiểu thuyết",
            "novel",
            "novels",
            "fiction",
            "classics",
            "literature",
            "văn học",
            "nobel",
            "truyện dài",
            "kinh điển",
        ],
    },
];

struct CompiledRule {
    def: &'static RuleDef,
    diac: Option<Regex>,
    folded: Option<Regex>,
}

fn word_re(words: &[String]) -> Option<Regex> {
    if words.is_empty() {
        return None;
    }
    let mut ws: Vec<&String> = words.iter().collect();
    ws.sort_by_key(|w| std::cmp::Reverse(w.len()));
    let alts: Vec<String> = ws.iter().map(|w| regex::escape(w)).collect();
    Regex::new(&format!(r"(?:^|[^\p{{L}}\p{{N}}_])(?:{})(?:$|[^\p{{L}}\p{{N}}_])", alts.join("|"))).ok()
}

fn nfc_lower(s: &str) -> String {
    s.nfc().collect::<String>().to_lowercase()
}

fn compiled() -> &'static Vec<CompiledRule> {
    static C: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    C.get_or_init(|| {
        RULES
            .iter()
            .map(|def| {
                let diac: Vec<String> =
                    def.kws.iter().filter(|k| SHORT_DIAC.contains(k)).map(|k| nfc_lower(k)).collect();
                let folded: Vec<String> = def.kws.iter().filter(|k| !SHORT_DIAC.contains(k)).map(|k| fold(k)).collect();
                CompiledRule { def, diac: word_re(&diac), folded: word_re(&folded) }
            })
            .collect()
    })
}

/// Điểm từng thể loại: tag ×3, tiêu đề/mô tả ×2, đường dẫn thư mục ×1.
fn score(subjects: &str, title: &str, path: &str) -> Vec<(&'static RuleDef, f64)> {
    // Chuẩn hoá mỗi trường một lần (không phải một lần cho mỗi luật): (trọng số, có dấu, bỏ dấu) – thường và
    // bản đã bỏ các cụm "tuổi thơ"… cho luật Thơ.
    let fields: Vec<(f64, [(String, String); 2])> = [(3.0, subjects), (2.0, title), (1.0, path)]
        .into_iter()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(w, text)| {
            let t = nfc_lower(text);
            let mut tp = t.clone();
            for p in NOT_POETRY {
                tp = tp.replace(p, " ");
            }
            let (f, fp) = (fold(&t), fold(&tp));
            (w, [(t, f), (tp, fp)])
        })
        .collect();
    let mut out = Vec::new();
    for r in compiled() {
        let mut s = 0.0;
        let variant = usize::from(r.def.name == "Thơ");
        for (w, forms) in &fields {
            let (t, f) = &forms[variant];
            if r.diac.as_ref().is_some_and(|re| re.is_match(t)) || r.folded.as_ref().is_some_and(|re| re.is_match(f)) {
                s += w * r.def.mult;
            }
        }
        if s > 0.0 {
            out.push((r.def, s));
        }
    }
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// Từ đồng nghĩa cho thư mục con người dùng tự chia trong một thể loại (khoá đã bỏ dấu): (khoá nhóm, từ mạnh,
/// từ yếu). Thư mục con có tên chứa khoá nhóm (`Đam Mỹ`, `Truyện đam mỹ`…) nhận thêm các từ của nhóm. Từ yếu (chung
/// chung) chỉ tính nửa điểm, để sách gắn tag "Romance; Đam mỹ" vào `Đam mỹ` chứ không vào `Ngôn tình`.
const SUB_ALIASES: &[(&str, &[&str], &[&str])] = &[
    ("dam my", &["dam my", "danmei", "dam mei", "boys love", "boy love", "yaoi"], &[]),
    ("ngon tinh", &["ngon tinh"], &["romance", "love story", "tong tai", "lang man"]),
    ("bach hop", &["bach hop", "yuri", "girls love", "girl love"], &[]),
    ("xuyen khong", &["xuyen khong", "xuyen sach", "xuyen nhanh", "isekai"], &[]),
    ("trong sinh", &["trong sinh", "reincarnation"], &[]),
    ("tien hiep", &["tien hiep", "tu tien", "tu chan", "xianxia"], &[]),
    ("kiem hiep", &["kiem hiep", "vo hiep", "wuxia"], &[]),
    ("huyen huyen", &["huyen huyen"], &[]),
];
/// Số cấp thư mục con tối đa đi xuống bên trong một thư mục thể loại.
const SUB_DEPTH: usize = 4;
/// Điểm tối thiểu để vào thư mục con: khớp tag / tiêu đề, hoặc khớp đủ tên ở đường dẫn trong hộp thư đến.
const SUB_MIN_SCORE: f64 = 1.0;

/// Từ khoá (bỏ dấu, kèm độ mạnh) nhận ra tác phẩm thuộc thư mục con `name`. Tên một từ ngắn (`Thơ`, `Mới`) không tự
/// làm từ khoá – bỏ dấu dễ nhầm (`tho` = thơ / thờ / thổ) – trừ khi thuộc một nhóm đồng nghĩa.
fn sub_terms(name: &str) -> Terms {
    let k = simple_key(name);
    let k = k.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ').to_string();
    let mut v = Vec::new();
    if k.contains(' ') || k.chars().count() >= 6 {
        v.push((k.clone(), 1.0));
    }
    let padded = format!(" {k} ");
    for (group, strong, weak) in SUB_ALIASES {
        if padded.contains(&format!(" {group} ")) {
            v.extend(strong.iter().map(|w| (w.to_string(), 1.0)));
            v.extend(weak.iter().map(|w| (w.to_string(), 0.5)));
        }
    }
    v
}

/// Điểm của tác phẩm cho một thư mục con: mỗi trường (tag ×3, tiêu đề/mô tả ×2, đường dẫn ×1) lấy từ khoá mạnh
/// nhất khớp nguyên cụm. `fields`: (trọng số, khoá bỏ dấu có khoảng trắng hai đầu).
fn sub_score(terms: &[(String, f64)], fields: &[(f64, String)]) -> f64 {
    fields
        .iter()
        .map(|(w, text)| {
            w * terms.iter().filter(|(t, _)| text.contains(&format!(" {t} "))).map(|(_, s)| *s).fold(0.0, f64::max)
        })
        .sum()
}

/// Từ khoá (bỏ dấu) kèm độ mạnh.
type Terms = Vec<(String, f64)>;
/// Thư mục con (tên) kèm từ khoá của nó.
type SubDirs = Vec<(String, Terms)>;

/// Thư mục con trực tiếp của `dir` (tên), kèm từ khoá của từng cái.
fn sub_dirs(dir: &Path) -> SubDirs {
    let mut v: SubDirs = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && !n.starts_with('$'))
        .map(|n| {
            crate::progress::pulse();
            let t = sub_terms(&n);
            (n, t)
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// Đi xuống các thư mục con (người dùng tự chia, tối đa [`SUB_DEPTH`] cấp) của thư mục thể loại `top` khi tác phẩm
/// khớp rõ một thư mục con; tính cả thư mục con bị chặn đã không còn trên đĩa. Hoà điểm: có thư mục bị chặn thì
/// chọn nó (người dùng đã nói không muốn thể loại đó), không thì dừng ở cấp hiện tại.
fn descend(
    root: &Path,
    mirrors: &[PathBuf],
    top: &Path,
    u: &Unit,
    blocked: &crate::blocked::Blocked,
    cache: &mut HashMap<PathBuf, SubDirs>,
) -> PathBuf {
    let mut dir = top.to_path_buf();
    let mut fields: Option<Vec<(f64, String)>> = None;
    for _ in 0..SUB_DEPTH {
        let keys = crate::blocked::dir_keys(root, &dir);
        // Thư mục con ở ổ này cộng thư mục con cùng chỗ ở các ổ thư viện khác (thể loại trải trên nhiều ổ).
        let mut subs = cache
            .entry(dir.clone())
            .or_insert_with(|| {
                let mut v = sub_dirs(&dir);
                let rel = keys.join("/");
                for m in mirrors {
                    let Some(md) = crate::blocked::find_dir(m, &rel) else { continue };
                    for (n, t) in sub_dirs(&md) {
                        let k = crate::blocked::name_key(&n);
                        if !v.iter().any(|(x, _)| crate::blocked::name_key(x) == k) {
                            v.push((n, t));
                        }
                    }
                }
                v
            })
            .clone();
        for b in blocked.children(&keys) {
            let k = crate::blocked::name_key(&b);
            if !subs.iter().any(|(n, _)| crate::blocked::name_key(n) == k) {
                let t = sub_terms(&b);
                subs.push((b, t));
            }
        }
        if subs.iter().all(|(_, t)| t.is_empty()) {
            break;
        }
        let f = fields.get_or_insert_with(|| {
            [(3.0, &u.subjects), (2.0, &u.title), (1.0, &u.path)]
                .into_iter()
                .filter(|(_, t)| !t.trim().is_empty())
                .map(|(w, t)| (w, format!(" {} ", simple_key(t))))
                .collect()
        });
        // (điểm, bị chặn, tên); `tie`: hai thư mục cùng hạng tốt nhất → không chắc.
        let mut best: Option<(f64, bool, &str)> = None;
        let mut tie = false;
        for (n, terms) in &subs {
            let s = sub_score(terms, f);
            if s < SUB_MIN_SCORE {
                continue;
            }
            let mut k = keys.clone();
            k.push(crate::blocked::name_key(n));
            let is_blocked = blocked.covers(&k);
            match best {
                Some((bs, bb, _)) if s < bs || (s == bs && bb && !is_blocked) => {}
                Some((bs, bb, _)) if s == bs && bb == is_blocked => tie = true,
                _ => {
                    best = Some((s, is_blocked, n));
                    tie = false;
                }
            }
        }
        match best {
            Some((_, _, n)) if !tie => dir = dir.join(n),
            _ => break,
        }
    }
    dir
}

/// Thể loại chắc chắn (hoặc `None` → "Khác").
fn decide(subjects: &str, title: &str, path: &str) -> Option<&'static RuleDef> {
    let sc = score(subjects, title, path);
    let (top, s1) = *sc.first()?;
    let s2 = sc.get(1).map_or(0.0, |x| x.1);
    (s1 >= MIN_SCORE && s1 - s2 >= MIN_MARGIN).then_some(top)
}

/// Đọc `dc:subject`, `dc:title`, `dc:description` từ `metadata.opf` của Calibre.
fn read_opf(path: &Path) -> (String, String, String) {
    let Ok(bytes) = std::fs::read(path) else { return Default::default() };
    let s = String::from_utf8_lossy(&bytes);
    let grab = |tag: &str| -> Vec<String> {
        let re = Regex::new(&format!(r"(?is)<dc:{tag}[^>]*>(.*?)</dc:{tag}>")).unwrap();
        re.captures_iter(&s)
            .map(|c| {
                let t = Regex::new(r"<[^>]+>").unwrap().replace_all(&c[1], " ").into_owned();
                t.replace("&amp;", "&")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&quot;", "\"")
                    .replace("&#39;", "'")
                    .trim()
                    .to_string()
            })
            .collect()
    };
    let subjects: Vec<String> =
        grab("subject").into_iter().filter(|x| !NOISE_TAGS.contains(&x.to_lowercase().as_str())).collect();
    let mut desc = grab("description").join(" ");
    desc.truncate(desc.char_indices().nth(600).map_or(desc.len(), |(i, _)| i));
    (subjects.join("; "), grab("title").join(" "), desc)
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// File đi kèm mang đuôi giống sách nhưng không phải sách: mô tả trang tải kèm theo (`description.txt`),
/// metadata Kindle (`metadata.kfx`), file ẩn (`.progress.txt`). Coi là sách thì mỗi thư mục Calibre thành hai
/// "tác phẩm" (mất tag của `metadata.opf`) và hàng nghìn `description (n).txt` dồn vào `Khác`.
fn is_sidecar(p: &Path) -> bool {
    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    name.starts_with('.') || matches!(stem.as_str(), "description" | "metadata")
}

pub(crate) fn is_book(p: &Path) -> bool {
    BOOK_EXT.contains(&ext_of(p).as_str()) && !is_sidecar(p)
}

/// Luồng đọc hộp thư đến song song: ổ USB và ổ mạng chậm ở *độ trễ* mỗi lần hỏi đĩa (vài ms), không phải
/// băng thông, nên đọc nhiều thư mục cùng lúc nhanh hơn hẳn đọc lần lượt (đo trên SSD USB: ~6,5×).
const IO_THREADS: usize = 8;
/// Số thư mục đọc mỗi lô (giữa hai lô: báo tiến trình, kiểm tra huỷ).
const READ_CHUNK: usize = 64;
/// Số tác phẩm chuyển mỗi lô (giữa hai lô: ghi DB, báo tiến trình, kiểm tra huỷ).
const MOVE_CHUNK: usize = 25;

/// Khoá tên file theo cách hệ điều hành so tên (Windows/macOS không phân biệt hoa thường).
fn name_key(n: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        n.to_lowercase()
    } else {
        n.to_string()
    }
}

/// Danh sách mục của một thư mục đích, đọc **một lần** rồi giữ trong bộ nhớ: kiểm tra bản trùng và chọn tên
/// `(n)` không phải hỏi đĩa từng file (ổ USB: mỗi lần hỏi ~4 ms; trước đây `description (603).txt` phải thử
/// 603 tên). Việc chuyển file vẫn tự kiểm tra không ghi đè (`moves::move_file`).
struct DirListing(HashMap<String, u64>);

impl DirListing {
    fn read(dir: &Path) -> Self {
        let mut m = HashMap::new();
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            crate::progress::pulse();
            let size = match e.metadata() {
                Ok(md) if md.is_file() => md.len(),
                _ => u64::MAX, // thư mục / không đọc được: tên đã có người dùng, nhưng không bao giờ là "bản trùng"
            };
            m.insert(name_key(&e.file_name().to_string_lossy()), size);
        }
        DirListing(m)
    }

    fn size_of(&self, name: &str) -> Option<u64> {
        self.0.get(&name_key(name)).copied()
    }

    /// Tên chưa dùng (`name`, `name (1)`… như `moves::unique_dest`), giữ chỗ luôn cho file sắp chuyển tới.
    fn reserve(&mut self, name: &str, size: u64) -> String {
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 => (&name[..i], &name[i..]),
            _ => (name, ""),
        };
        let pick = std::iter::once(name.to_string())
            .chain((1..).map(|n| format!("{stem} ({n}){ext}")))
            .find(|n| !self.0.contains_key(&name_key(n)))
            .unwrap();
        self.0.insert(name_key(&pick), size);
        pick
    }
}

/// Đọc một thư mục hộp thư đến: các tác phẩm trong đó, và thư mục có phải giữ lại (có truyện tranh) không.
fn read_inbox_dir(dir: &Path, inbox: &Path) -> (Vec<Unit>, bool) {
    crate::progress::working_on(dir);
    let Ok(rd) = std::fs::read_dir(dir) else { return (Vec::new(), false) };
    let mut groups: BTreeMap<String, Vec<(PathBuf, u64)>> = BTreeMap::new();
    let mut opf: Option<PathBuf> = None;
    for e in rd.flatten() {
        crate::progress::pulse();
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let p = e.path();
        if p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("metadata.opf")) {
            opf = Some(p);
        } else if is_book(&p) {
            // Windows trả sẵn dung lượng khi liệt kê thư mục – khỏi hỏi đĩa riêng cho từng file lúc chuyển.
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            let stem = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            groups.entry(stem).or_default().push((p, size));
        }
    }
    let mut units = Vec::new();
    let mut keep = false;
    if groups.is_empty() {
        return (units, keep);
    }
    let (osubj, otitle, odesc) = opf.as_deref().map(read_opf).unwrap_or_default();
    let single = groups.len() == 1;
    let rel = dir.strip_prefix(inbox).unwrap_or(dir);
    let path_text =
        rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
    for (_, files) in groups {
        if files.iter().any(|(f, _)| COMIC_EXT.contains(&ext_of(f).as_str())) {
            keep = true; // truyện tranh: để nguyên cho luồng bộ truyện
            continue;
        }
        let stem = files[0].0.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let title = if single && !otitle.is_empty() { otitle.clone() } else { stem };
        let title = if single { format!("{title} {odesc}") } else { title };
        units.push(Unit {
            subjects: if single { osubj.clone() } else { String::new() },
            title,
            path: path_text.clone(),
            files,
        });
    }
    (units, keep)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TriageReport {
    pub dry_run: bool,
    /// Số tác phẩm (nhóm file cùng tên) được xử lý.
    pub units: usize,
    pub moved_files: usize,
    /// Số tác phẩm theo thư mục đích.
    pub by_category: BTreeMap<String, usize>,
    pub to_other: usize,
    pub created_dirs: Vec<String>,
    pub duplicates_trashed: usize,
    /// File không phải sách (ảnh, âm thanh, video, .opf…) đã vào Thùng rác lẻ (ngoài các thư mục bị xoá cả cụm).
    pub junk_files_trashed: usize,
    pub dirs_trashed: usize,
    /// File sách thuộc thể loại bị chặn đã vào thẳng Thùng rác (dry-run: số file sẽ vào).
    pub blocked_trashed: usize,
    /// Số tác phẩm theo thư mục bị chặn (`A/B` như trong cấu hình).
    pub blocked: BTreeMap<String, usize>,
    pub failures: Vec<String>,
    /// Bị huỷ giữa chừng (các tác phẩm đã chuyển vẫn giữ nguyên).
    pub cancelled: bool,
}

#[derive(Default)]
pub struct TriageOptions<'a> {
    pub dry_run: bool,
    /// Nhật ký JSONL `{from,to}` (chỉ ghi khi không phải dry-run).
    pub journal: Option<&'a Path>,
    /// Huỷ giữa chừng: dừng sau tác phẩm đang chuyển (phần đã chuyển giữ nguyên, lần sau làm tiếp phần còn lại).
    pub cancel: Option<&'a AtomicBool>,
    /// Tiến trình theo tác phẩm (`phase = "triage"`).
    pub progress: Option<OnStep<'a>>,
    /// Có DB: cập nhật đường dẫn sách ngay khi chuyển (giữ tiến độ đọc, thể loại hiện ngay, lần quét sau không
    /// phải đọc lại metadata).
    pub conn: Option<&'a Connection>,
}

/// Ghi nhận vào DB file vừa chuyển (`to = Some`) hoặc vừa vào Thùng rác (`to = None`).
fn sync_db(conn: &Connection, cfg: &Config, from: &Path, to: Option<&Path>) {
    let id: Option<i64> = conn
        .query_row("SELECT id FROM books WHERE path = ?1", [from.to_string_lossy()], |r| r.get(0))
        .optional()
        .unwrap_or(None);
    let Some(id) = id else { return };
    let r = match to {
        Some(to) => crate::scan::relocate(conn, cfg, id, to),
        None => crate::db::set_status(conn, id, "trashed"),
    };
    if let Err(e) = r {
        tracing::warn!("phân loại: không cập nhật được DB cho {}: {e}", from.display());
    }
}

struct Unit {
    /// (đường dẫn, dung lượng lúc đọc hộp thư đến)
    files: Vec<(PathBuf, u64)>,
    subjects: String,
    title: String,
    path: String,
}

/// Thư mục thể loại có sẵn hợp với luật (tên giống hệt, hoặc chứa khoá đồng nghĩa), nếu có.
fn find_existing(def: &RuleDef, cats: &[(String, PathBuf)]) -> Option<PathBuf> {
    let want = simple_key(def.name);
    let keyed: Vec<(String, &PathBuf)> = cats
        .iter()
        .map(|(n, p)| {
            let k = simple_key(n);
            // bỏ số thứ tự đầu tên ("2. Kinh tế…")
            let k = k.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ').to_string();
            (k, p)
        })
        .collect();
    if let Some((_, p)) = keyed.iter().find(|(k, _)| *k == want) {
        return Some((*p).clone());
    }
    keyed
        .iter()
        .find(|(k, _)| {
            let padded = format!(" {k} ");
            def.aliases.iter().any(|a| padded.contains(&format!(" {a} ")))
        })
        .map(|(_, p)| (*p).clone())
}

/// Các ngăn của một chữ trong `Khác`.
struct Letter {
    /// Ngăn đang điền.
    n: usize,
    dir: PathBuf,
    cur: DirListing,
    /// Mọi file trong mọi ngăn của chữ này: tên → (dung lượng, đường dẫn), để nhận ra bản trùng y hệt.
    all: HashMap<String, (u64, PathBuf)>,
}

impl Letter {
    /// Bản trùng y hệt (cùng tên, cùng dung lượng) đã có trong một ngăn nào đó.
    fn duplicate(&self, name: &str, size: u64) -> Option<PathBuf> {
        self.all.get(&name_key(name)).filter(|(s, _)| *s == size).map(|(_, p)| p.clone())
    }

    /// Giữ chỗ trong ngăn hiện tại, trả về đường dẫn đích.
    fn reserve(&mut self, name: &str, size: u64) -> PathBuf {
        let to = self.dir.join(self.cur.reserve(name, size));
        self.all.entry(name_key(name)).or_insert((size, to.clone()));
        to
    }
}

/// Chọn ngăn cho sách vào `Khác`: đọc mỗi chữ một lần, giữ trong bộ nhớ.
pub(crate) struct Shelves {
    other: PathBuf,
    dry_run: bool,
    letters: HashMap<String, Letter>,
}

impl Shelves {
    pub(crate) fn new(other: &Path, dry_run: bool) -> Self {
        Shelves { other: other.to_path_buf(), dry_run, letters: HashMap::new() }
    }

    /// Ngăn cho một tác phẩm `k` file chữ `letter`: ngăn cuối, hoặc ngăn mới nếu thêm vào sẽ quá
    /// [`OTHER_SHELF_MAX`] (ngăn đang rỗng thì vẫn nhận, để không tách các định dạng).
    fn place(&mut self, letter: &str, k: usize) -> Result<&mut Letter> {
        let dry = self.dry_run;
        if !self.letters.contains_key(letter) {
            let ldir = self.other.join(letter);
            let mut all = HashMap::new();
            let mut last: Option<(usize, PathBuf, DirListing)> = None;
            for e in std::fs::read_dir(&ldir).into_iter().flatten().flatten() {
                crate::progress::pulse();
                if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let Some(n) = other_shelf_number(letter, &e.file_name().to_string_lossy()) else { continue };
                let listing = DirListing::read(&e.path());
                for (key, size) in &listing.0 {
                    if *size != u64::MAX {
                        all.insert(key.clone(), (*size, e.path().join(key)));
                    }
                }
                if last.as_ref().is_none_or(|(m, _, _)| n > *m) {
                    last = Some((n, e.path(), listing));
                }
            }
            let (n, dir, cur) = last.unwrap_or_else(|| {
                let d = ldir.join(other_shelf_name(letter, 1));
                (1, d, DirListing(HashMap::new()))
            });
            self.letters.insert(letter.to_string(), Letter { n, dir, cur, all });
        }
        let l = self.letters.get_mut(letter).expect("vừa thêm");
        let used = l.cur.0.len();
        if used > 0 && used + k > OTHER_SHELF_MAX {
            l.n += 1;
            l.dir = self.other.join(letter).join(other_shelf_name(letter, l.n));
            l.cur = DirListing::read(&l.dir);
        }
        if !dry {
            std::fs::create_dir_all(&l.dir)?;
        }
        Ok(l)
    }

    /// Thư mục ngăn cho một file (chuyển tay vào `Khác`).
    pub(crate) fn dir_for(&mut self, file_name: &str) -> Result<PathBuf> {
        Ok(self.place(&other_bucket(file_name), 1)?.dir.clone())
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BucketReport {
    pub dry_run: bool,
    /// File sách đã chuyển từ gốc `Khác` vào thư mục con.
    pub moved_files: usize,
    pub failures: Vec<String>,
    pub cancelled: bool,
}

/// Xếp các file sách đang nằm thẳng ở gốc thư mục `Khác` hoặc thẳng trong thư mục chữ cái (thư viện cũ, hoặc người
/// dùng tự thả vào) vào ngăn theo [`other_bucket`]. Sau lần đầu chỉ còn phải đọc gốc `Khác` và 28 thư mục chữ cái
/// (toàn thư mục con) nên lần gọi sau gần như không tốn gì. Không ghi đè (trùng tên thêm `(n)`); dùng chung nhật ký / DB / huỷ của [`TriageOptions`].
pub fn bucket_other(cfg: &Config, opts: &TriageOptions) -> Result<BucketReport> {
    if cfg.read_only {
        return Err(Error::ReadOnly);
    }
    let mut rep = BucketReport { dry_run: opts.dry_run, ..Default::default() };
    let others = classify::list_categories(cfg)?.into_iter().find(|c| is_other_dir(&c.name)).map(|c| c.paths);
    for other in others.unwrap_or_default() {
        bucket_other_dir(cfg, &other, opts, &mut rep)?;
        if rep.cancelled {
            break;
        }
    }
    Ok(rep)
}

/// [`bucket_other`] cho thư mục `Khác` của một ổ.
fn bucket_other_dir(cfg: &Config, other: &Path, opts: &TriageOptions, rep: &mut BucketReport) -> Result<()> {
    let other = other.to_path_buf();
    crate::progress::working_on(&other);
    // File sách nằm thẳng ở gốc `Khác` hoặc thẳng trong thư mục chữ cái (`Khác/T/x.epub`) – chưa vào ngăn.
    let mut files: Vec<(PathBuf, String, u64)> = Vec::new();
    let mut letter_dirs = Vec::new();
    let collect = |dir: &Path, files: &mut Vec<(PathBuf, String, u64)>, letter_dirs: &mut Vec<PathBuf>| {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            crate::progress::pulse();
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                if dir == other && is_other_bucket(&e.file_name().to_string_lossy()) {
                    letter_dirs.push(e.path());
                }
                continue;
            }
            let p = e.path();
            if !t.is_file() || !is_book(&p) {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            files.push((p, name, size));
        }
    };
    collect(&other, &mut files, &mut letter_dirs);
    for d in std::mem::take(&mut letter_dirs) {
        collect(&d, &mut files, &mut Vec::new());
    }
    if files.is_empty() {
        return Ok(());
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    let cancelled = || opts.cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    let report = |done: usize, total: usize| {
        if let Some(f) = opts.progress {
            f(Step { phase: "triage_other", done, total });
        }
    };
    let total = files.len();
    report(0, total);
    let mut journal = match (opts.dry_run, opts.journal) {
        (false, Some(p)) => {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            Some(std::fs::OpenOptions::new().create(true).append(true).open(p)?)
        }
        _ => None,
    };
    let conn = if opts.dry_run { None } else { opts.conn };
    let mut tx = conn.map(crate::db::write_tx).transpose()?;
    let mut shelves = Shelves::new(&other, opts.dry_run);
    for (i, (f, name, size)) in files.iter().enumerate() {
        if i > 0 && i.is_multiple_of(MOVE_CHUNK) {
            if let Some(t) = tx.take() {
                t.commit()?;
                tx = conn.map(crate::db::write_tx).transpose()?;
            }
            report(i, total);
        }
        if cancelled() {
            rep.cancelled = true;
            break;
        }
        let to = shelves.place(&other_bucket(name), 1)?.reserve(name, *size);
        if opts.dry_run {
            rep.moved_files += 1;
            continue;
        }
        crate::progress::working_on(f);
        match moves::move_file(f, &to) {
            Ok(()) => {
                rep.moved_files += 1;
                if let Some(t) = tx.as_ref() {
                    sync_db(t, cfg, f, Some(&to));
                }
                if let Some(j) = journal.as_mut() {
                    let line = serde_json::json!({"from": f.to_string_lossy(), "to": to.to_string_lossy()});
                    let _ = writeln!(j, "{line}");
                }
            }
            Err(e) => rep.failures.push(format!("{}: {e}", f.display())),
        }
    }
    if let Some(t) = tx.take() {
        t.commit()?;
    }
    report(rep.moved_files.min(total), total);
    Ok(())
}

impl TriageReport {
    fn merge(&mut self, o: TriageReport) {
        self.units += o.units;
        self.moved_files += o.moved_files;
        for (k, v) in o.by_category {
            *self.by_category.entry(k).or_default() += v;
        }
        self.to_other += o.to_other;
        for d in o.created_dirs {
            if !self.created_dirs.contains(&d) {
                self.created_dirs.push(d);
            }
        }
        self.duplicates_trashed += o.duplicates_trashed;
        self.junk_files_trashed += o.junk_files_trashed;
        self.dirs_trashed += o.dirs_trashed;
        self.blocked_trashed += o.blocked_trashed;
        for (k, v) in o.blocked {
            *self.blocked.entry(k).or_default() += v;
        }
        self.failures.extend(o.failures);
        self.cancelled |= o.cancelled;
    }
}

/// Tự phân loại toàn bộ hộp thư đến theo mô tả ở đầu module – hộp thư đến của từng ổ thư viện. Sách ở lại ổ của
/// hộp thư đến chứa nó (đổi tên tức thì, không tốn chỗ); ổ sắp đầy thì sang ổ thư viện còn chỗ (`volumes.rs`).
pub fn run(cfg: &Config, trasher: &dyn Trasher, opts: &TriageOptions) -> Result<TriageReport> {
    if cfg.read_only {
        return Err(Error::ReadOnly);
    }
    let roots = cfg.library_roots();
    if roots.is_empty() {
        return Err(Error::NoLibrary);
    }
    let mut placer = crate::volumes::Placer::new(cfg);
    let mut rep = TriageReport { dry_run: opts.dry_run, ..Default::default() };
    for root in &roots {
        if !root.is_dir() {
            continue;
        }
        rep.merge(run_root(cfg, root, &roots, trasher, opts, &mut placer)?);
        if rep.cancelled {
            break;
        }
    }
    Ok(rep)
}

fn run_root(
    cfg: &Config,
    root: &Path,
    roots: &[PathBuf],
    trasher: &dyn Trasher,
    opts: &TriageOptions,
    placer: &mut crate::volumes::Placer,
) -> Result<TriageReport> {
    let mirrors: Vec<PathBuf> = roots.iter().filter(|r| r.as_path() != root).cloned().collect();
    let inbox = root.join(&cfg.inbox_name);
    let mut rep = TriageReport { dry_run: opts.dry_run, ..Default::default() };
    if !inbox.is_dir() {
        return Ok(rep);
    }
    // Thể loại của mọi ổ, đường dẫn tính trên ổ này (có thể chưa có thư mục ở ổ này – tạo khi cần).
    let mut cats: Vec<(String, PathBuf)> = classify::list_categories(cfg)?
        .into_iter()
        .map(|c| {
            let p = c.paths.iter().find(|p| p.starts_with(root)).cloned().unwrap_or_else(|| root.join(&c.name));
            (c.name, p)
        })
        .collect();
    // Thể loại đã có thư mục ở một ổ nào đó (để không báo "tạo thư mục mới" khi chỉ là thêm ở ổ khác).
    let mut known: HashSet<String> = cats.iter().map(|(n, _)| fold(n)).collect();
    // Thể loại bị chặn mà thư mục đã được dọn khỏi đĩa: vẫn là "thư mục có sẵn" của luật tương ứng, để sách cùng
    // thể loại được nhận ra (và vào Thùng rác) thay vì tạo lại thư mục theo tên mặc định.
    let blocked = crate::blocked::Blocked::new(cfg);
    let mapping = crate::tagmap::Mapping::new(cfg);
    for name in blocked.top_levels() {
        let k = crate::blocked::name_key(&name);
        if !cats.iter().any(|(n, _)| crate::blocked::name_key(n) == k) {
            cats.push((name.clone(), root.join(&name)));
        }
    }

    let report = |done: usize, total: usize| {
        if let Some(f) = opts.progress {
            f(Step { phase: "triage", done, total });
        }
    };
    let cancelled = || opts.cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    report(0, 0);
    // 1) Gom tác phẩm: trong mỗi thư mục, các file sách cùng tên gốc là một tác phẩm.
    let mut units: Vec<Unit> = Vec::new();
    let mut keep_dirs: HashSet<PathBuf> = HashSet::new();
    let mut junk_dirs: Vec<PathBuf> = Vec::new();
    // Liệt kê thư mục trước (nhanh, chỉ đọc danh mục) để có tổng "x / y"; đọc từng thư mục (kèm metadata.opf)
    // mới chậm trên ổ chậm, nên báo tiến trình theo thời gian và cho huỷ giữa chừng.
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut walker = walkdir::WalkDir::new(&inbox).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        crate::progress::pulse();
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_dir() {
            continue;
        }
        if dirs.len().is_multiple_of(256) && cancelled() {
            rep.cancelled = true;
            return Ok(rep);
        }
        let dir = entry.path().to_path_buf();
        if dir != inbox && JUNK_DIRS.iter().any(|j| entry.file_name().to_string_lossy().eq_ignore_ascii_case(j)) {
            junk_dirs.push(dir);
            walker.skip_current_dir();
            continue;
        }
        dirs.push(dir);
    }
    let n_dirs = dirs.len();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(IO_THREADS)
        .build()
        .map_err(|e| Error::msg(format!("không tạo được luồng phân loại: {e}")))?;
    let mut last_report: Option<std::time::Instant> = None;
    for (ci, chunk) in dirs.chunks(READ_CHUNK).enumerate() {
        if cancelled() {
            rep.cancelled = true;
            return Ok(rep);
        }
        if last_report.is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(250)) {
            if let Some(f) = opts.progress {
                f(Step { phase: "triage_read", done: ci * READ_CHUNK, total: n_dirs });
            }
            last_report = Some(std::time::Instant::now());
        }
        let read: Vec<(Vec<Unit>, bool)> =
            pool.install(|| chunk.par_iter().map(|d| read_inbox_dir(d, &inbox)).collect());
        for (dir, (us, keep)) in chunk.iter().zip(read) {
            if keep {
                keep_dirs.insert(dir.clone());
            }
            units.extend(us);
        }
    }

    // 2) Phân loại + di chuyển.
    let mut journal = match (opts.dry_run, opts.journal) {
        (false, Some(p)) => {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            Some(std::fs::OpenOptions::new().create(true).append(true).open(p)?)
        }
        _ => None,
    };
    let mut dest_cache: HashMap<String, PathBuf> = HashMap::new();
    let conn = if opts.dry_run { None } else { opts.conn };
    let total = units.len();
    report(0, total);
    // Đoán thể loại (thuần CPU) song song cho cả hộp thư đến.
    let decisions: Vec<Option<&'static RuleDef>> = pool.install(|| {
        units
            .par_iter()
            .map(|u| {
                crate::progress::pulse();
                decide(&u.subjects, &u.title, &u.path)
            })
            .collect()
    });
    // Gom cập nhật DB theo lô (đỡ fsync); file đã chuyển mà lô chưa ghi thì lần quét sau tự nhận ra chỗ mới.
    // Mở giao dịch (giữ chỗ ghi) sau bước đoán thể loại, để không chặn kết nối khác suốt bước đó.
    let mut tx = conn.map(crate::db::write_tx).transpose()?;
    let mut listings: HashMap<PathBuf, DirListing> = HashMap::new();
    let mut shelves: HashMap<PathBuf, Shelves> = HashMap::new();
    let mut sub_cache: HashMap<PathBuf, SubDirs> = HashMap::new();
    let mut created: HashSet<PathBuf> = HashSet::new();
    for (ci, chunk) in units.chunks(MOVE_CHUNK).enumerate() {
        let start = ci * MOVE_CHUNK;
        if start > 0 {
            if let Some(t) = tx.take() {
                t.commit()?;
                tx = conn.map(crate::db::write_tx).transpose()?;
            }
            report(start, total);
        }
        if cancelled() {
            rep.cancelled = true;
            break;
        }
        // a) Lên kế hoạch (tuần tự, chỉ trong bộ nhớ): thư mục đích, bản trùng, tên không đụng hàng.
        let mut to_move: Vec<(&Path, PathBuf)> = Vec::new();
        let mut dups: Vec<(&Path, PathBuf, u64)> = Vec::new();
        // (file, thư mục bị chặn) → Thùng rác.
        let mut to_block: Vec<(&Path, String)> = Vec::new();
        for (k, u) in chunk.iter().enumerate() {
            crate::progress::pulse();
            rep.units += 1;
            // Ánh xạ tag → thể loại của người dùng (tagmap.rs) đi trước các luật có sẵn. Tác phẩm không có
            // metadata.opf: dùng tag đọc từ chính file nếu lần quét trước đã có trong DB.
            let mapped = if mapping.is_empty() {
                None
            } else if u.subjects.is_empty() {
                tx.as_ref()
                    .and_then(|t| {
                        t.query_row(
                            "SELECT subjects FROM books WHERE path = ?1",
                            [u.files[0].0.to_string_lossy()],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()
                        .ok()
                        .flatten()
                    })
                    .and_then(|s| crate::tagmap::triage_target(&mapping, root, &s))
            } else {
                crate::tagmap::triage_target(&mapping, root, &u.subjects)
            };
            let decision = if mapped.is_some() { None } else { decisions[start + k] };
            let (label, cat_dir) = match (&mapped, decision) {
                (Some(dir), _) => {
                    let rel = dir.strip_prefix(root).unwrap_or(dir);
                    let label = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/");
                    (label, dir.clone())
                }
                (None, Some(def)) => {
                    let dir = dest_cache
                        .entry(def.name.to_string())
                        .or_insert_with(|| find_existing(def, &cats).unwrap_or_else(|| root.join(def.name)));
                    (dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), dir.clone())
                }
                (None, None) => {
                    let dir = dest_cache.entry(OTHER_DIR.to_string()).or_insert_with(|| {
                        cats.iter()
                            .find(|(n, _)| is_other_dir(n))
                            .map(|(_, p)| p.clone())
                            .unwrap_or_else(|| root.join(OTHER_DIR))
                    });
                    rep.to_other += 1;
                    (OTHER_DIR.to_string(), dir.clone())
                }
            };
            let is_other = mapped.is_none() && decision.is_none();
            // Thư mục con người dùng tự chia (nhiều cấp) – kể cả thư mục con bị chặn đã dọn khỏi đĩa. Đích do ánh
            // xạ tag chỉ định thì dùng đúng thư mục đó.
            let dest_dir = if is_other || mapped.is_some() {
                cat_dir.clone()
            } else {
                descend(root, &mirrors, &cat_dir, u, &blocked, &mut sub_cache)
            };
            if !is_other && !blocked.is_empty() {
                let keys = crate::blocked::dir_keys(root, &dest_dir);
                if let Some(rel) = blocked.entry_for(&keys) {
                    // Thể loại bị chặn: không chuyển, không tạo thư mục – vào thẳng Thùng rác.
                    *rep.blocked.entry(rel.to_string()).or_default() += 1;
                    for (f, _) in &u.files {
                        if opts.dry_run {
                            rep.blocked_trashed += 1;
                        } else {
                            to_block.push((f, rel.to_string()));
                        }
                    }
                    continue;
                }
            }
            *rep.by_category.entry(label.clone()).or_default() += 1;
            // Chọn ổ: ở lại ổ này (đổi tên tức thì), trừ khi ổ sắp đầy (volumes.rs).
            let unit_size: u64 = u.files.iter().map(|(_, s)| *s).sum();
            let dest_dir = placer.place(&u.files[0].0, unit_size, &dest_dir);
            let cat_dir = if is_other { dest_dir.clone() } else { cat_dir };
            if !dest_dir.is_dir() && created.insert(dest_dir.clone()) {
                // Thư mục chưa có ở ổ này: tạo mới.
                if !opts.dry_run {
                    std::fs::create_dir_all(&dest_dir)?;
                }
                if mapped.is_none() && known.insert(fold(&label)) {
                    rep.created_dirs.push(label.clone());
                    if !cats.iter().any(|(_, p)| *p == cat_dir) {
                        cats.push((label.clone(), cat_dir.clone()));
                    }
                }
            }
            if is_other {
                // `Khác`: vào ngăn của chữ cái đầu (mọi định dạng của tác phẩm vào cùng một ngăn).
                let first = u.files[0].0.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                crate::progress::working_on(&cat_dir);
                let shelf = shelves
                    .entry(cat_dir.clone())
                    .or_insert_with(|| Shelves::new(&cat_dir, opts.dry_run))
                    .place(&other_bucket(&first), u.files.len())?;
                for (f, size) in &u.files {
                    let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if let Some(same) = shelf.duplicate(&name, *size) {
                        if opts.dry_run {
                            rep.duplicates_trashed += 1;
                        } else {
                            dups.push((f, same, *size));
                        }
                        continue;
                    }
                    let to = shelf.reserve(&name, *size);
                    if opts.dry_run {
                        rep.moved_files += 1;
                    } else {
                        to_move.push((f, to));
                    }
                }
                continue;
            }
            if !listings.contains_key(&dest_dir) {
                // Lần đầu gặp thư mục đích này: đọc danh sách file một lần.
                crate::progress::working_on(&dest_dir);
                listings.insert(dest_dir.clone(), DirListing::read(&dest_dir));
            }
            let listing = listings.get_mut(&dest_dir).expect("vừa thêm");
            for (f, size) in &u.files {
                let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if listing.size_of(&name) == Some(*size) {
                    // Bản trùng y hệt (cùng tên, cùng dung lượng) đã nằm ở thư mục đích.
                    if opts.dry_run {
                        rep.duplicates_trashed += 1;
                    } else {
                        dups.push((f, dest_dir.join(&name), *size));
                    }
                    continue;
                }
                let to = dest_dir.join(listing.reserve(&name, *size));
                if opts.dry_run {
                    rep.moved_files += 1;
                } else {
                    to_move.push((f, to));
                }
            }
        }
        // b) Chuyển file + ghi DB, nhật ký. Tuần tự: đổi tên vào cùng một thư mục bị NTFS xếp hàng, chạy song
        //    song còn chậm hơn (đo trên SSD USB: 1,76 vs 1,4 ms/file). `move_file` vẫn không bao giờ ghi đè.
        for (f, to) in &to_move {
            crate::progress::working_on(f);
            match moves::move_file(f, to) {
                Ok(()) => {
                    rep.moved_files += 1;
                    if let Some(t) = tx.as_ref() {
                        sync_db(t, cfg, f, Some(to));
                    }
                    if let Some(j) = journal.as_mut() {
                        let line = serde_json::json!({"from": f.to_string_lossy(), "to": to.to_string_lossy()});
                        let _ = writeln!(j, "{line}");
                    }
                }
                Err(e) => rep.failures.push(format!("{}: {e}", f.display())),
            }
        }
        // c) Bản trùng vào Thùng rác – chỉ khi bản ở thư mục đích thật sự còn đó (có thể là file vừa chuyển
        //    trong cùng lô mà bị lỗi).
        for (f, same, size) in dups {
            crate::progress::working_on(f);
            if !std::fs::metadata(&same).is_ok_and(|m| m.is_file() && m.len() == size) {
                continue; // để nguyên trong hộp thư đến, lần sau xét lại
            }
            match trasher.trash(f) {
                Ok(()) => {
                    rep.duplicates_trashed += 1;
                    if let Some(t) = tx.as_ref() {
                        sync_db(t, cfg, f, None);
                    }
                }
                Err(e) => rep.failures.push(format!("{}: {e}", f.display())),
            }
        }
        // d) Thể loại bị chặn → Thùng rác (ghi nhật ký `to: null` để biết vì sao file biến mất).
        for (f, rel) in to_block {
            crate::progress::working_on(f);
            match trasher.trash(f) {
                Ok(()) => {
                    rep.blocked_trashed += 1;
                    if let Some(t) = tx.as_ref() {
                        sync_db(t, cfg, f, None);
                    }
                    if let Some(j) = journal.as_mut() {
                        let line = serde_json::json!({"from": f.to_string_lossy(), "to": null, "blocked": rel});
                        let _ = writeln!(j, "{line}");
                    }
                }
                Err(e) => rep.failures.push(format!("{}: {e}", f.display())),
            }
        }
    }

    if let Some(t) = tx.take() {
        t.commit()?;
    }
    report(rep.units, total);
    // 3) Dọn: thư mục không còn sách / file nén / truyện thì vào Thùng rác; file rác lẻ ở gốc hộp thư đến.
    if opts.dry_run || rep.cancelled {
        return Ok(rep);
    }
    if let Some(f) = opts.progress {
        f(Step { phase: "triage_clean", done: 0, total: 0 });
    }
    let mut keepers: HashSet<PathBuf> = keep_dirs;
    let mut keep_all = HashSet::new();
    let not_junk_dir = |e: &walkdir::DirEntry| {
        !(e.file_type().is_dir()
            && e.depth() > 0
            && JUNK_DIRS.iter().any(|j| e.file_name().to_string_lossy().eq_ignore_ascii_case(j)))
    };
    for e in walkdir::WalkDir::new(&inbox).follow_links(false).into_iter().filter_entry(not_junk_dir).flatten() {
        crate::progress::pulse();
        if !e.file_type().is_file() {
            continue;
        }
        let ext = ext_of(e.path());
        if is_book(e.path()) || ARCHIVE_EXT.contains(&ext.as_str()) {
            keepers.insert(e.path().parent().unwrap_or(&inbox).to_path_buf());
        }
    }
    for d in &keepers {
        let mut cur = Some(d.as_path());
        while let Some(c) = cur {
            if !keep_all.insert(c.to_path_buf()) {
                break;
            }
            if c == inbox {
                break;
            }
            cur = c.parent();
        }
    }
    let junk_set: HashSet<&PathBuf> = junk_dirs.iter().collect();
    // Gom danh sách cần dọn trước để báo "x / y"; mỗi lần vào Thùng rác trên ổ chậm có thể mất cả giây.
    let mut targets: Vec<(PathBuf, bool)> = Vec::new(); // (đường dẫn, là thư mục)
    let mut walker = walkdir::WalkDir::new(&inbox).min_depth(1).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        crate::progress::pulse();
        let Ok(entry) = entry else { continue };
        let p = entry.path().to_path_buf();
        if entry.file_type().is_dir() {
            if junk_set.contains(&p) || !keep_all.contains(&p) {
                walker.skip_current_dir();
                targets.push((p, true));
            }
        } else if entry.file_type().is_file() {
            let ext = ext_of(&p);
            if is_book(&p) || ARCHIVE_EXT.contains(&ext.as_str()) {
                continue;
            }
            targets.push((p, false));
        }
    }
    let n_targets = targets.len();
    let mut last_report: Option<std::time::Instant> = None;
    for (i, (p, is_dir)) in targets.into_iter().enumerate() {
        if cancelled() {
            rep.cancelled = true; // phần còn lại được dọn ở lần sau
            break;
        }
        if last_report.is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(250)) {
            if let Some(f) = opts.progress {
                f(Step { phase: "triage_clean", done: i, total: n_targets });
            }
            last_report = Some(std::time::Instant::now());
        }
        crate::progress::working_on(&p);
        match trasher.trash(&p) {
            Ok(()) if is_dir => rep.dirs_trashed += 1,
            Ok(()) => rep.junk_files_trashed += 1,
            Err(e) => rep.failures.push(format!("{}: {e}", p.display())),
        }
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake(RefCell<Vec<PathBuf>>);
    impl Trasher for Fake {
        fn trash(&self, p: &Path) -> std::result::Result<(), String> {
            self.0.borrow_mut().push(p.to_path_buf());
            if p.is_dir() {
                std::fs::remove_dir_all(p).map_err(|e| e.to_string())
            } else {
                std::fs::remove_file(p).map_err(|e| e.to_string())
            }
        }
    }

    fn write(p: &Path, body: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn opf(subj: &str, title: &str) -> String {
        format!("<package><metadata><dc:title>{title}</dc:title><dc:subject>{subj}</dc:subject></metadata></package>")
    }

    #[test]
    fn decide_is_conservative() {
        assert_eq!(decide("Trinh thám", "Vụ án", "").map(|r| r.name), Some("Trinh thám - Hình sự"));
        assert!(decide("", "Mười Hai Chiếc Ghế", "").is_none());
        assert!(decide("", "Tuổi thơ dữ dội", "").is_none(), "tuổi thơ không phải thơ");
        assert_eq!(decide("fiction", "X", "").map(|r| r.name), Some("Tiểu thuyết"));
    }

    #[test]
    fn flattens_calibre_classifies_and_cleans() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let cfg = Config { library_root: Some(root.to_path_buf()), ..Config::default() };
        std::fs::create_dir_all(root.join("Thơ")).unwrap();
        let inbox = root.join("Chưa phân loại");
        // Calibre: Tác giả/Sách/{epub,pdf,opf,jpg}
        let a = inbox.join("Calibre Library/Nguyễn A/Tập thơ xuân (1)");
        write(&a.join("Tập thơ xuân - Nguyễn A.epub"), b"epub1");
        write(&a.join("Tập thơ xuân - Nguyễn A.pdf"), b"pdf1");
        write(&a.join("cover.jpg"), b"img");
        write(&a.join("metadata.opf"), opf("Thơ", "Tập thơ xuân").as_bytes());
        let b = inbox.join("Calibre Library/Trần B/Vụ án bí ẩn (2)");
        write(&b.join("Vụ án bí ẩn - Trần B.mobi"), b"mobi");
        write(&b.join("metadata.opf"), opf("Trinh thám; scan", "Vụ án bí ẩn").as_bytes());
        let c = inbox.join("Calibre Library/Lê C/Sách không rõ (3)");
        write(&c.join("Sách không rõ - Lê C.epub"), b"unk");
        write(&c.join("metadata.opf"), opf("", "Sách không rõ").as_bytes());
        write(&inbox.join("Calibre Library/metadata.db"), b"db");
        write(&inbox.join("Calibre Library/.caltrash/x/old.epub"), b"old");
        write(&inbox.join("loose.opf"), b"x");
        write(&inbox.join("nhac.mp3"), b"mp3");
        write(&inbox.join("Calibre Library/Lê C/Sách không rõ (3)/phim.mp4"), b"mp4");

        let fake = Fake(RefCell::new(vec![]));
        let journal = root.join("j.jsonl");
        let rep =
            run(&cfg, &fake, &TriageOptions { dry_run: false, journal: Some(&journal), ..Default::default() }).unwrap();
        assert!(rep.failures.is_empty(), "{:?}", rep.failures);
        assert_eq!(rep.units, 3);
        assert!(root.join("Thơ/Tập thơ xuân - Nguyễn A.epub").is_file(), "dùng thư mục có sẵn");
        assert!(root.join("Thơ/Tập thơ xuân - Nguyễn A.pdf").is_file(), "mọi định dạng cùng chuyển");
        assert!(root.join("Trinh thám - Hình sự/Vụ án bí ẩn - Trần B.mobi").is_file(), "tạo thư mục mới");
        assert!(
            root.join("Khác/S/S-01/Sách không rõ - Lê C.epub").is_file(),
            "chưa chắc → Khác, ngăn theo chữ cái đầu"
        );
        assert_eq!(rep.to_other, 1);
        assert!(!inbox.join("Calibre Library").exists(), "thư mục con đã vào Thùng rác");
        assert!(!inbox.join("loose.opf").exists());
        assert!(!inbox.join("nhac.mp3").exists(), "âm thanh bị dọn");
        assert!(inbox.is_dir(), "hộp thư đến vẫn còn");
        assert_eq!(std::fs::read_to_string(journal).unwrap().lines().count(), 4);
    }

    #[test]
    fn keeps_comics_archives_and_respects_flags() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let mut cfg = Config { library_root: Some(root.to_path_buf()), ..Config::default() };
        let inbox = root.join("Chưa phân loại");
        write(&inbox.join("Bộ/Dragon Ball 1.cbz"), b"c");
        write(&inbox.join("Bộ/phim.mp4"), b"v");
        write(&inbox.join("zips/Cổ tích.zip"), b"z");
        write(&inbox.join("Sách/Tập thơ.epub"), b"e");
        let fake = Fake(RefCell::new(vec![]));
        // dry-run không đụng gì
        let d = run(&cfg, &fake, &TriageOptions { dry_run: true, ..Default::default() }).unwrap();
        assert_eq!(d.units, 1);
        assert!(inbox.join("Sách/Tập thơ.epub").is_file());
        run(&cfg, &fake, &TriageOptions { dry_run: false, ..Default::default() }).unwrap();
        assert!(inbox.join("Bộ/Dragon Ball 1.cbz").is_file(), "truyện để nguyên");
        assert!(!inbox.join("Bộ/phim.mp4").exists(), "video trong thư mục còn sách vẫn bị dọn");
        assert!(inbox.join("zips/Cổ tích.zip").is_file(), "file nén để nguyên");
        assert!(!inbox.join("Sách").exists());
        assert!(root.join("Thơ/Tập thơ.epub").is_file());
        cfg.read_only = true;
        assert!(run(&cfg, &fake, &TriageOptions { dry_run: false, ..Default::default() }).is_err());
    }

    #[test]
    fn identical_duplicate_goes_to_trash_and_different_gets_suffix() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let cfg = Config { library_root: Some(root.to_path_buf()), ..Config::default() };
        write(&root.join("Thơ/Tập thơ.epub"), b"same");
        let inbox = root.join("Chưa phân loại");
        write(&inbox.join("a/Tập thơ.epub"), b"same");
        write(&inbox.join("b/Tập thơ.epub"), b"diff!");
        let fake = Fake(RefCell::new(vec![]));
        let rep = run(&cfg, &fake, &TriageOptions { dry_run: false, ..Default::default() }).unwrap();
        assert_eq!(rep.duplicates_trashed, 1);
        assert!(root.join("Thơ/Tập thơ (1).epub").is_file());
    }

    #[test]
    fn sidecar_txt_is_not_a_book_and_keeps_opf_tags() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let cfg = Config { library_root: Some(root.to_path_buf()), ..Config::default() };
        std::fs::create_dir_all(root.join("Thơ")).unwrap();
        let a = root.join("Chưa phân loại/Tác giả/Tập thơ xuân");
        write(&a.join("Tập thơ xuân.epub"), b"e");
        write(&a.join("description.txt"), b"mo ta");
        write(&a.join(".progress.txt"), b"1");
        write(&a.join("metadata.kfx"), b"k");
        write(&a.join("metadata.opf"), opf("Thơ", "Tập thơ xuân").as_bytes());
        let fake = Fake(RefCell::new(vec![]));
        let rep = run(&cfg, &fake, &TriageOptions::default()).unwrap();
        assert_eq!(rep.units, 1, "file đi kèm không thành tác phẩm riêng");
        assert!(root.join("Thơ/Tập thơ xuân.epub").is_file(), "vẫn dùng tag của metadata.opf");
        assert!(!root.join("Khác").exists());
        assert!(!root.join("Chưa phân loại/Tác giả").exists(), "thư mục chỉ còn file đi kèm thì được dọn");
    }

    #[test]
    fn many_same_names_get_distinct_suffixes() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let cfg = Config { library_root: Some(root.to_path_buf()), ..Config::default() };
        write(&root.join("Thơ/Tập thơ.epub"), b"0");
        write(&root.join("Thơ/Tập thơ (2).epub"), b"00");
        for i in 0..5 {
            write(&root.join(format!("Chưa phân loại/{i}/Tập thơ.epub")), format!("khac {i}").as_bytes());
        }
        let fake = Fake(RefCell::new(vec![]));
        let rep = run(&cfg, &fake, &TriageOptions::default()).unwrap();
        assert!(rep.failures.is_empty(), "{:?}", rep.failures);
        assert_eq!(rep.moved_files, 5);
        let n = std::fs::read_dir(root.join("Thơ")).unwrap().count();
        assert_eq!(n, 7, "không file nào bị ghi đè");
        for k in [1, 3, 4, 5, 6] {
            assert!(root.join(format!("Thơ/Tập thơ ({k}).epub")).is_file(), "thiếu ({k})");
        }
    }

    #[test]
    fn goes_into_matching_user_subfolders() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let cfg = Config { library_root: Some(root.to_path_buf()), ..Config::default() };
        let parent = root.join("5. Ngôn Tình - Đam Mỹ");
        std::fs::create_dir_all(parent.join("Đam Mỹ")).unwrap();
        std::fs::create_dir_all(parent.join("Ngôn Tình/Hiện đại")).unwrap();
        std::fs::create_dir_all(parent.join("Mới")).unwrap(); // tên một từ ngắn: không phải từ khoá
        let inbox = root.join("Chưa phân loại");
        for (name, tag) in [
            ("A", "Đam mỹ"),
            ("B", "Romance; Đam mỹ"),
            ("C", "Ngôn tình"),
            ("D", "Ngôn tình; Hiện đại"),
            ("E", "Ngôn tình; Đam mỹ"),
            ("F", "Danmei; Lãng mạn"),
        ] {
            write(&inbox.join(name).join(format!("{name}.epub")), name.as_bytes());
            write(&inbox.join(name).join("metadata.opf"), opf(tag, &format!("Truyện mới {name}")).as_bytes());
        }
        let fake = Fake(RefCell::new(vec![]));
        let rep = run(&cfg, &fake, &TriageOptions::default()).unwrap();
        assert!(rep.failures.is_empty(), "{:?}", rep.failures);
        assert!(rep.created_dirs.is_empty(), "dùng thư mục có số thứ tự sẵn có: {:?}", rep.created_dirs);
        assert!(parent.join("Đam Mỹ/A.epub").is_file(), "tag khớp tên thư mục con");
        assert!(parent.join("Đam Mỹ/B.epub").is_file(), "romance chỉ là từ yếu của Ngôn tình");
        assert!(parent.join("Ngôn Tình/C.epub").is_file());
        assert!(parent.join("Ngôn Tình/Hiện đại/D.epub").is_file(), "đi xuống nhiều cấp");
        assert!(parent.join("E.epub").is_file(), "hoà điểm, không mục nào bị chặn → để ở thư mục thể loại");
        assert!(parent.join("Đam Mỹ/F.epub").is_file(), "từ đồng nghĩa");
        assert!(std::fs::read_dir(parent.join("Mới")).unwrap().next().is_none());
    }

    #[test]
    fn blocked_genre_goes_to_trash_and_is_never_recreated() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let mut cfg = Config { library_root: Some(root.to_path_buf()), ..Config::default() };
        // Thư mục con bị chặn đã được dọn khỏi đĩa, chỉ còn "Ngôn tình".
        std::fs::create_dir_all(root.join("Ngôn tình - Đam mỹ/Ngôn tình")).unwrap();
        cfg.blocked_categories = vec!["Ngôn tình - Đam mỹ/Đam mỹ".into(), "8. Trinh Thám".into()];
        let inbox = root.join("Chưa phân loại");
        for (name, tag) in
            [("A", "Đam mỹ"), ("B", "Ngôn tình; Đam mỹ"), ("C", "Ngôn tình"), ("D", "Trinh thám"), ("E", "Thơ")]
        {
            write(&inbox.join(name).join(format!("{name}.epub")), name.as_bytes());
            write(&inbox.join(name).join(format!("{name}.pdf")), name.as_bytes());
            write(&inbox.join(name).join("metadata.opf"), opf(tag, name).as_bytes());
        }
        let fake = Fake(RefCell::new(vec![]));
        let dry = run(&cfg, &fake, &TriageOptions { dry_run: true, ..Default::default() }).unwrap();
        assert_eq!(dry.blocked_trashed, 6, "{dry:?}");
        assert!(fake.0.borrow().is_empty(), "dry-run không xoá");
        let journal = root.join("j.jsonl");
        let rep = run(&cfg, &fake, &TriageOptions { journal: Some(&journal), ..Default::default() }).unwrap();
        assert!(rep.failures.is_empty(), "{:?}", rep.failures);
        assert_eq!(rep.blocked_trashed, 6);
        assert_eq!(rep.blocked.get("Ngôn tình - Đam mỹ/Đam mỹ"), Some(&2), "{:?}", rep.blocked);
        assert_eq!(rep.blocked.get("8. Trinh Thám"), Some(&1), "luật Trinh thám nhận ra thư mục bị chặn có số thứ tự");
        assert!(!root.join("Ngôn tình - Đam mỹ/Đam mỹ").exists(), "không tạo lại thư mục con bị chặn");
        assert!(!root.join("8. Trinh Thám").exists() && !root.join("Trinh thám - Hình sự").exists());
        assert!(root.join("Ngôn tình - Đam mỹ/Ngôn tình/C.epub").is_file());
        assert!(root.join("Thơ/E.pdf").is_file(), "thể loại khác vẫn tạo mới như thường");
        assert!(!rep.by_category.contains_key("8. Trinh Thám"));
        let blocked_lines =
            std::fs::read_to_string(&journal).unwrap().lines().filter(|l| l.contains("\"blocked\"")).count();
        assert_eq!(blocked_lines, 6);
    }

    #[test]
    fn other_bucket_names() {
        assert_eq!(other_bucket("Thiên long bát bộ.epub"), "T");
        assert_eq!(other_bucket("Đất rừng phương Nam.pdf"), "D");
        assert_eq!(other_bucket("ÔNG GIÀ VÀ BIỂN CẢ.mobi"), "O");
        assert_eq!(other_bucket("« Người lạ ».epub"), "N", "bỏ ký tự đầu không phải chữ");
        assert_eq!(other_bucket("A.epub"), "A");
        assert_eq!(other_bucket("an.epub"), "A");
        assert_eq!(other_bucket("2 vạn dặm dưới đáy biển.epub"), "0-9");
        assert_eq!(other_bucket("1984.epub"), "0-9");
        assert_eq!(other_bucket("三国演义.epub"), "#");
        assert_eq!(other_bucket("!!!.epub"), "#");
        for b in ["T", "A", "0-9", "#"] {
            assert!(is_other_bucket(b), "{b}");
        }
        for n in ["Conan", "Th", "Thế", "abc", "7 seeds", "1"] {
            assert!(!is_other_bucket(n), "{n}");
        }
        assert!(is_other_dir("khac") && is_other_dir("Khác") && !is_other_dir("Thơ"));
    }

    #[test]
    fn shelf_names() {
        assert_eq!(other_shelf_name("T", 1), "T-01");
        assert_eq!(other_shelf_name("0-9", 12), "0-9-12");
        assert_eq!(other_shelf_number("T", "T-03"), Some(3));
        assert_eq!(other_shelf_number("T", "t-120"), Some(120));
        assert_eq!(other_shelf_number("0-9", "0-9-01"), Some(1));
        assert_eq!(other_shelf_number("#", "#-02"), Some(2));
        for bad in ["T", "T-", "T01", "T-0a", "N-01", "Truyện"] {
            assert_eq!(other_shelf_number("T", bad), None, "{bad}");
        }
    }

    #[test]
    fn bucket_other_splits_flat_khac() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let cfg = Config { library_root: Some(root.to_path_buf()), ..Config::default() };
        write(&root.join("Khác/Thiên long.epub"), b"1");
        write(&root.join("Khác/Thiên long.pdf"), b"2");
        write(&root.join("Khác/1984.mobi"), b"3");
        write(&root.join("Khác/T/T-01/Thiên long.epub"), b"cu");
        write(&root.join("Khác/T/Tự thả.epub"), b"4");
        write(&root.join("Khác/ghi chu.docx.tmp"), b"x");
        write(&root.join("Khác/Conan/Conan 1.epub"), b"c");
        let journal = root.join("j.jsonl");
        let dry = bucket_other(&cfg, &TriageOptions { dry_run: true, ..Default::default() }).unwrap();
        assert_eq!(dry.moved_files, 4);
        assert!(root.join("Khác/1984.mobi").is_file(), "dry-run không chuyển");
        assert!(!root.join("Khác/0-9").exists(), "dry-run không tạo thư mục");
        let rep = bucket_other(&cfg, &TriageOptions { journal: Some(&journal), ..Default::default() }).unwrap();
        assert!(rep.failures.is_empty(), "{:?}", rep.failures);
        assert_eq!(rep.moved_files, 4);
        assert!(root.join("Khác/T/T-01/Thiên long (1).epub").is_file(), "không ghi đè");
        assert_eq!(std::fs::read(root.join("Khác/T/T-01/Thiên long.epub")).unwrap(), b"cu");
        assert!(root.join("Khác/T/T-01/Thiên long.pdf").is_file());
        assert!(root.join("Khác/T/T-01/Tự thả.epub").is_file(), "file lẻ trong thư mục chữ cái cũng vào ngăn");
        assert!(root.join("Khác/0-9/0-9-01/1984.mobi").is_file());
        assert!(root.join("Khác/ghi chu.docx.tmp").is_file(), "file không phải sách để nguyên");
        assert!(root.join("Khác/Conan/Conan 1.epub").is_file(), "thư mục con có sẵn để nguyên");
        assert_eq!(std::fs::read_to_string(&journal).unwrap().lines().count(), 4);
        let again = bucket_other(&cfg, &TriageOptions::default()).unwrap();
        assert_eq!(again.moved_files, 0, "lần sau không còn gì để chuyển");
    }

    #[test]
    fn shelves_hold_at_most_500_and_never_split_a_work() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let cfg = Config { library_root: Some(root.to_path_buf()), ..Config::default() };
        for i in 0..(OTHER_SHELF_MAX * 2 + 1) {
            write(&root.join(format!("Khác/Truyện {i:04}.epub")), b"x");
        }
        let rep = bucket_other(&cfg, &TriageOptions::default()).unwrap();
        assert_eq!(rep.moved_files, OTHER_SHELF_MAX * 2 + 1);
        let count = |d: &str| std::fs::read_dir(root.join(d)).map(|r| r.count()).unwrap_or(0);
        assert_eq!(count("Khác/T/T-01"), OTHER_SHELF_MAX);
        assert_eq!(count("Khác/T/T-02"), OTHER_SHELF_MAX);
        assert_eq!(count("Khác/T/T-03"), 1);
        assert_eq!(count("Khác/T"), 3, "chỉ có các ngăn");

        // Ngăn cuối còn 1 chỗ, tác phẩm 2 định dạng → cả hai vào ngăn mới.
        for i in 0..(OTHER_SHELF_MAX - 2) {
            write(&root.join(format!("Khác/T/T-03/Thêm {i:04}.epub")), b"y");
        }
        assert_eq!(count("Khác/T/T-03"), OTHER_SHELF_MAX - 1);
        write(&root.join("Chưa phân loại/Tên lạ - Ai đó.epub"), b"e");
        write(&root.join("Chưa phân loại/Tên lạ - Ai đó.pdf"), b"p");
        let fake = Fake(RefCell::new(vec![]));
        let rep = run(&cfg, &fake, &TriageOptions::default()).unwrap();
        assert!(rep.failures.is_empty(), "{:?}", rep.failures);
        assert_eq!(rep.to_other, 1);
        assert!(root.join("Khác/T/T-04/Tên lạ - Ai đó.epub").is_file());
        assert!(root.join("Khác/T/T-04/Tên lạ - Ai đó.pdf").is_file());
        assert_eq!(count("Khác/T/T-03"), OTHER_SHELF_MAX - 1);

        // Bản y hệt nằm ở ngăn cũ vẫn được nhận ra.
        write(&root.join("Chưa phân loại/x/Truyện 0003.epub"), b"x");
        let rep = run(&cfg, &fake, &TriageOptions::default()).unwrap();
        assert_eq!(rep.duplicates_trashed, 1, "{rep:?}");
    }
}
