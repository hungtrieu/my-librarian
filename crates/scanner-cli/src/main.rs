//! `librarian` – CLI cho lõi My Librarian (dùng để thử trên kho thật trước khi có giao diện).
//! Mọi thao tác xoá/di chuyển đều xem trước; chỉ thực hiện khi có `--yes`.

use clap::{Parser, Subcommand};
use librarian_core::classify::{self, Bucket};
use librarian_core::dedupe::{self, Decision};
use librarian_core::moves::{self, MoveRequest};
use librarian_core::progress::{self, Watchdog};
use librarian_core::report::{self, human_bytes};
use librarian_core::scan::{self, ScanOptions, ScanProgress};
use librarian_core::trash::{self, SystemTrasher};
use librarian_core::{config, db, hash, lock, search, testdata, Library, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "librarian", version, about = "My Librarian – quản lý kho ebook")]
struct Cli {
    /// Thư mục dữ liệu app (mặc định %APPDATA%\my-librarian).
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    /// Chế độ chỉ đọc cho lần chạy này: không xoá/di chuyển file nào.
    #[arg(long, global = true)]
    read_only: bool,
    /// In kết quả dạng JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Đặt thư mục thư viện `books`.
    Init { books: PathBuf },
    /// Xem/sửa cấu hình.
    Config {
        #[arg(long)]
        read_only: Option<bool>,
        /// Tự phân loại hộp thư đến mỗi lần quét (xem `triage`).
        #[arg(long)]
        auto_triage: Option<bool>,
        #[arg(long)]
        threads: Option<usize>,
        #[arg(long)]
        inbox: Option<String>,
        #[arg(long)]
        add_source: Option<PathBuf>,
        #[arg(long)]
        high: Option<u8>,
        #[arg(long)]
        medium: Option<u8>,
    },
    /// Quét thư viện (tăng dần). Ctrl+C để huỷ, lần sau quét tiếp.
    Scan,
    /// Quét + băm + tìm trùng + đề xuất phân loại (như tác vụ ngầm, chỉ đọc).
    Analyze,
    /// Tìm sách theo tên/tác giả/tên file (không dấu, tiền tố).
    Search {
        query: Vec<String>,
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },
    /// Liệt kê nhóm trùng chắc chắn; `--yes` để dọn vào Thùng rác.
    Dupes {
        #[arg(long)]
        yes: bool,
        /// Đổi bản giữ: HASH_PREFIX=BOOK_ID (lặp lại được).
        #[arg(long = "keep")]
        keep: Vec<String>,
    },
    /// Liệt kê nhóm nghi trùng chờ duyệt.
    Suspects,
    /// Ghi quyết định cho một nhóm nghi trùng.
    Decide {
        ids: Vec<i64>,
        #[arg(long, conflicts_with = "keep_both")]
        not_dup: bool,
        #[arg(long)]
        keep_both: bool,
    },
    /// Xoá sách vào Thùng rác Windows.
    Trash {
        ids: Vec<i64>,
        #[arg(long)]
        yes: bool,
    },
    /// Xem trước phân loại sách trong hộp thư đến; `--yes` để áp dụng các đề xuất được tick.
    Classify {
        #[arg(long)]
        yes: bool,
        /// Áp dụng cả đề xuất độ tin cậy trung bình.
        #[arg(long)]
        include_medium: bool,
    },
    /// Tự phân loại cả hộp thư đến (đọc metadata.opf, làm phẳng, tạo thư mục thể loại, chưa chắc → Khác,
    /// dọn file/thư mục thừa vào Thùng rác). Mặc định chỉ xem trước; `--yes` để thực hiện.
    Triage {
        #[arg(long)]
        yes: bool,
    },
    /// Phân loại tay: chuyển sách vào một thư mục có sẵn.
    Move {
        id: i64,
        dir: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Hoàn tác một lô di chuyển.
    Undo { batch: i64 },
    /// Nhật ký thao tác (hoặc chi tiết một lô).
    Activity { batch: Option<i64> },
    /// Danh sách bộ truyện.
    Series,
    /// Danh mục (thư mục thể loại) và số sách.
    Categories,
    /// Đo độ chính xác phân loại trên chính kho (leave-one-out).
    Evaluate,
    /// Báo cáo quét gần nhất / lịch sử.
    Report {
        #[arg(long, default_value_t = 1)]
        history: usize,
    },
    /// Sao lưu DB (giữ 7 bản).
    Backup,
    /// Sinh thư viện mẫu để thử.
    GenSample { dir: PathBuf },
    /// Đo hiệu năng trên kho tổng hợp trong thư mục tạm (không đụng kho thật) – E10-03.
    Bench {
        /// Số file sách cần sinh.
        #[arg(long, default_value_t = 20_000)]
        books: usize,
        #[arg(long, default_value_t = 3)]
        threads: usize,
        /// Thư mục làm việc (mặc định: thư mục tạm của Windows).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Giữ lại kho tổng hợp sau khi đo.
        #[arg(long)]
        keep: bool,
    },
}

fn print_json<T: serde::Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).unwrap());
}

