//! Sao lưu DB tự động (E10-06): mỗi ngày một bản `VACUUM INTO` trong `<home>/backups`, giữ 7 bản.
//! Khi mở app mà `library.db` hỏng: giữ lại file hỏng (đổi tên), khôi phục từ bản sao lưu mới nhất còn
//! đọc được; không có bản nào thì tạo DB mới (quét lại sẽ dựng lại chỉ mục, mất tiến độ đọc/nhật ký).

use crate::db;
use crate::error::Result;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

pub const KEEP: usize = 7;
pub const DAILY: i64 = 20 * 3600;

pub fn dir(home: &Path) -> PathBuf {
    home.join("backups")
}

/// Các bản sao lưu, mới nhất trước: (thời điểm unix, đường dẫn).
pub fn list(home: &Path) -> Vec<(i64, PathBuf)> {
    let mut v: Vec<(i64, PathBuf)> = std::fs::read_dir(dir(home))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let ts = p.file_stem()?.to_str()?.strip_prefix("library-")?.parse().ok()?;
                    (p.extension()? == "db").then_some((ts, p))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|x| std::cmp::Reverse(x.0));
    v
}

/// Sao lưu nếu bản mới nhất cũ hơn `max_age` giây. Trả đường dẫn bản mới (nếu có tạo).
pub fn backup_if_due(conn: &Connection, home: &Path, max_age: i64) -> Result<Option<PathBuf>> {
    if list(home).first().is_some_and(|(ts, _)| db::now() - ts < max_age) {
        return Ok(None);
    }
    db::backup(conn, &dir(home), KEEP).map(Some)
}

/// DB mở được và qua `PRAGMA quick_check`?
fn healthy(path: &Path) -> bool {
    let Ok(c) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else { return false };
    c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)).is_ok_and(|s| s == "ok")
}

/// Mở `library.db`; nếu hỏng thì khôi phục. Trả (kết nối, thông báo cho người dùng nếu đã phải khôi phục).
pub fn open_or_recover(home: &Path) -> Result<(Connection, Option<String>)> {
    let path = home.join("library.db");
    if !path.exists() || healthy(&path) {
        if let Ok(c) = db::open(&path) {
            return Ok((c, None));
        }
    }
    // Hỏng: cất file hỏng (kèm -wal/-shm) sang bên cạnh, không xoá.
    let stamp = db::now();
    let kept = home.join(format!("library.db.corrupt-{stamp}"));
    std::fs::rename(&path, &kept)?;
    for ext in ["-wal", "-shm"] {
        let side = home.join(format!("library.db{ext}"));
        if side.exists() {
            let _ = std::fs::rename(&side, home.join(format!("library.db{ext}.corrupt-{stamp}")));
        }
    }
    tracing::error!("library.db hỏng – đã cất thành {}", kept.display());
    for (ts, b) in list(home) {
        if !healthy(&b) {
            continue;
        }
        std::fs::copy(&b, &path)?;
        if let Ok(c) = db::open(&path) {
            let when = chrono_like(ts);
            tracing::warn!("đã khôi phục DB từ {}", b.display());
            return Ok((
                c,
                Some(format!(
                    "Dữ liệu thư viện bị hỏng nên đã được khôi phục từ bản sao lưu lúc {when}. \
                     Thay đổi sau thời điểm đó (tiến độ đọc, nhật ký) có thể bị mất; bấm Quét thư viện để cập nhật. \
                     File hỏng được giữ ở {}.",
                    kept.display()
                )),
            ));
        }
        let _ = std::fs::remove_file(&path);
    }
    let c = db::open(&path)?;
    Ok((
        c,
        Some(format!(
            "Dữ liệu thư viện bị hỏng và không có bản sao lưu dùng được nên đã tạo mới. \
             Bấm Quét thư viện để dựng lại; file hỏng được giữ ở {}.",
            kept.display()
        )),
    ))
}

/// "dd/mm/yyyy hh:mm" (UTC+7 – giờ Việt Nam) không cần thêm thư viện thời gian.
fn chrono_like(ts: i64) -> String {
    let t = ts + 7 * 3600;
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Thuật toán civil_from_days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{d:02}/{m:02}/{y} {:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_format() {
        assert_eq!(chrono_like(0), "01/01/1970 07:00");
        assert_eq!(chrono_like(1_793_966_400), "06/11/2026 19:00");
    }

    #[test]
    fn backup_daily_and_recover() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let (conn, notice) = open_or_recover(home).unwrap();
        assert!(notice.is_none());
        db::ensure_root(&conn, "C:/books", "library").unwrap();
        assert!(backup_if_due(&conn, home, DAILY).unwrap().is_some());
        assert!(backup_if_due(&conn, home, DAILY).unwrap().is_none(), "mỗi ngày một bản");
        drop(conn);

        // Phá header của DB.
        let p = home.join("library.db");
        let mut bytes = std::fs::read(&p).unwrap();
        bytes[..100].fill(0x5a);
        std::fs::write(&p, bytes).unwrap();
        let (conn, notice) = open_or_recover(home).unwrap();
        assert!(notice.unwrap().contains("khôi phục từ bản sao lưu"));
        let roots: i64 = conn.query_row("SELECT COUNT(*) FROM roots", [], |r| r.get(0)).unwrap();
        assert_eq!(roots, 1, "dữ liệu từ bản sao lưu");
        assert!(std::fs::read_dir(home)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("library.db.corrupt-")));
        drop(conn);

        // Hỏng và không có bản sao lưu → DB mới.
        std::fs::remove_dir_all(dir(home)).unwrap();
        std::fs::write(&p, b"rac rac rac rac rac rac rac rac rac rac rac rac rac rac rac rac rac rac").unwrap();
        let (conn, notice) = open_or_recover(home).unwrap();
        assert!(notice.unwrap().contains("tạo mới"));
        assert!(db::schema_version(&conn).unwrap() > 0);
    }
}
