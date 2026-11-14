//! Trùng chắc chắn (E4-02, E4-03) và nghi trùng (E4-04..E4-06).
//! Quy tắc: SHA-256 giống nhau được dọn hàng loạt; nghi trùng có độ tin cậy ≥ [`AUTO_SUSPECT_CONFIDENCE`]% được
//! tự dọn (giữ bản tốt nhất), phần còn lại qua người duyệt.

use crate::config::Config;
use crate::db::{self, now, Book};
use crate::error::Result;
use crate::hash;
use crate::normalize::{authors_conflict, has_copy_suffix, normalize_title, series_from_name};
use crate::trash::{trash_items_with, TrashItem, TrashReport, Trasher};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Serialize)]
pub struct ExactGroup {
    pub full_hash: String,
    pub size: i64,
    pub keeper: Book,
    pub keep_reason: String,
    pub others: Vec<Book>,
    pub reclaimable: i64,
}

/// Khoá sắp xếp chọn bản giữ (BRD 3.4):
/// trong thư mục thể loại của thư viện → tên không có hậu tố (1)/copy → đường dẫn nông hơn → file cũ hơn.
fn keeper_key(b: &Book, library_root_id: Option<i64>) -> (bool, bool, usize, i64, i64) {
    let in_library_category = Some(b.root_id) == library_root_id && b.category.is_some() && !b.in_inbox;
    (!in_library_category, has_copy_suffix(b.stem()), b.depth(), b.mtime, b.id)
}

fn keep_reason(k: &Book, runner_up: &Book, lib: Option<i64>) -> String {
    let a = keeper_key(k, lib);
    let b = keeper_key(runner_up, lib);
    if a.0 != b.0 {
        "đã nằm trong thư mục thể loại của thư viện".into()
    } else if a.1 != b.1 {
        "tên không có hậu tố (1)/copy".into()
    } else if a.2 != b.2 {
        "đường dẫn nông hơn".into()
    } else if a.3 != b.3 {
        "file cũ hơn".into()
    } else {
        "thêm vào thư viện sớm hơn".into()
    }
}

pub fn library_root_id(conn: &Connection) -> Result<Option<i64>> {
    use rusqlite::OptionalExtension;
    Ok(conn.query_row("SELECT id FROM roots WHERE kind = 'library' LIMIT 1", [], |r| r.get(0)).optional()?)
}

/// Chọn bản giữ; trả về (chỉ số, lý do).
pub fn choose_keeper(members: &[Book], lib: Option<i64>) -> (usize, String) {
    let mut idx: Vec<usize> = (0..members.len()).collect();
    idx.sort_by_key(|&i| keeper_key(&members[i], lib));
    let k = idx[0];
    let reason = if members.len() > 1 { keep_reason(&members[k], &members[idx[1]], lib) } else { String::new() };
    (k, reason)
}

/// Nhóm trùng chắc chắn hiện có (dựa trên băm đã lưu – gọi `hash::ensure_hashes` trước).
pub fn exact_groups(conn: &Connection) -> Result<Vec<ExactGroup>> {
    let lib = library_root_id(conn)?;
    let books = db::books_where(
        conn,
        "status = 'present' AND full_hash IS NOT NULL AND hash_size = size AND hash_mtime = mtime
         AND full_hash IN (SELECT full_hash FROM books WHERE status = 'present' AND full_hash IS NOT NULL
                           GROUP BY full_hash HAVING COUNT(*) > 1)
         ORDER BY full_hash, id",
        [],
    )?;
    let mut groups: BTreeMap<String, Vec<Book>> = BTreeMap::new();
    for b in books {
        groups.entry(b.full_hash.clone().unwrap()).or_default().push(b);
    }
    let mut out = Vec::new();
    for (h, mut members) in groups {
        if members.len() < 2 {
            continue;
        }
        let (k, reason) = choose_keeper(&members, lib);
        let keeper = members.remove(k);
        let size = keeper.size;
        out.push(ExactGroup {
            full_hash: h,
            size,
            reclaimable: size * members.len() as i64,
            keeper,
            keep_reason: reason,
            others: members,
        });
    }
    out.sort_by_key(|g| std::cmp::Reverse(g.reclaimable));
    Ok(out)
}

