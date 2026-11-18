//! Giám sát tiến trình quét: phát hiện bước bị treo.
//!
//! Hai tín hiệu "còn sống":
//! - [`Step`] báo qua callback tiến trình (đổi bước hoặc `done` tăng);
//! - **nhịp** [`pulse`]: mọi vòng lặp làm việc nặng (đọc file, băm từng MB, chuyển/xoá file, ghi DB theo lô)
//!   gọi `pulse()` sau mỗi đơn vị việc, kể cả khi chưa đủ để tăng `done` (vd. đang băm một file 500 MB).
//!
//! [`Watchdog`] chạy một luồng riêng, đếm số giây không có tín hiệu nào. Quá [`STALL_LIMIT`] thì coi là treo:
//! ghi log lỗi (bước nào, x / y, file đang xử lý), bật cờ huỷ để luồng quét dừng ở chỗ kiểm tra kế tiếp, và
//! trả về [`Stall`] để nơi gọi báo lỗi cho người dùng.
//!
//! Quy ước cho code mới: vòng lặp nào có thể chạy lâu (I/O trên ổ chậm, hàng nghìn mục) phải gọi `pulse()` mỗi
//! vòng **và** kiểm tra cờ huỷ, và mỗi bước mới phải báo `Step` với `phase` riêng ngay khi bắt đầu
//! (test `every_phase_is_reported` trong `tests/integration.rs` chặn bước không tên).

use crate::Step;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Đứng yên quá lâu thì dừng quét và báo lỗi.
pub const STALL_LIMIT: Duration = Duration::from_secs(60);

/// Hai lần đọc nhịp liên tiếp của bộ giám sát (bình thường cách nhau ≤ 200 ms) mà cách nhau quá khoảng này thì
/// chính luồng giám sát đã bị đóng băng – máy ngủ / Modern Standby (Windows treo cả tiến trình desktop, còn
/// `Instant` vẫn đếm thời gian ngủ), hoặc cả tiến trình bị dừng. Khi đó luồng quét cũng đứng cùng lúc, không phải
/// lỗi của nó: khoảng đóng băng không tính vào thời gian đứng yên. (Lần 13/11/2026: máy vào Modern Standby
/// 23 phút giữa bước phân loại, vừa thức dậy thì báo "treo 1408 giây" và huỷ một lần quét đang chạy tốt.)
pub const SUSPEND_GAP: Duration = Duration::from_secs(5);

static PULSE: AtomicU64 = AtomicU64::new(0);

/// Báo "vẫn đang làm việc" (rẻ: một phép cộng nguyên tử). Gọi được từ mọi luồng.
#[inline]
pub fn pulse() {
    PULSE.fetch_add(1, Ordering::Relaxed);
}

/// Tổng số nhịp từ lúc chạy tiến trình.
pub fn pulses() -> u64 {
    PULSE.load(Ordering::Relaxed)
}

static CURRENT: Mutex<Option<String>> = Mutex::new(None);

/// Ghi lại mục (file / thư mục) sắp xử lý, kèm một nhịp – để khi treo biết chính xác treo ở đâu.
pub fn working_on(item: &std::path::Path) {
    pulse();
    if let Ok(mut g) = CURRENT.lock() {
        *g = Some(item.to_string_lossy().into_owned());
    }
}

/// Mục ghi lại gần nhất bởi [`working_on`].
pub fn current_item() -> Option<String> {
    CURRENT.lock().ok().and_then(|g| g.clone())
}

