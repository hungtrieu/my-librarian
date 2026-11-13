//! Chuỗi phân tích đầy đủ + báo cáo mỗi lần quét (E8-01, E8-04, E8-05, E8-07, E9-01).
//! Tác vụ ngầm CHỈ lập danh sách (D4): không xoá, không di chuyển – trừ khi bật E8-05 (tự dọn trùng) hoặc
//! `auto_triage` (tự phân loại hộp thư đến, mặc định bật, tắt được trong Cài đặt).

use crate::config::Config;
use crate::error::Result;
use crate::scan::{ScanOptions, ScanSummary};
use crate::trash::Trasher;
use crate::{classify, dedupe, hash, scan, OnStep, Step};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ReportStats {
    pub scan: ScanSummary,
    pub total_books: i64,
    pub total_bytes: i64,
    pub exact_groups: usize,
    pub exact_files: usize,
    pub reclaimable_bytes: i64,
    pub suspect_groups: usize,
    pub inbox_books: usize,
    pub inbox_proposed: usize,
    pub missing_books: i64,
    pub hash_errors: usize,
    pub auto_cleaned: Option<usize>,
    /// Số file sách đã tự phân loại từ hộp thư đến ở đầu lần quét (khi bật `auto_triage`).
    #[serde(default)]
    pub auto_triaged: Option<usize>,
}

/// Quét + băm + phát hiện trùng + đếm sách chờ phân loại. Lưu kết quả vào `scan_reports`.
pub fn run_analysis(
    conn: &Connection,
    cfg: &Config,
    kind: &str,
    cancel: Option<&AtomicBool>,
    auto_clean: Option<&dyn Trasher>,
) -> Result<ReportStats> {
    run_analysis_with(conn, cfg, kind, cancel, auto_clean, None)
}