/// Dọn các nhóm trùng chắc chắn vào Thùng rác (nút "Dọn tất cả" / dọn từng nhóm).
/// `keeper_override`: full_hash → book_id người dùng chọn giữ thay cho đề xuất.
pub fn clean_exact(
    conn: &Connection,
    cfg: &Config,
    trasher: &dyn Trasher,
    groups: &[ExactGroup],
    keeper_override: &HashMap<String, i64>,
) -> Result<TrashReport> {
    clean_exact_with(conn, cfg, trasher, groups, keeper_override, None, None)
}

/// Như [`clean_exact`], báo tiến trình (bước `trash`, x / y file) và dừng được giữa chừng.
pub fn clean_exact_with(
    conn: &Connection,
    cfg: &Config,
    trasher: &dyn Trasher,
    groups: &[ExactGroup],
    keeper_override: &HashMap<String, i64>,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on: Option<crate::OnStep>,
) -> Result<TrashReport> {
    let mut regrouped: Vec<(Book, Vec<Book>)> = Vec::new();
    for g in groups {
        let mut all: Vec<Book> = std::iter::once(g.keeper.clone()).chain(g.others.iter().cloned()).collect();
        let k = keeper_override.get(&g.full_hash).and_then(|id| all.iter().position(|b| b.id == *id)).unwrap_or(0);
        let keeper = all.remove(k);
        regrouped.push((keeper, all));
    }
    let mut items = Vec::new();
    for (keeper, others) in &regrouped {
        for o in others {
            if o.full_hash != keeper.full_hash || o.id == keeper.id {
                continue; // phòng thủ: chỉ xoá khi băm đầy đủ trùng khớp
            }
            items.push(TrashItem {
                book: o,
                expect: Some((o.hash_size.unwrap_or(o.size), o.hash_mtime.unwrap_or(o.mtime))),
                keeper: Some(keeper),
                note: Some(format!("trùng chắc chắn với {}", keeper.path)),
            });
        }
    }
    trash_items_with(conn, cfg, trasher, "dedupe", &items, cancel, on)
}

// ---------------------------------------------------------------- nghi trùng

#[derive(Debug, Clone, Serialize)]
pub struct SuspectGroup {
    /// Khoá ổn định của nhóm (để UI ghi nhớ).
    pub key: String,
    pub reason: String,
    pub members: Vec<Book>,
    /// Độ tin cậy 0..100 rằng cả nhóm là cùng một cuốn (cặp yếu nhất trong nhóm), xem [`pair_confidence`].
    pub confidence: u8,
    /// Bản đề xuất giữ lại (xem [`choose_suspect_keeper`]) – luôn là sách đang có, không bao giờ là mục "đã xoá".
    pub keeper_id: i64,
    /// Nhóm có mục "đã xoá trước đây" (id âm, `status = "deleted"`, xem deleted.rs): sách mới tải về gần giống một
    /// cuốn người dùng đã xoá. Không tự dọn – luôn để người dùng duyệt.
    pub has_deleted: bool,
}

/// Nhóm nghi trùng có độ tin cậy từ ngưỡng này trở lên được tự dọn, không cần người duyệt.
pub const AUTO_SUSPECT_CONFIDENCE: u8 = 85;

