//! Chuỗi phân tích đầy đủ + báo cáo mỗi lần quét (E8-04, E8-07, E9-01).
//! Phân tích chỉ lập danh sách (D4): không xoá, không di chuyển – trừ khi bật `auto_triage` (tự phân loại hộp thư
//! đến, mặc định bật, tắt được trong Cài đặt) và các bước tự dọn đi kèm.

use crate::config::Config;
use crate::error::Result;
use crate::scan::{ScanOptions, ScanSummary};
use crate::trash::Trasher;
use crate::{classify, dedupe, hash, scan, OnStep, Step};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
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
    /// Số file sách đã tự phân loại từ hộp thư đến ở đầu lần quét (khi bật `auto_triage`).
    #[serde(default)]
    pub auto_triaged: Option<usize>,
    /// Số sách mới tải về trùng y hệt một cuốn đã xoá trước đây, đã tự đưa vào Thùng rác (khi bật
    /// `auto_reject_deleted`).
    #[serde(default)]
    pub auto_rejected: Option<usize>,
    /// Số file sách thuộc thể loại bị chặn đã tự đưa vào Thùng rác (từ hộp thư đến + thư mục bị chặn còn trên đĩa).
    #[serde(default)]
    pub auto_blocked: Option<usize>,
    /// Số file sách đã đưa ra khỏi thư mục con không do app quản lý (khi bật `flatten_subfolders`).
    #[serde(default)]
    pub auto_flattened: Option<usize>,
}

/// Quét + băm + phát hiện trùng + đếm sách chờ phân loại. Lưu kết quả vào `scan_reports`.
pub fn run_analysis(
    conn: &Connection,
    cfg: &Config,
    kind: &str,
    cancel: Option<&AtomicBool>,
    trasher: Option<&dyn Trasher>,
) -> Result<ReportStats> {
    run_analysis_with(conn, cfg, kind, cancel, trasher, None)
}