/// Như [`run_analysis`], báo tiến trình từng bước qua `on`. Mỗi bước lưu kết quả ngay theo từng sách/lô, nên
/// huỷ hoặc tắt máy giữa chừng thì lần sau chỉ làm phần chưa xong.
pub fn run_analysis_with(
    conn: &Connection,
    cfg: &Config,
    kind: &str,
    cancel: Option<&AtomicBool>,
    auto_clean: Option<&dyn Trasher>,
    on: Option<OnStep>,
) -> Result<ReportStats> {
    let is_cancelled = || cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed));
    // Phân loại tự động hộp thư đến TRƯỚC khi quét, để lần quét thấy ngay vị trí mới.
    let mut auto_triaged = None;
    if cfg.auto_triage && !cfg.read_only && !is_cancelled() {
        let journal = crate::config::default_home().join("triage-journal.jsonl");
        match crate::triage::run(
            cfg,
            auto_clean.unwrap_or(&crate::trash::SystemTrasher),
            &crate::triage::TriageOptions {
                dry_run: false,
                journal: Some(&journal),
                cancel,
                progress: on,
                conn: Some(conn),
            },
        ) {
            Ok(r) => {
                tracing::info!(
                    "tự phân loại: {} tác phẩm ({} vào Khác), {} file chuyển, {} thư mục thừa vào Thùng rác, {} lỗi",
                    r.units,
                    r.to_other,
                    r.moved_files,
                    r.dirs_trashed,
                    r.failures.len()
                );
                for f in &r.failures {
                    tracing::warn!("tự phân loại: {f}");
                }
                auto_triaged = Some(r.moved_files);
            }
            Err(e) => tracing::warn!("tự phân loại lỗi: {e}"),
        }
    }
    let scan_cb = |p: &scan::ScanProgress| {
        if let Some(f) = on {
            f(Step {
                phase: p.phase,
                done: p.files_seen,
                total: if p.total == 0 { 0 } else { p.total.max(p.files_seen) },
            });
        }
    };
    let summary = scan::scan(
        conn,
        cfg,
        &ScanOptions { cancel, progress: on.map(|_| &scan_cb as &dyn Fn(&scan::ScanProgress)), kind: Some(kind) },
    )?;
    let mut st = ReportStats { scan: summary.clone(), auto_triaged, ..Default::default() };
    if !summary.cancelled {
        let hs = hash::ensure_hashes_with(conn, cfg.hash_threads, cancel, on)?;
        st.hash_errors = hs.errors.len();
        if hs.cancelled || is_cancelled() {
            st.scan.cancelled = true;
        }
    }
    if !st.scan.cancelled {
        let step = |done: usize| {
            if let Some(f) = on {
                f(Step { phase: "analyze", done, total: 4 });
            }
        };
        step(0);
        let groups = dedupe::exact_groups(conn)?;
        step(1);
        st.exact_groups = groups.len();
        st.exact_files = groups.iter().map(|g| g.others.len()).sum();
        st.reclaimable_bytes = groups.iter().map(|g| g.reclaimable).sum();
        if let Some(t) = auto_clean {
            if cfg.auto_clean_exact_in_background && !cfg.read_only && !groups.is_empty() {
                let rep = dedupe::clean_exact(conn, cfg, t, &groups, &HashMap::new())?;
                st.auto_cleaned = Some(rep.trashed);
            }
        }
        step(2);
        st.suspect_groups = dedupe::suspect_groups(conn)?.len();
        step(3);
        if cfg.library_root.as_ref().is_some_and(|r| r.is_dir()) {
            let props = classify::propose(conn, cfg)?;
            st.inbox_books = props.len();
            st.inbox_proposed = props.iter().filter(|p| p.target.is_some()).count();
        }
        step(4);
    }
    let (n, bytes): (i64, Option<i64>) =
        conn.query_row("SELECT COUNT(*), SUM(size) FROM books WHERE status = 'present'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    st.total_books = n;
    st.total_bytes = bytes.unwrap_or(0);
    st.missing_books = conn.query_row("SELECT COUNT(*) FROM books WHERE status = 'missing'", [], |r| r.get(0))?;
    conn.execute(
        "UPDATE scan_reports SET stats = ?2 WHERE id = ?1",
        params![summary.scan_id, serde_json::to_string(&st)?],
    )?;
    Ok(st)
}

/// Tính lại các số "việc cần làm" (trùng chắc chắn, nghi trùng, chờ phân loại) từ dữ liệu hiện tại và ghi đè vào
/// báo cáo gần nhất. Gọi sau khi dọn trùng / xoá / duyệt / di chuyển: badge và banner lấy số từ báo cáo,
/// nếu không cập nhật thì vẫn hiện số cũ của lần quét trước dù đã dọn xong.
pub fn refresh_counts(conn: &Connection, cfg: &Config) -> Result<()> {
    let Some(rep) = latest(conn)? else { return Ok(()) };
    let Some(mut st) = rep.stats else { return Ok(()) };
    let groups = dedupe::exact_groups(conn)?;
    st.exact_groups = groups.len();
    st.exact_files = groups.iter().map(|g| g.others.len()).sum();
    st.reclaimable_bytes = groups.iter().map(|g| g.reclaimable).sum();
    st.suspect_groups = dedupe::suspect_groups(conn)?.len();
    if cfg.library_root.as_ref().is_some_and(|r| r.is_dir()) {
        let props = classify::propose(conn, cfg)?;
        st.inbox_books = props.len();
        st.inbox_proposed = props.iter().filter(|p| p.target.is_some()).count();
    }
    conn.execute("UPDATE scan_reports SET stats = ?2 WHERE id = ?1", params![rep.id, serde_json::to_string(&st)?])?;
    Ok(())
}