/// Tên bước để hiện cho người dùng / ghi log.
pub fn phase_label(phase: &str) -> &'static str {
    match phase {
        "count" => "Liệt kê file",
        "scan" => "Quét file",
        "recover" => "Nhận lại sách chuyển chỗ",
        "triage" => "Phân loại hộp thư đến",
        "triage_read" => "Đọc hộp thư đến",
        "triage_clean" => "Dọn hộp thư đến",
        "triage_other" => "Chia nhỏ thư mục Khác",
        "hash_quick" => "Băm nhanh",
        "hash_full" => "Băm đầy đủ",
        "suspect_hash" => "Băm bổ sung cho nghi trùng",
        "reject_deleted" => "Loại sách đã xoá trước đây",
        "block" => "Xoá sách thuộc thể loại bị chặn",
        "block_clean" => "Dọn thư mục bị chặn",
        "flatten" => "Đưa sách ra khỏi thư mục con lạ",
        "flatten_clean" => "Dọn thư mục con đã hết sách",
        "analyze" => "Phân tích trùng & thể loại",
        "trash" => "Đưa file vào Thùng rác",
        "refresh" => "Cập nhật số liệu",
        "start" => "Bắt đầu",
        _ => "Bước không rõ",
    }
}

/// Trạng thái theo dõi (thuần, không luồng – để test được).
#[derive(Debug, Clone)]
pub struct Watch {
    pub phase: &'static str,
    pub done: usize,
    pub total: usize,
    /// File / mục đang xử lý (nếu bước có báo).
    pub detail: Option<String>,
    pulse: u64,
    since: Instant,
    /// Lần [`Watch::tick`] gần nhất – để nhận ra khoảng bộ giám sát bị đóng băng.
    last_tick: Instant,
    /// Tổng thời gian đóng băng đã bỏ qua (ghi log / test).
    pub suspended: Duration,
}

impl Watch {
    pub fn new(now: Instant) -> Self {
        if let Ok(mut g) = CURRENT.lock() {
            *g = None;
        }
        Watch {
            phase: "start",
            done: 0,
            total: 0,
            detail: None,
            pulse: pulses(),
            since: now,
            last_tick: now,
            suspended: Duration::ZERO,
        }
    }
    /// Ghi nhận một `Step`; chỉ tính là tiến triển khi đổi bước hoặc `done` đổi.
    pub fn step(&mut self, s: Step, now: Instant) {
        if s.phase != self.phase || s.done != self.done {
            self.since = now;
            if s.phase != self.phase {
                self.detail = None;
            }
        }
        self.phase = s.phase;
        self.done = s.done;
        self.total = s.total;
    }
    /// Đọc nhịp hiện tại; trả về thời gian đứng yên. Khoảng giữa hai lần gọi dài hơn [`SUSPEND_GAP`] là máy
    /// ngủ / tiến trình bị đóng băng (xem hằng số) → không tính vào thời gian đứng yên.
    pub fn tick(&mut self, pulse: u64, now: Instant) -> Duration {
        let gap = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;
        if gap > SUSPEND_GAP {
            self.suspended += gap;
            // dời mốc "từ lúc" qua khoảng đóng băng, giữ phần đứng yên thật trước đó
            self.since = (self.since + gap).min(now);
            tracing::info!("bộ giám sát quét bị đóng băng {}s (máy ngủ?) – không tính là treo", gap.as_secs());
        }
        if pulse != self.pulse {
            self.pulse = pulse;
            self.since = now;
        }
        now.saturating_duration_since(self.since)
    }
}

/// Tình trạng gửi ra giao diện mỗi giây.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Health {
    pub phase: &'static str,
    pub label: &'static str,
    pub done: usize,
    pub total: usize,
    /// Số giây không có tiến triển nào.
    pub idle_secs: u64,
    pub limit_secs: u64,
    /// Đã quá giới hạn → đang dừng quét.
    pub stalled: bool,
}

/// Một lần treo bị phát hiện.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Stall {
    pub phase: &'static str,
    pub done: usize,
    pub total: usize,
    pub idle_secs: u64,
    pub detail: Option<String>,
}