/// Độ tin cậy (0..100) hai file cùng định dạng là cùng một cuốn, dựa trên ISBN, tên chuẩn hoá, tác giả,
/// bộ/tập, dung lượng và số trang. Chỉ chạm ngưỡng tự dọn khi có bằng chứng mạnh: cùng ISBN, hoặc cùng tên + cùng
/// tác giả – và dung lượng / số trang gần nhau.
pub fn pair_confidence(a: &Book, b: &Book) -> u8 {
    let same_isbn = matches!((&a.isbn, &b.isbn), (Some(x), Some(y)) if x == y);
    let isbn_conflict = matches!((&a.isbn, &b.isbn), (Some(x), Some(y)) if x != y);
    let same_title = match (title_key(a), title_key(b)) {
        (Some((ka, _)), Some((kb, _))) => ka == kb,
        _ => false,
    };
    let authors_known = !a.norm_author.trim().is_empty() && !b.norm_author.trim().is_empty();
    if authors_known && authors_conflict(&a.norm_author, &b.norm_author) {
        return 0;
    }
    let mut score: i32 = match (same_isbn, same_title, authors_known) {
        (true, true, _) => 92,
        (true, false, _) => 85,
        (false, true, true) => 75,
        (false, true, false) => 55,
        _ => 30,
    };
    if isbn_conflict {
        score -= 30; // khác ISBN: thường là ấn bản / bản dịch khác
    }
    let (sa, sb) = (a.size.max(1) as f64, b.size.max(1) as f64);
    let ratio = sa.max(sb) / sa.min(sb);
    score += if ratio <= 1.05 {
        15
    } else if ratio <= 1.25 {
        8
    } else if ratio <= 2.0 {
        0
    } else if ratio <= 4.0 {
        -15
    } else {
        -30
    };
    if let (Some(pa), Some(pb)) = (a.page_count.filter(|&p| p > 0), b.page_count.filter(|&p| p > 0)) {
        let d = (pa - pb).abs() as f64 / pa.max(pb) as f64;
        score += if d <= 0.02 {
            10
        } else if d <= 0.10 {
            4
        } else if d <= 0.25 {
            -5
        } else {
            -20
        };
    }
    if let (Some(va), Some(vb)) = (a.volume, b.volume) {
        if va != vb {
            score = score.min(50); // khác tập: không bao giờ tự dọn
        }
    }
    score.clamp(0, 100) as u8
}

/// Độ tin cậy của cả nhóm = cặp yếu nhất (nhóm gộp bắc cầu A~B~C chỉ chắc khi mọi cặp đều chắc).
pub fn group_confidence(members: &[Book]) -> u8 {
    let mut min = 100u8;
    for i in 0..members.len() {
        for j in i + 1..members.len() {
            min = min.min(pair_confidence(&members[i], &members[j]));
        }
    }
    min
}

/// Chọn bản giữ trong nhóm nghi trùng (nội dung khác nhau nên ưu tiên bản "đầy đủ" hơn):
/// trong thư mục thể loại của thư viện → nhiều trang hơn → dung lượng lớn hơn → tên không có hậu tố (1)/copy →
/// đường dẫn nông hơn → file cũ hơn.
pub fn choose_suspect_keeper(members: &[Book], library_root_id: Option<i64>) -> usize {
    (0..members.len())
        .min_by_key(|&i| {
            let b = &members[i];
            let k = keeper_key(b, library_root_id);
            (k.0, std::cmp::Reverse(b.page_count.unwrap_or(0)), std::cmp::Reverse(b.size), k.1, k.2, k.3, k.4)
        })
        .unwrap_or(0)
}