/// Màn "Trùng chắc chắn" vừa tính danh sách nhóm: ghi số đó vào báo cáo nếu khác (không tính lại phần khác).
/// Trả `true` nếu số đã đổi (giao diện cần tải lại badge).
pub fn sync_exact(conn: &Connection, groups: &[dedupe::ExactGroup]) -> Result<bool> {
    update_stats(conn, |st| {
        let files = groups.iter().map(|g| g.others.len()).sum();
        let changed = st.exact_groups != groups.len() || st.exact_files != files;
        st.exact_groups = groups.len();
        st.exact_files = files;
        st.reclaimable_bytes = groups.iter().map(|g| g.reclaimable).sum();
        changed
    })
}

/// Như [`sync_exact`] cho màn "Nghi trùng".
pub fn sync_suspects(conn: &Connection, groups: usize) -> Result<bool> {
    update_stats(conn, |st| std::mem::replace(&mut st.suspect_groups, groups) != groups)
}

fn update_stats(conn: &Connection, f: impl FnOnce(&mut ReportStats) -> bool) -> Result<bool> {
    let Some(rep) = latest(conn)? else { return Ok(false) };
    let Some(mut st) = rep.stats else { return Ok(false) };
    if !f(&mut st) {
        return Ok(false);
    }
    conn.execute("UPDATE scan_reports SET stats = ?2 WHERE id = ?1", params![rep.id, serde_json::to_string(&st)?])?;
    Ok(true)
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub id: i64,
    pub kind: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: String,
    pub stats: Option<ReportStats>,
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<Report> {
    let s: Option<String> = r.get(5)?;
    Ok(Report {
        id: r.get(0)?,
        kind: r.get(1)?,
        started_at: r.get(2)?,
        finished_at: r.get(3)?,
        status: r.get(4)?,
        stats: s.and_then(|s| serde_json::from_str(&s).ok()),
    })
}

pub fn latest(conn: &Connection) -> Result<Option<Report>> {
    Ok(conn
        .query_row(
            "SELECT id, kind, started_at, finished_at, status, stats FROM scan_reports ORDER BY id DESC LIMIT 1",
            [],
            row,
        )
        .optional()?)
}

/// Lịch sử báo cáo (E8-04).
pub fn history(conn: &Connection, limit: usize) -> Result<Vec<Report>> {
    let mut st = conn.prepare(
        "SELECT id, kind, started_at, finished_at, status, stats FROM scan_reports ORDER BY id DESC LIMIT ?1",
    )?;
    let v = st.query_map([limit as i64], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Dòng banner khi mở app (E9-01), vd. "Từ lần quét ngầm 03:00: 12 nghi trùng, 4 trùng chắc chắn, 9 sách mới chờ phân loại".
pub fn banner(r: &Report) -> Option<String> {
    let s = r.stats.as_ref()?;
    let mut parts = Vec::new();
    if s.suspect_groups > 0 {
        parts.push(format!("{} nhóm nghi trùng", s.suspect_groups));
    }
    if s.exact_files > 0 {
        parts.push(format!("{} bản trùng chắc chắn ({})", s.exact_files, human_bytes(s.reclaimable_bytes)));
    }
    if s.inbox_books > 0 {
        parts.push(format!("{} sách mới chờ phân loại", s.inbox_books));
    }
    if s.scan.missing > 0 {
        parts.push(format!("{} sách mất", s.scan.missing));
    }
    let mut out = String::new();
    let label = if r.kind == "background" { "lần quét ngầm" } else { "lần quét" };
    if !s.scan.skipped_roots.is_empty() {
        out.push_str(&format!("Bỏ qua – ổ chưa kết nối: {}. ", s.scan.skipped_roots.join(", ")));
    }
    if parts.is_empty() {
        out.push_str(&format!("Từ {label} gần nhất: không có gì cần xử lý."));
    } else {
        out.push_str(&format!("Từ {label} gần nhất: {}.", parts.join(", ")));
    }
    Some(out)
}

pub fn human_bytes(b: i64) -> String {
    let b = b as f64;
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", b as i64, U[0])
    } else {
        format!("{v:.1} {}", U[i])
    }
}