impl std::fmt::Display for Stall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bước “{}” không có tiến triển trong {} giây", phase_label(self.phase), self.idle_secs)?;
        if self.total > 0 {
            write!(f, " (đang ở {} / {})", self.done, self.total)?;
        }
        if let Some(d) = &self.detail {
            write!(f, ", mục đang xử lý: {d}")?;
        }
        write!(f, ". Đã dừng quét – phần đã làm được giữ lại.")
    }
}

/// Luồng giám sát. Tạo khi bắt đầu quét, đưa [`Watchdog::observe`] vào callback tiến trình,
/// gọi [`Watchdog::finish`] khi quét xong.
pub struct Watchdog {
    watch: Arc<Mutex<Watch>>,
    stall: Arc<Mutex<Option<Stall>>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Watchdog {
    /// `cancel`: cờ huỷ của tiến trình quét – bật lên khi phát hiện treo.
    /// `on_tick`: gọi mỗi giây với tình trạng hiện tại (vd. gửi lên giao diện).
    pub fn start(cancel: Arc<AtomicBool>, limit: Duration, on_tick: impl Fn(&Health) + Send + 'static) -> Watchdog {
        Self::start_with(cancel, limit, on_tick, |_| {})
    }

    /// Như [`Watchdog::start`], thêm `on_stall`: gọi **một lần, trên luồng giám sát**, ngay khi phát hiện treo.
    ///
    /// Bật cờ huỷ chỉ có tác dụng khi luồng quét tới chỗ kiểm tra kế tiếp; nếu nó kẹt trong một lệnh không bao giờ
    /// trả về (đọc ổ USB rớt kết nối, `IFileOperation`…) thì [`Watchdog::finish`] cũng không bao giờ được gọi.
    /// Nơi gọi dùng `on_stall` để báo lỗi và mở khoá giao diện ngay, không chờ luồng quét.
    pub fn start_with(
        cancel: Arc<AtomicBool>,
        limit: Duration,
        on_tick: impl Fn(&Health) + Send + 'static,
        on_stall: impl FnOnce(&Stall) + Send + 'static,
    ) -> Watchdog {
        let watch = Arc::new(Mutex::new(Watch::new(Instant::now())));
        let stall: Arc<Mutex<Option<Stall>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (w, s, st) = (watch.clone(), stall.clone(), stop.clone());
        let handle = std::thread::Builder::new()
            .name("scan-watchdog".into())
            .spawn(move || {
                let interval = Duration::from_millis(200).min(limit / 4).max(Duration::from_millis(10));
                let mut last_tick = Instant::now();
                let mut on_stall = Some(on_stall);
                while !st.load(Ordering::Relaxed) {
                    std::thread::sleep(interval);
                    let now = Instant::now();
                    let Ok(mut g) = w.lock() else { return };
                    let idle = g.tick(pulses(), now);
                    let mut stalled = s.lock().map(|x| x.is_some()).unwrap_or(false);
                    let mut found_now = None;
                    if !stalled && idle >= limit {
                        let found = Stall {
                            phase: g.phase,
                            done: g.done,
                            total: g.total,
                            idle_secs: idle.as_secs(),
                            detail: g.detail.clone().or_else(current_item),
                        };
                        tracing::error!("quét bị treo: {found}");
                        cancel.store(true, Ordering::Relaxed);
                        if let Ok(mut x) = s.lock() {
                            *x = Some(found.clone());
                        }
                        found_now = Some(found);
                        stalled = true;
                    }
                    if now.duration_since(last_tick) >= Duration::from_secs(1) || stalled {
                        last_tick = now;
                        on_tick(&Health {
                            phase: g.phase,
                            label: phase_label(g.phase),
                            done: g.done,
                            total: g.total,
                            idle_secs: idle.as_secs(),
                            limit_secs: limit.as_secs(),
                            stalled,
                        });
                    }
                    drop(g); // không giữ khoá khi gọi ra ngoài: `observe` của luồng quét không bị chặn
                    if let Some(found) = found_now {
                        if let Some(f) = on_stall.take() {
                            f(&found);
                        }
                    }
                }
            })
            .ok();
        Watchdog { watch, stall, stop, handle }
    }

    /// Ghi nhận tiến trình (gọi trong callback `OnStep`).
    pub fn observe(&self, s: Step) {
        if let Ok(mut g) = self.watch.lock() {
            g.step(s, Instant::now());
        }
    }

    /// Ghi nhận mục đang xử lý (để báo lỗi rõ file nào làm treo).
    pub fn detail(&self, d: impl Into<String>) {
        if let Ok(mut g) = self.watch.lock() {
            g.detail = Some(d.into());
        }
    }

    /// Đã phát hiện treo chưa.
    pub fn stalled(&self) -> Option<Stall> {
        self.stall.lock().ok().and_then(|g| g.clone())
    }

    /// Dừng luồng giám sát; trả về lần treo (nếu có).
    pub fn finish(mut self) -> Option<Stall> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        self.stalled()
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_resets_on_progress_or_pulse_only() {
        let t0 = Instant::now();
        let mut w = Watch::new(t0);
        let p = pulses();
        assert_eq!(w.tick(p, t0 + Duration::from_secs(5)), Duration::from_secs(5));
        // cùng bước, cùng done → không tính là tiến triển
        w.step(Step { phase: "start", done: 0, total: 0 }, t0 + Duration::from_secs(6));
        assert_eq!(w.tick(p, t0 + Duration::from_secs(7)), Duration::from_secs(7));
        // done tăng → về 0
        w.step(Step { phase: "start", done: 1, total: 9 }, t0 + Duration::from_secs(8));
        assert_eq!(w.tick(p, t0 + Duration::from_secs(9)), Duration::from_secs(1));
        // đổi bước → về 0
        w.step(Step { phase: "scan", done: 1, total: 9 }, t0 + Duration::from_secs(20));
        assert_eq!(w.tick(p, t0 + Duration::from_secs(20)), Duration::ZERO);
        // có nhịp → về 0
        assert_eq!(w.tick(p + 1, t0 + Duration::from_secs(50)), Duration::ZERO);
        assert_eq!(w.tick(p + 1, t0 + Duration::from_secs(51)), Duration::from_secs(1));
    }

    #[test]
    fn every_phase_has_a_label() {
        for p in [
            "count",
            "scan",
            "recover",
            "triage",
            "triage_read",
            "triage_clean",
            "triage_other",
            "hash_quick",
            "hash_full",
            "analyze",
            "suspect_hash",
            "reject_deleted",
        ] {
            assert_ne!(phase_label(p), "Bước không rõ", "{p}");
        }
    }

    #[test]
    fn watchdog_cancels_a_silent_step() {
        let cancel = Arc::new(AtomicBool::new(false));
        let wd = Watchdog::start(cancel.clone(), Duration::from_millis(150), |_| {});
        wd.observe(Step { phase: "hash_full", done: 3, total: 10 });
        wd.detail("D:\\books\\x.pdf");
        // Lưu ý: test khác chạy song song có thể gọi pulse() → chờ tới khi phát hiện, có giới hạn.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !cancel.load(Ordering::Relaxed) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let s = wd.finish().expect("phải phát hiện treo");
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!((s.phase, s.done, s.total), ("hash_full", 3, 10));
        let msg = s.to_string();
        assert!(msg.contains("Băm đầy đủ") && msg.contains("3 / 10") && msg.contains("x.pdf"), "{msg}");
    }

    #[test]
    fn watchdog_stays_quiet_while_work_progresses() {
        let cancel = Arc::new(AtomicBool::new(false));
        let wd = Watchdog::start(cancel.clone(), Duration::from_millis(300), |_| {});
        for i in 0..20 {
            wd.observe(Step { phase: "scan", done: i, total: 20 });
            std::thread::sleep(Duration::from_millis(40));
        }
        assert!(wd.finish().is_none());
        assert!(!cancel.load(Ordering::Relaxed));
    }
}