/// Tự dọn các nhóm nghi trùng có độ tin cậy ≥ `min_confidence`: giữ [`SuspectGroup::keeper_id`], đưa các bản còn
/// lại vào Thùng rác (mỗi file được kiểm tra chưa đổi kể từ lúc quét, bản giữ được kiểm tra còn nguyên).
/// Báo tiến trình bước `trash` x / y và dừng được giữa chừng như dọn trùng chắc chắn.
pub fn clean_suspects_with(
    conn: &Connection,
    cfg: &Config,
    trasher: &dyn Trasher,
    groups: &[SuspectGroup],
    min_confidence: u8,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on: Option<crate::OnStep>,
) -> Result<TrashReport> {
    // Bản giữ được so với dung lượng / thời điểm sửa lúc quét (băm của nó có thể cũ hơn – không dùng để kiểm).
    let keepers: Vec<(usize, Book)> = groups
        .iter()
        .enumerate()
        .filter(|(_, g)| g.confidence >= min_confidence && !g.has_deleted)
        .filter_map(|(gi, g)| {
            let k = g.members.iter().find(|b| b.id == g.keeper_id)?;
            Some((gi, Book { hash_size: Some(k.size), hash_mtime: Some(k.mtime), ..k.clone() }))
        })
        .collect();
    let mut items = Vec::new();
    for (gi, keeper) in &keepers {
        let g = &groups[*gi];
        for o in g.members.iter().filter(|b| b.id != keeper.id) {
            items.push(TrashItem {
                book: o,
                expect: Some((o.size, o.mtime)),
                keeper: Some(keeper),
                note: Some(format!("nghi trùng {}% với {}", g.confidence, keeper.path)),
            });
        }
    }
    trash_items_with(conn, cfg, trasher, "dedupe_suspect", &items, cancel, on)
}

fn is_comic(b: &Book) -> bool {
    b.format == "cbz" || b.format == "cbr" || b.series.is_some()
}

/// Khoá so khớp tên. Truyện có số tập: (bộ, tập) – không bao giờ gộp tập khác nhau (E4-05).
fn title_key(b: &Book) -> Option<(String, String)> {
    if is_comic(b) {
        if let Some(v) = b.volume {
            let series = match &b.series {
                Some(s) => normalize_title(s),
                None => series_from_name(b.stem()),
            };
            if series.is_empty() {
                return None;
            }
            return Some((format!("c:{series}#{v}"), format!("Cùng bộ “{series}”, tập {v}")));
        }
    }
    if b.norm_title.chars().count() < 3 {
        return None;
    }
    Some((format!("t:{}", b.norm_title), format!("Cùng tên chuẩn hoá “{}”", b.norm_title)))
}

/// Dấu vân tay nội dung để ghi nhớ quyết định duyệt: luôn dùng băm nhanh (dung lượng + 3 đoạn),
/// vì băm đầy đủ chỉ có khi file có "anh em" cùng dung lượng và có thể xuất hiện/mất đi theo thời gian.
fn fingerprint(b: &Book) -> Option<String> {
    b.quick_hash.as_ref().map(|q| format!("q:{}:{q}", b.size))
}

/// Dấu vân tay của mục "đã xoá" (id âm): băm nhanh lúc xoá, hoặc id trong sổ nếu lúc đó không băm được.
fn ghost_fingerprint(b: &Book) -> String {
    fingerprint(b).unwrap_or_else(|| format!("d:{}", -b.id))
}

/// Dấu vân tay hiện tại của một cuốn (chỉ khi băm nhanh còn khớp file) – để chuyển quyết định khi file đổi nội dung.
pub fn fingerprint_of(b: &Book) -> Option<String> {
    fingerprint(b).filter(|_| b.hash_size == Some(b.size) && b.hash_mtime == Some(b.mtime))
}

