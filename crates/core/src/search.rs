//! Tìm kiếm FTS5 không dấu, khớp tiền tố (E3-01); nhóm truyện theo bộ (E3-07).

use crate::db::{Book, BOOK_COLS};
use crate::error::Result;
use crate::normalize::{natural_cmp, simple_key};
use rusqlite::{params, Connection};
use serde::Serialize;

/// Chuyển câu người dùng gõ thành truy vấn FTS5: mỗi từ là một tiền tố, AND với nhau.
pub fn fts_query(q: &str) -> Option<String> {
    let toks: Vec<String> =
        simple_key(q).split(' ').filter(|t| !t.is_empty()).map(|t| format!("\"{}\"*", t.replace('"', ""))).collect();
    if toks.is_empty() {
        None
    } else {
        Some(toks.join(" "))
    }
}

/// Tìm theo tên sách, tác giả, tên file, tên bộ. "mat bi" → "Mắt Biếc".
pub fn search(conn: &Connection, q: &str, limit: usize) -> Result<Vec<Book>> {
    let Some(fq) = fts_query(q) else {
        return recent(conn, limit);
    };
    let cols = BOOK_COLS.split(", ").map(|c| format!("b.{}", c.trim())).collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT {cols} FROM books b
         JOIN (SELECT rowid AS rid, bm25(books_fts, 10.0, 4.0, 2.0, 3.0) AS rank
               FROM books_fts WHERE books_fts MATCH ?1) f ON b.id = f.rid
         WHERE b.status = 'present'
         ORDER BY f.rank, b.title
         LIMIT ?2"
    );
    let mut st = conn.prepare_cached(&sql)?;
    let rows = st.query_map(params![fq, limit as i64], Book::from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn recent(conn: &Connection, limit: usize) -> Result<Vec<Book>> {
    crate::db::books_where(conn, "status = 'present' ORDER BY added_at DESC, id DESC LIMIT ?1", [limit as i64])
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesSummary {
    pub category: Option<String>,
    pub series: String,
    pub volumes: usize,
    pub read: usize,
    pub first_book_id: i64,
    /// Vol đang đọc dở (nút "Đọc tiếp").
    pub continue_book_id: Option<i64>,
}

/// Mỗi thư mục bộ = một ô trong lưới (E3-07).
pub fn list_series(conn: &Connection) -> Result<Vec<SeriesSummary>> {
    let books =
        crate::db::books_where(conn, "status = 'present' AND series IS NOT NULL ORDER BY category, series", [])?;
    let mut out: Vec<SeriesSummary> = Vec::new();
    let mut i = 0;
    while i < books.len() {
        let mut j = i;
        while j < books.len() && books[j].category == books[i].category && books[j].series == books[i].series {
            j += 1;
        }
        let mut group: Vec<&Book> = books[i..j].iter().collect();
        sort_volumes(&mut group);
        out.push(SeriesSummary {
            category: books[i].category.clone(),
            series: books[i].series.clone().unwrap_or_default(),
            volumes: group.len(),
            read: group.iter().filter(|b| b.read_status == "read").count(),
            first_book_id: group[0].id,
            continue_book_id: group.iter().find(|b| b.read_status == "reading").map(|b| b.id).or_else(|| {
                // Vol đầu tiên chưa đọc sau vol đã đọc cuối cùng.
                let last_read = group.iter().rposition(|b| b.read_status == "read")?;
                group.get(last_read + 1).map(|b| b.id)
            }),
        });
        i = j;
    }
    out.sort_by(|a, b| natural_cmp(&a.series, &b.series));
    Ok(out)
}

/// Các vol của một bộ theo thứ tự tự nhiên (Vol 2 trước Vol 10).
pub fn series_volumes(conn: &Connection, category: Option<&str>, series: &str) -> Result<Vec<Book>> {
    let mut v = crate::db::books_where(
        conn,
        "status = 'present' AND series = ?1 AND category IS ?2",
        params![series, category],
    )?;
    let mut refs: Vec<&Book> = v.iter().collect();
    sort_volumes(&mut refs);
    let order: Vec<i64> = refs.iter().map(|b| b.id).collect();
    v.sort_by_key(|b| order.iter().position(|id| *id == b.id));
    Ok(v)
}

fn sort_volumes(v: &mut [&Book]) {
    v.sort_by(|a, b| crate::normalize::volume_cmp(a.volume, &a.file_name, b.volume, &b.file_name));
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryCount {
    pub category: String,
    pub books: i64,
    pub bytes: i64,
}

/// Số sách mỗi thể loại (E6-01).
pub fn category_counts(conn: &Connection) -> Result<Vec<CategoryCount>> {
    let mut st = conn.prepare(
        "SELECT COALESCE(category, ''), COUNT(*), SUM(size) FROM books WHERE status = 'present'
         GROUP BY category ORDER BY category",
    )?;
    let rows = st
        .query_map([], |r| Ok(CategoryCount { category: r.get(0)?, books: r.get(1)?, bytes: r.get(2)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_building() {
        assert_eq!(fts_query("Mắt bi").as_deref(), Some("\"mat\"* \"bi\"*"));
        assert_eq!(fts_query("  \"  "), None);
    }
}