/// Như [`run_analysis`], báo tiến trình từng bước qua `on`. Mỗi bước lưu kết quả ngay theo từng sách/lô, nên
/// huỷ hoặc tắt máy giữa chừng thì lần sau chỉ làm phần chưa xong.
pub fn run_analysis_with(
    conn: &Connection,
    cfg: &Config,
    kind: &str,
    cancel: Option<&AtomicBool>,
    trasher: Option<&dyn Trasher>,
    on: Option<OnStep>,
) -> Result<ReportStats> {
    let is_cancelled = || cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed));
    // Phân loại tự động hộp thư đến TRƯỚC khi quét, để lần quét thấy ngay vị trí mới.
    let mut auto_triaged = None;
    let mut auto_blocked = 0usize;
    if cfg.auto_triage && !cfg.read_only && !is_cancelled() {
        let journal = crate::config::default_home().join("triage-journal.jsonl");
        match crate::triage::run(
            cfg,
            trasher.unwrap_or(&crate::trash::SystemTrasher),
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
                for (rel, n) in &r.blocked {
                    tracing::info!("tự phân loại: {n} tác phẩm thuộc thể loại bị chặn “{rel}” vào Thùng rác");
                }
                auto_triaged = Some(r.moved_files);
                auto_blocked += r.blocked_trashed;
            }
            Err(e) => tracing::warn!("tự phân loại lỗi: {e}"),
        }
        // Thư viện cũ có `Khác` phẳng (hàng chục nghìn file, Explorer/Finder mở rất chậm): chia vào thư mục con.
        if !is_cancelled() {
            let opts = crate::triage::TriageOptions {
                dry_run: false,
                journal: Some(&journal),
                cancel,
                progress: on,
                conn: Some(conn),
            };
            match crate::triage::bucket_other(cfg, &opts) {
                Ok(r) if r.moved_files > 0 || !r.failures.is_empty() => {
                    tracing::info!("chia nhỏ Khác: {} file chuyển, {} lỗi", r.moved_files, r.failures.len());
                    for f in &r.failures {
                        tracing::warn!("chia nhỏ Khác: {f}");
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("chia nhỏ Khác lỗi: {e}"),
            }
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
    // Thư mục bị chặn còn sách trên đĩa (lần chặn trước dừng giữa chừng, hoặc thư mục được tạo lại bằng tay): dọn tiếp.
    if !summary.cancelled && !cfg.read_only && !cfg.blocked_categories.is_empty() {
        let r = crate::blocked::enforce(conn, cfg, trasher.unwrap_or(&crate::trash::SystemTrasher), None, cancel, on)?;
        if r.trashed > 0 || !r.failures.is_empty() {
            tracing::info!("thể loại bị chặn: {} sách vào Thùng rác, {} lỗi", r.trashed, r.failures.len());
        }
        for f in &r.failures {
            tracing::warn!("thể loại bị chặn: {}: {}", f.path, f.message);
        }
        auto_blocked += r.trashed;
        if r.cancelled {
            st.scan.cancelled = true;
        }
    }
    if auto_blocked > 0 || !cfg.blocked_categories.is_empty() {
        st.auto_blocked = Some(auto_blocked);
    }
    // Thư mục con chép theo từ nguồn (không do app tạo): đưa sách ra thư mục thể loại, dọn thư mục đã hết sách.
    if !st.scan.cancelled && cfg.auto_triage && cfg.flatten_subfolders && !cfg.read_only && !is_cancelled() {
        match crate::subcats::flatten(conn, cfg, trasher.unwrap_or(&crate::trash::SystemTrasher), cancel, on) {
            Ok(r) => {
                if r.moved > 0 || r.dirs_trashed > 0 || !r.failures.is_empty() {
                    tracing::info!(
                        "làm phẳng thư mục con: {} file ra thư mục thể loại, {} thư mục vào Thùng rác, {} lỗi",
                        r.moved,
                        r.dirs_trashed,
                        r.failures.len()
                    );
                }
                for f in r.failures.iter().take(50) {
                    tracing::warn!("làm phẳng thư mục con: {f}");
                }
                st.auto_flattened = Some(r.moved);
                if r.cancelled {
                    st.scan.cancelled = true;
                }
            }
            Err(e) => tracing::warn!("làm phẳng thư mục con lỗi: {e}"),
        }
    }
    // Sách mới xuất hiện ở chỗ khác trong thư viện (chép thẳng vào một thư mục thể loại, hoặc tự phân loại chưa
    // biết tag vì không có metadata.opf): xếp theo ánh xạ tag → thể loại của người dùng.
    if !st.scan.cancelled && cfg.auto_triage && !cfg.read_only && !cfg.tag_map.is_empty() {
        let opts = crate::tagmap::PlanOptions { first_seen_scan: Some(summary.scan_id), ..Default::default() };
        match crate::tagmap::plan(conn, cfg, &opts).and_then(|p| crate::tagmap::apply(conn, cfg, &p)) {
            Ok(r) => {
                if !r.moved.is_empty() || !r.failures.is_empty() {
                    tracing::info!("ánh xạ tag: {} sách mới chuyển, {} lỗi", r.moved.len(), r.failures.len());
                }
                for f in &r.failures {
                    tracing::warn!("ánh xạ tag: {}: {}", f.path, f.message);
                }
                if !r.moved.is_empty() {
                    st.auto_triaged = Some(st.auto_triaged.unwrap_or(0) + r.moved.len());
                }
            }
            Err(e) => tracing::warn!("ánh xạ tag lỗi: {e}"),
        }
    }
    // Sách đã xoá trước đây mà lại được tải về (nội dung y hệt): đưa thẳng vào Thùng rác, trước khi băm / tìm trùng.
    if !st.scan.cancelled && cfg.auto_reject_deleted && !cfg.read_only {
        let r = crate::deleted::reject_known(conn, cfg, trasher.unwrap_or(&crate::trash::SystemTrasher), cancel, on)?;
        if r.trash.trashed > 0 || !r.trash.failures.is_empty() {
            tracing::info!(
                "loại sách đã xoá trước đây: {} vào Thùng rác, {} lỗi",
                r.trash.trashed,
                r.trash.failures.len()
            );
        }
        for f in &r.trash.failures {
            tracing::warn!("loại sách đã xoá: {}: {}", f.path, f.message);
        }
        st.auto_rejected = Some(r.trash.trashed);
        if r.cancelled {
            st.scan.cancelled = true;
        }
    }
    if !st.scan.cancelled {
        let hs = hash::ensure_hashes_with(conn, cfg.hash_threads, cancel, on)?;
        st.hash_errors = hs.errors.len();
        if hs.cancelled || is_cancelled() {
            st.scan.cancelled = true;
        }
    }
    'analyze: {
        if st.scan.cancelled {
            break 'analyze;
        }
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
        step(2);
        // Có thể phải băm bổ sung nhiều file: bước con riêng có x / y (`suspect_hash`), huỷ được, lưu theo lô.
        match dedupe::suspect_groups_with(conn, cfg.hash_threads, cancel, on)? {
            Some(g) => st.suspect_groups = g.len(),
            None => {
                st.scan.cancelled = true;
                break 'analyze;
            }
        }
        step(3);
        if cfg.library_online() {
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
    if cfg.library_online() {
        let props = classify::propose(conn, cfg)?;
        st.inbox_books = props.len();
        st.inbox_proposed = props.iter().filter(|p| p.target.is_some()).count();
    }
    conn.execute("UPDATE scan_reports SET stats = ?2 WHERE id = ?1", params![rep.id, serde_json::to_string(&st)?])?;
    Ok(())
}

/// Sau một lô di chuyển: chuyển chỗ không đổi nội dung file, nên số trùng / nghi trùng giữ nguyên – chỉ số "chờ phân
/// loại" có thể đổi, và chỉ khi lô có sách ra / vào hộp thư đến. Rẻ hơn hẳn [`refresh_counts`] (với thư viện vài
/// chục nghìn cuốn, tính lại nghi trùng + đề xuất mất hàng trăm ms tới vài giây sau mỗi lần kéo thả).
pub fn refresh_after_moves(conn: &Connection, cfg: &Config, moved: &[crate::moves::MoveDone]) -> Result<()> {
    let inboxes: Vec<std::path::PathBuf> = cfg.library_roots().into_iter().map(|r| r.join(&cfg.inbox_name)).collect();
    let touches = |p: &str| {
        let f = crate::normalize::fold(p);
        inboxes.iter().any(|i| f.starts_with(&crate::normalize::fold(&i.to_string_lossy())))
    };
    if !moved.iter().any(|m| touches(&m.from) || touches(&m.to)) {
        return Ok(());
    }
    refresh_inbox_counts(conn, cfg)
}

/// Chỉ tính lại số sách chờ phân loại (hộp thư đến) trong báo cáo gần nhất.
pub fn refresh_inbox_counts(conn: &Connection, cfg: &Config) -> Result<()> {
    if !cfg.library_online() {
        return Ok(());
    }
    let props = classify::propose(conn, cfg)?;
    let (n, p) = (props.len(), props.iter().filter(|p| p.target.is_some()).count());
    update_stats(conn, |st| {
        let changed = st.inbox_books != n || st.inbox_proposed != p;
        st.inbox_books = n;
        st.inbox_proposed = p;
        changed
    })
    .map(|_| ())
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

/// Dòng banner khi mở app (E9-01), vd. "Từ lần quét gần nhất: 12 nghi trùng, 4 trùng chắc chắn, 9 sách mới chờ phân loại".
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
    if let Some(n) = s.auto_rejected.filter(|&n| n > 0) {
        parts.push(format!("{n} sách đã xoá trước đây bị tải lại – đã tự đưa vào Thùng rác"));
    }
    if let Some(n) = s.auto_blocked.filter(|&n| n > 0) {
        parts.push(format!("{n} sách thuộc thể loại bị chặn – đã tự đưa vào Thùng rác"));
    }
    let mut out = String::new();
    // Báo cáo cũ có thể là "background" (tác vụ quét ngầm, nay đã bỏ).
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
