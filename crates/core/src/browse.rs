//! Truy vấn cho lưới thư viện (E3-03 lọc, E3-04 sắp xếp, E3-05 phân trang cho danh sách ảo hoá,
//! E3-07 gộp truyện theo bộ). Lấy cột nhẹ rồi gộp/sắp trong Rust: 50k dòng vẫn chỉ vài chục ms.

use crate::error::Result;
use crate::normalize::{fold, natural_cmp, natural_cmp_folded, volume_cmp};
use crate::search::fts_query;
use rusqlite::{params_from_iter, types::Value, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    /// Mặc định khi đang tìm: theo độ khớp; không tìm: như `Added`.
    #[default]
    Relevance,
    Added,
    Title,
    Author,
    RecentRead,
    Size,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Query {
    pub q: Option<String>,
    /// Tên thư mục thể loại (so khớp bỏ dấu). `""` = sách nằm ngay ở gốc `books`.
    pub category: Option<String>,
    pub format: Option<String>,
    /// Lọc theo tác giả (chuỗi đã chuẩn hoá `norm_author` hoặc tên hiển thị).
    pub author: Option<String>,
    /// `unread` | `reading` | `read`
    pub read_status: Option<String>,
    pub sort: Sort,
    pub desc: bool,
    /// Gộp các vol cùng bộ thành một ô.
    pub group_series: bool,
    /// Chỉ trả các vol của một bộ (màn chi tiết bộ).
    pub series: Option<String>,
    /// Chỉ trả các ô bộ truyện (mục "Bộ truyện").
    pub only_series: bool,
    /// Chỉ sách không đọc được (lỗi đọc metadata / file hỏng) hoặc có DRM (E10-04).
    pub problems: bool,
    /// Chỉ sách đang có trên iPad (I3-06): theo danh sách iPad báo về.
    pub on_ipad: bool,
    /// Mục "Không đáng đọc": chỉ trả các cuốn đã đánh dấu. Mặc định (`false`) các cuốn này bị ẩn khỏi mọi danh sách.
    pub not_worth: bool,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct BookCard {
    pub id: i64,
    pub title: String,
    pub authors: String,
    pub format: String,
    pub size: i64,
    pub category: Option<String>,
    pub series: Option<String>,
    pub volume: Option<i64>,
    pub read_status: String,
    pub progress: Option<f64>,
    pub added_at: i64,
    pub last_read_at: Option<i64>,
    pub file_name: String,
    pub path: String,
    pub rel_path: String,
    pub in_inbox: bool,
    pub page_count: Option<i64>,
    pub mtime: i64,
    /// Lỗi khi đọc file lúc quét (file hỏng, định dạng lạ…).
    pub error: Option<String>,
    pub drm: bool,
    /// Đang có trên iPad (I3-06).
    pub on_ipad: bool,
    /// Đã đánh dấu "không đáng đọc".
    pub not_worth: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesCard {
    pub category: Option<String>,
    pub series: String,
    pub volumes: usize,
    pub read: usize,
    pub cover_book_id: i64,
    pub continue_book_id: Option<i64>,
    pub size: i64,
    pub authors: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
// Phần lớn mục là sách; đóng hộp BookCard chỉ thêm một lần cấp phát mỗi mục mà không lợi gì.
#[allow(clippy::large_enum_variant)]
pub enum Item {
    Book(BookCard),
    Series(SeriesCard),
}

#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub total: usize,
    pub offset: usize,
    pub items: Vec<Item>,
}

const COLS: &str = "id, title, authors, format, size, category, series, volume, read_status, progress, added_at, \
    last_read_at, file_name, path, rel_path, in_inbox, page_count, mtime, norm_author, error, drm, \
    (id IN (SELECT value FROM devices, json_each(devices.on_device))), not_worth";

fn card(r: &rusqlite::Row) -> rusqlite::Result<(BookCard, String)> {
    Ok((
        BookCard {
            id: r.get(0)?,
            title: r.get(1)?,
            authors: r.get(2)?,
            format: r.get(3)?,
            size: r.get(4)?,
            category: r.get(5)?,
            series: r.get(6)?,
            volume: r.get(7)?,
            read_status: r.get(8)?,
            progress: r.get(9)?,
            added_at: r.get(10)?,
            last_read_at: r.get(11)?,
            file_name: r.get(12)?,
            path: r.get(13)?,
            rel_path: r.get(14)?,
            in_inbox: r.get(15)?,
            page_count: r.get(16)?,
            mtime: r.get(17)?,
            error: r.get(19)?,
            drm: r.get(20)?,
            on_ipad: r.get(21)?,
            not_worth: r.get(22)?,
        },
        r.get(18)?,
    ))
}

fn vol_cmp(a: &BookCard, b: &BookCard) -> std::cmp::Ordering {
    volume_cmp(a.volume, &a.file_name, b.volume, &b.file_name)
}

pub fn query(conn: &Connection, q: &Query) -> Result<Page> {
    let mut where_ = vec!["status = 'present'".to_string(), format!("not_worth = {}", q.not_worth as i32)];
    let mut args: Vec<Value> = Vec::new();
    let fq = q.q.as_deref().and_then(fts_query);
    let mut rank: HashMap<i64, usize> = HashMap::new();
    if let Some(fq) = &fq {
        let mut st = conn.prepare_cached(
            "SELECT rowid FROM books_fts WHERE books_fts MATCH ?1 ORDER BY bm25(books_fts, 10.0, 4.0, 2.0, 3.0)",
        )?;
        let ids: Vec<i64> = st.query_map([fq], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        if ids.is_empty() {
            return Ok(Page { total: 0, offset: q.offset, items: vec![] });
        }
        for (i, id) in ids.iter().enumerate() {
            rank.insert(*id, i);
        }
        where_.push(format!("id IN ({})", ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")));
    }
    if let Some(f) = &q.format {
        where_.push("format = ?".into());
        args.push(Value::Text(f.clone()));
    }
    if let Some(s) = &q.read_status {
        where_.push("read_status = ?".into());
        args.push(Value::Text(s.clone()));
    }
    if let Some(s) = &q.series {
        where_.push("series = ?".into());
        args.push(Value::Text(s.clone()));
    }
    if q.problems {
        where_.push("(error IS NOT NULL OR drm = 1)".into());
    }
    if q.on_ipad {
        where_.push("id IN (SELECT value FROM devices, json_each(devices.on_device))".into());
    }
    let sql = format!("SELECT {COLS} FROM books WHERE {}", where_.join(" AND "));
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(params_from_iter(args.iter()), card)?.collect::<rusqlite::Result<Vec<_>>>()?;
    let cat_key = q.category.as_deref().map(fold);
    let author_key = q.author.as_deref().map(crate::normalize::normalize_author);
    let mut books: Vec<BookCard> = rows
        .into_iter()
        .filter(|(b, _)| match &cat_key {
            None => true,
            Some(k) if k.is_empty() => b.category.is_none(),
            Some(k) => b.category.as_deref().map(fold).as_deref() == Some(k.as_str()),
        })
        .filter(|(_, na)| author_key.as_ref().is_none_or(|k| na == k || (!k.is_empty() && na.contains(k.as_str()))))
        .map(|(b, _)| b)
        .collect();

    let sort = if q.sort == Sort::Relevance && fq.is_none() { Sort::Added } else { q.sort };
    let key_cmp = |a: &BookCard, b: &BookCard| -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let o = match sort {
            Sort::Relevance => rank.get(&a.id).cmp(&rank.get(&b.id)),
            Sort::Added => b.added_at.cmp(&a.added_at).then(b.id.cmp(&a.id)),
            Sort::Title => natural_cmp(&a.title, &b.title),
            Sort::Author => natural_cmp(&a.authors, &b.authors).then_with(|| natural_cmp(&a.title, &b.title)),
            Sort::RecentRead => b.last_read_at.cmp(&a.last_read_at),
            Sort::Size => b.size.cmp(&a.size),
        };
        if o == Ordering::Equal {
            natural_cmp(&a.title, &b.title)
        } else {
            o
        }
    };

    let mut items: Vec<(Item, BookCard)> = Vec::new(); // (item, đại diện để sắp xếp)
    if q.group_series && q.series.is_none() {
        let mut groups: HashMap<(Option<String>, String), Vec<BookCard>> = HashMap::new();
        let mut singles = Vec::new();
        for b in books {
            match b.series.clone() {
                Some(s) => groups.entry((b.category.clone(), s)).or_default().push(b),
                None => singles.push(b),
            }
        }
        if !q.only_series {
            for b in singles {
                items.push((Item::Book(b.clone()), b));
            }
        }
        for ((category, series), mut vols) in groups {
            if vols.len() == 1 && !q.only_series {
                let b = vols.pop().unwrap();
                items.push((Item::Book(b.clone()), b));
                continue;
            }
            vols.sort_by(vol_cmp);
            let continue_book_id = vols.iter().find(|b| b.read_status == "reading").map(|b| b.id).or_else(|| {
                let last = vols.iter().rposition(|b| b.read_status == "read")?;
                vols.get(last + 1).map(|b| b.id)
            });
            // Đại diện: vol khớp tốt nhất / mới nhất / đọc gần nhất theo kiểu sắp xếp.
            let mut rep = vols[0].clone();
            for v in &vols {
                if key_cmp(v, &rep) == std::cmp::Ordering::Less {
                    rep = v.clone();
                }
            }
            rep.title = series.clone();
            rep.authors = vols.iter().find(|b| !b.authors.is_empty()).map(|b| b.authors.clone()).unwrap_or_default();
            rep.size = vols.iter().map(|b| b.size).sum();
            let card = SeriesCard {
                category,
                volumes: vols.len(),
                read: vols.iter().filter(|b| b.read_status == "read").count(),
                cover_book_id: vols[0].id,
                continue_book_id,
                size: rep.size,
                authors: rep.authors.clone(),
                series,
            };
            items.push((Item::Series(card), rep));
        }
    } else {
        if q.series.is_some() {
            books.sort_by(vol_cmp);
        }
        for b in books {
            items.push((Item::Book(b.clone()), b));
        }
    }
    if q.series.is_none() {
        if matches!(sort, Sort::Title | Sort::Author) {
            // Bỏ dấu một lần cho mỗi mục rồi mới sắp (E10-03: 50k mục).
            let mut keyed: Vec<(String, String, (Item, BookCard))> =
                items.into_iter().map(|it| (fold(&it.1.title), fold(&it.1.authors), it)).collect();
            let by_title = |a: &(String, String, (Item, BookCard)), b: &(String, String, (Item, BookCard))| {
                natural_cmp_folded(&a.0, &b.0).then_with(|| a.2 .1.title.cmp(&b.2 .1.title))
            };
            if sort == Sort::Author {
                keyed.sort_by(|a, b| natural_cmp_folded(&a.1, &b.1).then_with(|| by_title(a, b)));
            } else {
                keyed.sort_by(by_title);
            }
            items = keyed.into_iter().map(|x| x.2).collect();
        } else {
            items.sort_by(|a, b| key_cmp(&a.1, &b.1));
        }
    }
    if q.desc {
        items.reverse();
    }
    let total = items.len();
    let limit = if q.limit == 0 { 200 } else { q.limit };
    let items = items.into_iter().skip(q.offset).take(limit).map(|(i, _)| i).collect();
    Ok(Page { total, offset: q.offset, items })
}

#[derive(Debug, Clone, Serialize)]
pub struct Facets {
    pub formats: Vec<(String, i64)>,
    pub read_status: Vec<(String, i64)>,
    pub total: i64,
    pub bytes: i64,
    pub inbox: i64,
    pub missing: i64,
    /// Sách không đọc được hoặc có DRM (E10-04).
    pub problems: i64,
    /// Số sách đang có trên iPad.
    pub on_ipad: i64,
    /// Số sách đã đánh dấu "không đáng đọc" (nằm trong `total` nhưng không hiện trong thư viện).
    pub not_worth: i64,
}

/// Số liệu cho bộ lọc và trang Tổng quan (E9-02).
pub fn facets(conn: &Connection) -> Result<Facets> {
    let pairs = |sql: &str| -> Result<Vec<(String, i64)>> {
        let mut st = conn.prepare(sql)?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    };
    let (total, bytes, inbox): (i64, Option<i64>, i64) = conn.query_row(
        "SELECT COUNT(*), SUM(size), COALESCE(SUM(in_inbox), 0) FROM books WHERE status = 'present'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(Facets {
        formats: pairs("SELECT format, COUNT(*) FROM books WHERE status='present' GROUP BY format ORDER BY 2 DESC")?,
        read_status: pairs(
            "SELECT read_status, COUNT(*) FROM books WHERE status='present' AND not_worth = 0 GROUP BY read_status",
        )?,
        total,
        bytes: bytes.unwrap_or(0),
        inbox,
        missing: conn.query_row("SELECT COUNT(*) FROM books WHERE status = 'missing'", [], |r| r.get(0))?,
        problems: conn.query_row(
            "SELECT COUNT(*) FROM books WHERE status = 'present' AND (error IS NOT NULL OR drm = 1)",
            [],
            |r| r.get(0),
        )?,
        on_ipad: conn.query_row(
            "SELECT COUNT(*) FROM books WHERE status = 'present' AND id IN (SELECT value FROM devices, json_each(devices.on_device))",
            [],
            |r| r.get(0),
        )?,
        not_worth: conn.query_row("SELECT COUNT(*) FROM books WHERE status = 'present' AND not_worth = 1", [], |r| {
            r.get(0)
        })?,
    })
}

/// Đánh dấu "không đáng đọc" (`true`: ẩn khỏi thư viện, chuyển sang mục riêng) hoặc đưa lại vào thư viện (`false`).
/// Ghi lại thời điểm để đồng bộ với iPad/iPhone/Android ("ai đổi sau thắng"). Trả về số cuốn đã đổi.
pub fn set_not_worth(conn: &Connection, ids: &[i64], not_worth: bool) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let t = crate::db::now();
    let mut n = 0;
    for id in ids {
        // Đổi hai lần trong cùng một giây vẫn tăng mốc, để lần đổi sau cùng thắng bản trên thiết bị.
        n += tx.execute(
            "UPDATE books SET not_worth = ?2, worth_at = MAX(?3, worth_at + 1) WHERE id = ?1 AND not_worth <> ?2",
            rusqlite::params![id, not_worth, t],
        )?;
    }
    tx.commit()?;
    Ok(n)
}

/// Đặt trạng thái đọc (E3-03; trình đọc M5 sẽ cập nhật tự động).
pub fn set_read_status(conn: &Connection, ids: &[i64], status: &str) -> Result<()> {
    if !["unread", "reading", "read"].contains(&status) {
        return Err(crate::Error::msg("trạng thái đọc không hợp lệ"));
    }
    let tx = conn.unchecked_transaction()?;
    for id in ids {
        tx.execute(
            "UPDATE books SET read_status = ?2, last_read_at = CASE WHEN ?2 = 'unread' THEN last_read_at ELSE ?3 END,
               progress = CASE WHEN ?2 = 'read' THEN 1.0 WHEN ?2 = 'unread' THEN NULL ELSE progress END
             WHERE id = ?1",
            rusqlite::params![id, status, crate::db::now()],
        )?;
    }
    tx.commit()?;
    Ok(())
}