/// File đổi nội dung do chính app sửa (vd. ghi tiêu đề): chuyển mọi quyết định duyệt nghi trùng từ dấu vân tay cũ
/// sang dấu vân tay mới để nhóm đã duyệt không hiện lại.
pub fn replace_fingerprint(conn: &Connection, old: &str, new: &str) -> Result<usize> {
    if old == new {
        return Ok(0);
    }
    let rows: Vec<(String, String, String, i64)> = {
        let mut st =
            conn.prepare("SELECT fp_a, fp_b, decision, decided_at FROM dup_decisions WHERE fp_a = ?1 OR fp_b = ?1")?;
        let v = st
            .query_map([old], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        v
    };
    for (a, b, d, at) in &rows {
        conn.execute("DELETE FROM dup_decisions WHERE fp_a = ?1 AND fp_b = ?2", params![a, b])?;
        let other = if a == old { b } else { a };
        let (x, y) = pair_key(new, other);
        conn.execute(
            "INSERT OR REPLACE INTO dup_decisions(fp_a, fp_b, decision, decided_at) VALUES (?1, ?2, ?3, ?4)",
            params![x, y, d, at],
        )?;
    }
    Ok(rows.len())
}

fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

struct Dsu(Vec<usize>);
impl Dsu {
    fn find(&mut self, x: usize) -> usize {
        if self.0[x] != x {
            let r = self.find(self.0[x]);
            self.0[x] = r;
        }
        self.0[x]
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

/// Phát hiện nghi trùng: cùng ISBN, hoặc cùng tên chuẩn hoá + tác giả không mâu thuẫn; nội dung khác nhau.
/// Nhóm/cặp đã được duyệt ("không trùng", "giữ cả hai") không hiện lại, kể cả sau khi quét lại.
pub fn suspect_groups(conn: &Connection) -> Result<Vec<SuspectGroup>> {
    Ok(suspect_groups_with(conn, 2, None, None)?.unwrap_or_default())
}

/// Bước con báo tiến trình khi băm bổ sung cho nghi trùng.
pub const SUSPECT_HASH_PHASE: &str = "suspect_hash";

/// Như [`suspect_groups`], băm bổ sung bằng `threads` luồng, báo tiến trình [`SUSPECT_HASH_PHASE`] x / y qua `on`
/// và dừng được bằng `cancel` (trả về `None`; phần đã băm vẫn được lưu nên lần sau chỉ băm phần còn lại).
///
/// Cùng một tác phẩm ở **nhiều định dạng** (vd. `.epub` + `.mobi`) không phải trùng: chỉ so các file cùng định dạng.
pub fn suspect_groups_with(
    conn: &Connection,
    threads: usize,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on: Option<crate::OnStep>,
) -> Result<Option<Vec<SuspectGroup>>> {
    let mut books = db::books_where(conn, "status = 'present' ORDER BY id", [])?;
    // Sách đã xoá trước đây (sổ deleted_books) tham gia so khớp như mục "ma" (id âm) – chỉ ghép với sách được thêm
    // vào SAU lúc xoá (sách đang có lúc xoá chính là bản người dùng đã chọn giữ, không hỏi lại).
    let added_at: HashMap<i64, i64> = {
        let mut st = conn.prepare("SELECT id, added_at FROM books WHERE status = 'present'")?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        v
    };
    let mut deleted_at: HashMap<usize, i64> = HashMap::new();
    for (g, at) in crate::deleted::ghosts(conn)? {
        deleted_at.insert(books.len(), at);
        books.push(g);
    }
    let mut buckets: HashMap<String, (String, Vec<usize>)> = HashMap::new();
    for (i, b) in books.iter().enumerate() {
        crate::progress::pulse();
        if let Some(isbn) = &b.isbn {
            buckets.entry(format!("i:{isbn}")).or_insert_with(|| (format!("Cùng ISBN {isbn}"), Vec::new())).1.push(i);
        }
        if let Some((k, reason)) = title_key(b) {
            buckets.entry(k).or_insert_with(|| (reason, Vec::new())).1.push(i);
        }
    }
    // Cặp ứng viên.
    let mut pairs: Vec<(usize, usize, String)> = Vec::new();
    let mut keys: Vec<&String> = buckets.keys().collect();
    keys.sort();
    for k in keys {
        let (reason, idx) = &buckets[k];
        if idx.len() < 2 || idx.len() > 200 {
            continue; // bucket quá lớn = tên quá chung chung ("tap 1") → bỏ để tránh báo nhầm
        }
        crate::progress::pulse();
        for x in 0..idx.len() {
            for y in x + 1..idx.len() {
                let (a, b) = (&books[idx[x]], &books[idx[y]]);
                match (deleted_at.get(&idx[x]), deleted_at.get(&idx[y])) {
                    (Some(_), Some(_)) => continue, // hai mục đã xoá với nhau: không có gì để duyệt
                    (Some(&at), None) if added_at.get(&b.id).is_none_or(|&t| t <= at) => continue,
                    (None, Some(&at)) if added_at.get(&a.id).is_none_or(|&t| t <= at) => continue,
                    _ => {}
                }
                if !a.format.eq_ignore_ascii_case(&b.format) {
                    continue; // một tác phẩm nhiều định dạng – không phải trùng
                }
                if a.full_hash.is_some() && a.full_hash == b.full_hash {
                    continue; // trùng chắc chắn – xử lý ở màn khác
                }
                if authors_conflict(&a.norm_author, &b.norm_author) {
                    continue;
                }
                if let (Some(va), Some(vb)) = (a.volume, b.volume) {
                    if va != vb && (is_comic(a) || is_comic(b)) {
                        continue;
                    }
                }
                pairs.push((idx[x], idx[y], reason.clone()));
            }
        }
    }
    if pairs.is_empty() {
        return Ok(Some(Vec::new()));
    }
    // Đảm bảo có dấu vân tay để ghi nhớ quyết định: băm bù (song song, lưu theo lô) những file còn thiếu.
    let involved: Vec<usize> =
        pairs.iter().flat_map(|(a, b, _)| [*a, *b]).collect::<HashSet<_>>().into_iter().collect();
    let missing: Vec<i64> = involved
        .iter()
        .filter(|i| !deleted_at.contains_key(i))
        .map(|&i| &books[i])
        .filter(|b| fingerprint(b).is_none() || b.hash_size != Some(b.size) || b.hash_mtime != Some(b.mtime))
        .map(|b| b.id)
        .collect();
    if !hash::ensure_quick_many(conn, &missing, threads, cancel, SUSPECT_HASH_PHASE, on)? {
        return Ok(None);
    }
    let mut fps: HashMap<usize, Option<String>> = HashMap::new();
    {
        let mut st =
            conn.prepare("SELECT quick_hash FROM books WHERE id = ?1 AND hash_size IS size AND hash_mtime IS mtime")?;
        let missing: HashSet<i64> = missing.into_iter().collect();
        for i in involved {
            let b = &books[i];
            let fp = if deleted_at.contains_key(&i) {
                Some(ghost_fingerprint(b))
            } else if missing.contains(&b.id) {
                st.query_row([b.id], |r| r.get::<_, Option<String>>(0))
                    .optional()?
                    .flatten()
                    .map(|q| format!("q:{}:{q}", b.size))
            } else {
                fingerprint(b)
            };
            fps.insert(i, fp);
        }
    }
    let decided = load_decisions(conn)?;
    // Nội dung → lần đầu có trong thư viện (sách đang có). Bản copy lại của cuốn đã có TRƯỚC lúc xoá một mục trong sổ
    // (vd. ấn bản người dùng đã chọn giữ) không ghép lại với mục đó: quyết định giữ / xoá đã có rồi.
    let first_added: HashMap<String, i64> = if deleted_at.is_empty() {
        HashMap::new()
    } else {
        let mut st = conn.prepare(
            "SELECT size, quick_hash, MIN(added_at) FROM books
             WHERE status = 'present' AND quick_hash IS NOT NULL AND hash_size IS size AND hash_mtime IS mtime
             GROUP BY size, quick_hash",
        )?;
        let v = st
            .query_map([], |r| Ok((format!("q:{}:{}", r.get::<_, i64>(0)?, r.get::<_, String>(1)?), r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        v
    };
    let mut dsu = Dsu((0..books.len()).collect());
    let mut reasons: HashMap<usize, Vec<String>> = HashMap::new();
    for (a, b, reason) in pairs {
        crate::progress::pulse();
        if let (Some(Some(fa)), Some(Some(fb))) = (fps.get(&a), fps.get(&b)) {
            if fa == fb {
                continue; // nội dung y hệt (băm nhanh trùng) – không phải "nghi trùng"
            }
            if decided.contains(&pair_key(fa, fb)) {
                continue;
            }
            let (ghost_at, real_fp) = match (deleted_at.get(&a), deleted_at.get(&b)) {
                (Some(&at), None) => (Some(at), fb),
                (None, Some(&at)) => (Some(at), fa),
                _ => (None, fa),
            };
            if ghost_at.is_some_and(|at| first_added.get(real_fp).is_some_and(|&t| t <= at)) {
                continue;
            }
        }
        dsu.union(a, b);
        reasons.entry(a).or_default().push(reason.clone());
        reasons.entry(b).or_default().push(reason);
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in reasons.keys() {
        let r = dsu.find(*i);
        groups.entry(r).or_default().push(*i);
    }
    let lib = library_root_id(conn)?;
    let mut out = Vec::new();
    for (_, mut idx) in groups {
        if idx.len() < 2 {
            continue;
        }
        idx.sort();
        let mut rs: Vec<String> = idx.iter().flat_map(|i| reasons[i].clone()).collect();
        rs.sort();
        rs.dedup();
        let members: Vec<Book> = idx.iter().map(|i| books[*i].clone()).collect();
        let key = members.iter().map(|b| b.id.to_string()).collect::<Vec<_>>().join("-");
        let confidence = group_confidence(&members);
        let real: Vec<Book> = members.iter().filter(|b| b.id > 0).cloned().collect();
        let keeper_id = real[choose_suspect_keeper(&real, lib)].id;
        let has_deleted = real.len() < members.len();
        out.push(SuspectGroup { key, reason: rs.join("; "), members, confidence, keeper_id, has_deleted });
    }
    Ok(Some(out))
}

fn load_decisions(conn: &Connection) -> Result<HashSet<(String, String)>> {
    let mut st = conn.prepare("SELECT fp_a, fp_b FROM dup_decisions")?;
    let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<HashSet<_>>>()?;
    Ok(v)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    NotDuplicate,
    KeepBoth,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::NotDuplicate => "not_duplicate",
            Decision::KeepBoth => "keep_both",
        }
    }
}

/// Ghi quyết định cho mọi cặp trong nhóm (theo dấu vân tay nội dung, nên vẫn nhớ sau khi đổi tên/quét lại).
pub fn decide(conn: &Connection, book_ids: &[i64], decision: Decision) -> Result<usize> {
    let mut fps = Vec::new();
    for id in book_ids {
        if *id < 0 {
            if let Some(g) = crate::deleted::ghost(conn, *id)? {
                fps.push(ghost_fingerprint(&g));
            }
            continue;
        }
        let Some(b) = db::get_book(conn, *id)? else { continue };
        let fp = match fingerprint(&b).filter(|_| b.hash_size == Some(b.size) && b.hash_mtime == Some(b.mtime)) {
            Some(f) => Some(f),
            None => hash::ensure_quick(conn, b.id)?.map(|q| format!("q:{}:{q}", b.size)),
        };
        if let Some(f) = fp {
            fps.push(f);
        }
    }
    let mut n = 0;
    for i in 0..fps.len() {
        for j in i + 1..fps.len() {
            let (a, b) = pair_key(&fps[i], &fps[j]);
            n += conn.execute(
                "INSERT OR REPLACE INTO dup_decisions(fp_a, fp_b, decision, decided_at) VALUES (?1, ?2, ?3, ?4)",
                params![a, b, decision.as_str(), now()],
            )?;
        }
    }
    Ok(n)
}