fn short(h: &str) -> &str {
    &h[..h.len().min(10)]
}

fn main() {
    let cli = Cli::parse();
    let home = cli.home.clone().unwrap_or_else(config::default_home);
    let _guard = librarian_core::logging::init(&home, true);
    if let Err(e) = run(cli, home) {
        eprintln!("Lỗi: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli, home: PathBuf) -> Result<()> {
    if let Cmd::Bench { books, threads, dir, keep } = &cli.cmd {
        return bench(*books, *threads, dir.clone(), *keep, cli.json);
    }
    if let Cmd::GenSample { dir } = &cli.cmd {
        let s = testdata::generate_sample(dir)?;
        println!("Đã tạo thư viện mẫu: {} ({} file sách)", s.books.display(), s.book_files);
        println!("Tiếp theo: librarian init \"{}\"", s.books.display());
        return Ok(());
    }
    let mut lib = Library::open_recovering(&home)?;
    if let Some(n) = &lib.notice {
        eprintln!("⚠ {n}");
    }
    if cli.read_only {
        lib.config.read_only = true;
    }
    let json = cli.json;
    match cli.cmd {
        Cmd::GenSample { .. } | Cmd::Bench { .. } => unreachable!(),
        Cmd::Init { books } => {
            lib.set_library_root(&books)?;
            println!("Thư viện: {}", books.display());
            println!("Hộp thư đến: {}", lib.config.inbox_dir().unwrap().display());
        }
        Cmd::Config { read_only, auto_triage, threads, inbox, add_source, high, medium } => {
            let mut changed = false;
            if let Some(v) = read_only {
                lib.config.read_only = v;
                changed = true;
            }
            if let Some(v) = auto_triage {
                lib.config.auto_triage = v;
                changed = true;
            }
            if let Some(v) = threads {
                lib.config.hash_threads = v;
                changed = true;
            }
            if let Some(v) = inbox {
                lib.config.inbox_name = v;
                changed = true;
            }
            if let Some(v) = add_source {
                lib.config.extra_roots.push(v);
                changed = true;
            }
            if let Some(v) = high {
                lib.config.classify_high = v;
                changed = true;
            }
            if let Some(v) = medium {
                lib.config.classify_medium = v;
                changed = true;
            }
            if changed {
                if cli.read_only {
                    lib.config.read_only = read_only.unwrap_or(false);
                }
                lib.save_config()?;
            }
            println!("# {}", librarian_core::Config::path(&lib.home).display());
            print_json(&lib.config);
        }
        Cmd::Scan => {
            let _lock = lock::ScanLock::try_acquire(&lib.home)?;
            let cancel = Arc::new(AtomicBool::new(false));
            let c2 = cancel.clone();
            let _ = ctrlc_hook(move || c2.store(true, Ordering::Relaxed));
            let wd = Watchdog::start(cancel.clone(), progress::STALL_LIMIT, |_| {});
            let on_progress = |p: &ScanProgress| {
                let s = librarian_core::Step { phase: p.phase, done: p.files_seen, total: p.total };
                wd.observe(s);
                print_step(s);
            };
            let res = scan::scan(
                &lib.conn,
                &lib.config,
                &ScanOptions { cancel: Some(&cancel), progress: Some(&on_progress), kind: Some("app") },
            );
            eprintln!();
            if let Some(stall) = wd.finish() {
                return Err(librarian_core::Error::msg(stall.to_string()));
            }
            let s = res?;
            if json {
                print_json(&s);
                return Ok(());
            }
            println!(
                "Quét xong: {} file · mới {} · đổi {} · không đổi {} · mất {} · nhận lại (đổi tên) {}{}",
                s.files_seen,
                s.added,
                s.updated,
                s.unchanged,
                s.missing,
                s.recovered,
                if s.cancelled { " · ĐÃ HUỶ (chạy lại để quét tiếp)" } else { "" }
            );
            for r in &s.skipped_roots {
                println!("  Bỏ qua – ổ chưa kết nối: {r}");
            }
            for e in s.errors.iter().take(20) {
                println!("  Lỗi: {} – {}", e.path, e.message);
            }
        }
        Cmd::Analyze => {
            let _lock = lock::ScanLock::try_acquire(&lib.home)?;
            let cancel = Arc::new(AtomicBool::new(false));
            let c2 = cancel.clone();
            let _ = ctrlc_hook(move || c2.store(true, Ordering::Relaxed));
            let wd = Watchdog::start(cancel.clone(), progress::STALL_LIMIT, |_| {});
            let on = |s: librarian_core::Step| {
                wd.observe(s);
                print_step(s);
            };
            let res = report::run_analysis_with(&lib.conn, &lib.config, "app", Some(&cancel), None, Some(&on));
            eprintln!();
            if let Some(stall) = wd.finish() {
                return Err(librarian_core::Error::msg(stall.to_string()));
            }
            let st = res?;
            eprintln!();
            if json {
                print_json(&st);
                return Ok(());
            }
            println!("{} sách · {}", st.total_books, human_bytes(st.total_bytes));
            println!(
                "Trùng chắc chắn: {} nhóm, {} bản thừa ({} thu hồi được)",
                st.exact_groups,
                st.exact_files,
                human_bytes(st.reclaimable_bytes)
            );
            println!("Nghi trùng chờ duyệt: {} nhóm", st.suspect_groups);
            println!("Hộp thư đến: {} sách, {} có đề xuất thư mục", st.inbox_books, st.inbox_proposed);
            println!("Sách mất: {}", st.missing_books);
            if let Some(r) = report::latest(&lib.conn)? {
                if let Some(b) = report::banner(&r) {
                    println!("\n{b}");
                }
            }
        }
        Cmd::Search { query, limit } => {
            let q = query.join(" ");
            let t = std::time::Instant::now();
            let r = search::search(&lib.conn, &q, limit)?;
            if json {
                print_json(&r);
                return Ok(());
            }
            for b in &r {
                println!(
                    "#{:<6} {} — {} [{}] {}",
                    b.id,
                    b.title,
                    if b.authors.is_empty() { "?" } else { &b.authors },
                    b.format,
                    b.rel_path
                );
            }
            println!("{} kết quả trong {:.1} ms", r.len(), t.elapsed().as_secs_f64() * 1000.0);
        }
        Cmd::Dupes { yes, keep } => {
            hash::ensure_hashes(&lib.conn, lib.config.hash_threads, None)?;
            let groups = dedupe::exact_groups(&lib.conn)?;
            let mut overrides = HashMap::new();
            for k in keep {
                let (h, id) = k.split_once('=').ok_or_else(|| librarian_core::Error::msg("--keep HASH=ID"))?;
                let id: i64 = id.parse().map_err(|_| librarian_core::Error::msg("ID không hợp lệ"))?;
                if let Some(g) = groups.iter().find(|g| g.full_hash.starts_with(h)) {
                    overrides.insert(g.full_hash.clone(), id);
                }
            }
            if json && !yes {
                print_json(&groups);
                return Ok(());
            }
            let total: i64 = groups.iter().map(|g| g.reclaimable).sum();
            for g in &groups {
                println!(
                    "[{}] {} · giữ #{} {} ({})",
                    short(&g.full_hash),
                    human_bytes(g.size),
                    g.keeper.id,
                    g.keeper.path,
                    g.keep_reason
                );
                for o in &g.others {
                    println!("      bỏ #{} {}", o.id, o.path);
                }
            }
            println!("{} nhóm, thu hồi {}", groups.len(), human_bytes(total));
            if groups.is_empty() {
                return Ok(());
            }
            if !yes {
                println!("Chạy lại với --yes để dọn tất cả vào Thùng rác.");
                return Ok(());
            }
            let rep = dedupe::clean_exact(&lib.conn, &lib.config, &SystemTrasher, &groups, &overrides)?;
            print_trash(&rep);
        }
        Cmd::Suspects => {
            let groups = dedupe::suspect_groups(&lib.conn)?;
            if json {
                print_json(&groups);
                return Ok(());
            }
            for g in &groups {
                println!("● {}", g.reason);
                for b in &g.members {
                    println!("    #{:<6} {:<5} {:>10}  {}", b.id, b.format, human_bytes(b.size), b.rel_path);
                }
            }
            println!("{} nhóm nghi trùng. Duyệt: librarian decide <id...> --not-dup | --keep-both, hoặc librarian trash <id> --yes", groups.len());
        }
        Cmd::Decide { ids, not_dup, keep_both } => {
            let d = if keep_both {
                Decision::KeepBoth
            } else if not_dup {
                Decision::NotDuplicate
            } else {
                return Err(librarian_core::Error::msg("Chọn --not-dup hoặc --keep-both"));
            };
            let n = dedupe::decide(&lib.conn, &ids, d)?;
            println!("Đã ghi {n} quyết định – nhóm này sẽ không hiện lại.");
        }
        Cmd::Trash { ids, yes } => {
            let books: Vec<db::Book> =
                ids.iter().filter_map(|id| db::get_book(&lib.conn, *id).ok().flatten()).collect();
            let total: i64 = books.iter().map(|b| b.size).sum();
            for b in &books {
                println!("  #{} {}", b.id, b.path);
            }
            println!("{} sách, {}", books.len(), human_bytes(total));
            if !yes {
                println!("Chạy lại với --yes để đưa vào Thùng rác.");
                return Ok(());
            }
            let rep = trash::trash_books(&lib.conn, &lib.config, &SystemTrasher, &ids)?;
            print_trash(&rep);
        }
        Cmd::Classify { yes, include_medium } => {
            let props = classify::propose(&lib.conn, &lib.config)?;
            if json && !yes {
                print_json(&props);
                return Ok(());
            }
            for bucket in [Bucket::High, Bucket::Medium, Bucket::Manual] {
                let items: Vec<_> = props.iter().filter(|p| p.bucket == bucket).collect();
                if items.is_empty() {
                    continue;
                }
                println!(
                    "\n== {} ({}) ==",
                    match bucket {
                        Bucket::High => "Tin cậy cao – sẽ áp dụng",
                        Bucket::Medium =>
                            if include_medium {
                                "Trung bình – sẽ áp dụng"
                            } else {
                                "Trung bình – chưa tick (--include-medium)"
                            },
                        Bucket::Manual => "Cần phân loại tay",
                    },
                    items.len()
                );
                for p in items {
                    let to = p.target_label.clone().unwrap_or_else(|| "—".into());
                    let extra = p.suggestion.as_ref().map(|s| format!(" · gợi ý: {s}")).unwrap_or_default();
                    println!(
                        "  #{:<6} {:<45} → {:<28} {:>3}%  {}{}",
                        p.book.id, p.book.file_name, to, p.confidence, p.reason, extra
                    );
                }
            }
            let reqs: Vec<MoveRequest> = props
                .iter()
                .filter(|p| p.preselected || (include_medium && p.bucket == Bucket::Medium))
                .filter_map(|p| Some(MoveRequest { book_id: p.book.id, target_dir: p.target.clone()? }))
                .collect();
            if reqs.is_empty() {
                println!("\nKhông có gì để áp dụng.");
                return Ok(());
            }
            if !yes {
                println!(
                    "\n{} file sẽ được di chuyển. Chạy lại với --yes để áp dụng (hoàn tác được bằng `librarian undo`).",
                    reqs.len()
                );
                return Ok(());
            }
            let rep = moves::apply_moves(&lib.conn, &lib.config, &reqs, "classify")?;
            print_moves(&rep);
        }
        Cmd::Triage { yes } => {
            let journal = lib.home.join("triage-journal.jsonl");
            let opts = librarian_core::triage::TriageOptions {
                dry_run: !yes,
                journal: Some(&journal),
                progress: Some(&print_step),
                conn: Some(&lib.conn),
                ..Default::default()
            };
            let r = librarian_core::triage::run(&lib.config, &SystemTrasher, &opts)?;
            eprintln!();
            if json {
                print_json(&r);
            } else {
                println!(
                    "{} tác phẩm, {} file sách{}",
                    r.units,
                    r.moved_files,
                    if yes { " đã chuyển" } else { " sẽ chuyển" }
                );
                for (c, n) in &r.by_category {
                    println!("  {n:>6}  {c}");
                }
                if !r.created_dirs.is_empty() {
                    println!(
                        "Thư mục thể loại {}: {}",
                        if yes { "đã tạo" } else { "sẽ tạo" },
                        r.created_dirs.join(", ")
                    );
                }
                println!(
                    "Bản trùng y hệt vào Thùng rác: {}; thư mục thừa: {}; file rác lẻ: {}; lỗi: {}",
                    r.duplicates_trashed,
                    r.dirs_trashed,
                    r.junk_files_trashed,
                    r.failures.len()
                );
                for f in &r.failures {
                    eprintln!("  lỗi: {f}");
                }
                if !yes {
                    println!("Chạy lại với --yes để thực hiện (nhật ký: {}).", journal.display());
                }
            }
        }
        Cmd::Move { id, dir, yes } => {
            let b = db::get_book(&lib.conn, id)?.ok_or_else(|| librarian_core::Error::msg("Không có sách này"))?;
            println!("{} → {}", b.path, dir.display());
            if !yes {
                println!("Chạy lại với --yes để di chuyển.");
                return Ok(());
            }
            let rep =
                moves::apply_moves(&lib.conn, &lib.config, &[MoveRequest { book_id: id, target_dir: dir }], "manual")?;
            print_moves(&rep);
        }
        Cmd::Undo { batch } => {
            let r = moves::undo_batch(&lib.conn, &lib.config, batch)?;
            println!("Đã trả {} file về chỗ cũ.", r.restored);
            for f in &r.failures {
                println!("  Không hoàn tác được #{} {}: {}", f.book_id, f.path, f.message);
            }
        }
        Cmd::Activity { batch } => match batch {
            Some(b) => {
                let ops = moves::batch_operations(&lib.conn, b)?;
                if json {
                    print_json(&ops);
                    return Ok(());
                }
                for o in ops {
                    println!(
                        "{:<7} {:<11} {} {}{}",
                        o.kind,
                        o.status,
                        o.src,
                        o.dst.map(|d| format!("→ {d}")).unwrap_or_default(),
                        o.message.map(|m| format!("  ({m})")).unwrap_or_default()
                    );
                }
            }
            None => {
                let v = moves::list_batches(&lib.conn, 30)?;
                if json {
                    print_json(&v);
                    return Ok(());
                }
                for b in v {
                    println!(
                        "lô #{:<5} {:<9} {} · {} xong, {} lỗi · {}{}",
                        b.id,
                        b.kind,
                        fmt_time(b.created_at),
                        b.done,
                        b.failed,
                        human_bytes(b.bytes),
                        if b.undone_at.is_some() { " · đã hoàn tác" } else { "" }
                    );
                }
                println!("Mở Thùng rác: explorer shell:RecycleBinFolder");
            }
        },
        Cmd::Series => {
            let v = search::list_series(&lib.conn)?;
            if json {
                print_json(&v);
                return Ok(());
            }
            for s in v {
                println!(
                    "{:<40} {:>4} vol · đã đọc {}/{} · [{}]",
                    s.series,
                    s.volumes,
                    s.read,
                    s.volumes,
                    s.category.unwrap_or_default()
                );
            }
        }
        Cmd::Categories => {
            let cats = classify::list_categories(&lib.config)?;
            let counts = search::category_counts(&lib.conn)?;
            for c in cats {
                let n = counts.iter().find(|x| librarian_core::normalize::fold(&x.category) == c.key);
                println!(
                    "{:<35} {:>6} sách {:>10}  ({} thư mục bộ)",
                    c.name,
                    n.map_or(0, |x| x.books),
                    human_bytes(n.map_or(0, |x| x.bytes)),
                    c.series_dirs.len()
                );
            }
        }
        Cmd::Evaluate => {
            let ev = classify::evaluate(&lib.conn, &lib.config)?;
            if json {
                print_json(&ev);
                return Ok(());
            }
            let pct = |a: usize, b: usize| if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 };
            println!("Leave-one-out trên {} sách đã có thư mục:", ev.total);
            println!("  Đúng tổng thể: {:.1}%", 100.0 * ev.accuracy());
            println!(
                "  Tin cậy cao:   {}/{} đúng ({:.1}%)",
                ev.high_correct,
                ev.high_total,
                pct(ev.high_correct, ev.high_total)
            );
            println!(
                "  Trung bình:    {}/{} đúng ({:.1}%)",
                ev.medium_correct,
                ev.medium_total,
                pct(ev.medium_correct, ev.medium_total)
            );
            println!("  Sẽ để phân loại tay: {}", ev.manual);
        }
        Cmd::Report { history } => {
            let v = report::history(&lib.conn, history)?;
            if json {
                print_json(&v);
                return Ok(());
            }
            for r in v {
                println!("#{} {} {} [{}]", r.id, r.kind, fmt_time(r.started_at), r.status);
                if let Some(b) = report::banner(&r) {
                    println!("  {b}");
                }
            }
        }
        Cmd::Backup => {
            let p = db::backup(&lib.conn, &lib.home.join("backups"), 7)?;
            println!("Đã sao lưu: {}", p.display());
        }
    }
    Ok(())
}

fn bench(n: usize, threads: usize, dir: Option<PathBuf>, keep: bool, json: bool) -> Result<()> {
    use librarian_core::bench;
    let work = dir.unwrap_or_else(std::env::temp_dir).join(format!("my-librarian-bench-{}", db::now()));
    if work.exists() {
        return Err(librarian_core::Error::msg(format!("{} đã tồn tại", work.display())));
    }
    eprintln!("Kho tổng hợp {n} sách ở {}", work.display());
    let res = bench::run(&work, n, threads, &|step| eprintln!("  … {step}"));
    if !keep {
        let _ = std::fs::remove_dir_all(&work);
    }
    let r = res?;
    if json {
        print_json(&r);
        return Ok(());
    }
    println!("Kho: {} sách, {} · DB {}", r.books, human_bytes(r.bytes as i64), human_bytes(r.db_bytes as i64));
    println!("{:<46} {:>10} {:>12}  ghi chú", "Bước", "ms", "ngưỡng");
    for m in &r.metrics {
        let limit = m.limit_ms.map(|l| format!("≤ {l:.0}")).unwrap_or_default();
        let mark = if m.ok() { " " } else { "✗" };
        println!("{mark}{:<45} {:>10.1} {:>12}  {}", m.name, m.ms, limit, m.note);
    }
    let bad = r.metrics.iter().filter(|m| !m.ok()).count();
    if bad > 0 {
        println!("{bad} bước vượt ngưỡng.");
        std::process::exit(3);
    }
    Ok(())
}

fn print_trash(rep: &trash::TrashReport) {
    println!(
        "Đã đưa {} file vào Thùng rác, giải phóng {} (lô #{}).",
        rep.trashed,
        human_bytes(rep.freed_bytes as i64),
        rep.batch_id
    );
    for f in &rep.failures {
        println!("  Lỗi #{} {}: {}", f.book_id, f.path, f.message);
    }
    if let Some(a) = &rep.aborted {
        println!("ĐÃ DỪNG: {a}");
    }
}

fn print_moves(rep: &moves::MoveReport) {
    println!("Đã di chuyển {} file (lô #{}). Hoàn tác: librarian undo {}", rep.moved.len(), rep.batch_id, rep.batch_id);
    for f in &rep.failures {
        println!("  Lỗi #{} {}: {}", f.book_id, f.path, f.message);
    }
}

fn fmt_time(t: i64) -> String {
    // UTC, đủ cho CLI; giao diện sẽ hiển thị giờ địa phương.
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", secs / 3600, secs % 3600 / 60)
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Ctrl+C → huỷ quét êm (lần sau quét tiếp từ dữ liệu đã lưu).
fn ctrlc_hook(f: impl Fn() + Send + 'static) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::sync::OnceLock;
        static HOOK: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
        let f = std::sync::Mutex::new(f);
        let _ = HOOK.set(Box::new(move || (f.lock().unwrap())()));
        unsafe extern "system" fn handler(_ctrl: u32) -> i32 {
            if let Some(h) = HOOK.get() {
                h();
            }
            1
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetConsoleCtrlHandler(h: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
        }
        unsafe {
            SetConsoleCtrlHandler(Some(handler), 1);
        }
    }
    #[cfg(not(windows))]
    let _ = f;
    Ok(())
}

/// In tiến trình một dòng (ghi đè) ra stderr để không lẫn với đầu ra `--json`.
fn print_step(s: librarian_core::Step) {
    let label = librarian_core::progress::phase_label(s.phase);
    const CR: char = '\r';
    if s.total > 0 {
        eprint!("{CR}{label}: {}/{} ({}%)        ", s.done, s.total, s.done * 100 / s.total);
    } else {
        eprint!("{CR}{label}…                          ");
    }
}
