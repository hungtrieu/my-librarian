//! Cấu hình ứng dụng, lưu tại `<home>/config.json`.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_INBOX: &str = "Chưa phân loại";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Thư mục thư viện duy nhất (D2), vd. `E:\books`.
    pub library_root: Option<PathBuf>,
    /// Tên thư mục hộp thư đến bên trong `library_root` (D10).
    pub inbox_name: String,
    /// Thư mục nguồn phụ (E1-01, tuỳ chọn).
    pub extra_roots: Vec<PathBuf>,
    /// Tên thư mục bị bỏ qua khi quét (so khớp không phân biệt hoa thường).
    pub exclude_names: Vec<String>,
    /// Chế độ chỉ đọc toàn app (E10-02): không xoá, không di chuyển.
    pub read_only: bool,
    /// E8-05: tự dọn trùng chắc chắn khi quét ngầm. Mặc định TẮT.
    pub auto_clean_exact_in_background: bool,
    /// Tự phân loại hộp thư đến mỗi lần quét (đọc metadata.opf, làm phẳng thư mục kiểu Calibre, tạo thư mục
    /// thể loại mới, chưa chắc → "Khác", dọn mọi file không phải sách và thư mục thừa vào Thùng rác). Mặc định BẬT.
    pub auto_triage: bool,
    /// Mỗi lần quét, sách mới tải về có nội dung y hệt một cuốn đã xoá trước đây (xem deleted.rs) được tự đưa vào
    /// Thùng rác. Mặc định BẬT.
    pub auto_reject_deleted: bool,
    /// Thư mục thể loại bị chặn (đường dẫn tương đối so với `library_root`, phân cách `/`, có thể là thư mục con
    /// người dùng tự chia, vd. `Ngôn tình - Đam mỹ/Đam mỹ`). Sách trong đó vào Thùng rác; sách tự phân loại vào đó
    /// cũng vậy, và thư mục không được tạo lại. Xem `blocked.rs`.
    pub blocked_categories: Vec<String>,
    /// Ánh xạ tag (metadata `subject` của sách) → thư mục thể loại (đường dẫn tương đối, có thể là thư mục con,
    /// vd. `Tiểu thuyết/Nước ngoài`). Tag so khớp không phân biệt dấu / hoa thường / ký tự ngăn cách, nên
    /// `Tiểu thuyết` và `tieu-thuyet` là một. Đổi ánh xạ thì sách mang tag đó được chuyển sang thư mục mới; sách mới
    /// tự phân loại cũng theo ánh xạ trước luật có sẵn. Xem `tagmap.rs`.
    pub tag_map: Vec<TagRule>,
    /// Ngưỡng tin cậy phân loại (E6-04).
    pub classify_high: u8,
    pub classify_medium: u8,
    /// Số luồng băm (SSD ngoài: 2–4 là hợp lý; chốt ở S5).
    pub hash_threads: usize,
    /// Giờ quét ngầm hằng ngày (HH:MM) – dùng ở M6.
    pub scan_time: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            library_root: None,
            inbox_name: DEFAULT_INBOX.to_string(),
            extra_roots: Vec::new(),
            exclude_names: vec!["$RECYCLE.BIN".into(), "System Volume Information".into(), ".Trashes".into()],
            read_only: false,
            auto_clean_exact_in_background: false,
            auto_triage: true,
            auto_reject_deleted: true,
            blocked_categories: Vec::new(),
            tag_map: Vec::new(),
            classify_high: 75,
            classify_medium: 45,
            hash_threads: 3,
            scan_time: "03:00".into(),
        }
    }
}

/// Một dòng ánh xạ tag → thư mục thể loại.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagRule {
    /// Tag như người dùng thấy (một trong các cách viết).
    pub tag: String,
    /// Thư mục đích, tương đối so với thư mục thư viện, phân cách `/`.
    pub target: String,
}

impl Config {
    pub fn path(home: &Path) -> PathBuf {
        home.join("config.json")
    }

    pub fn load(home: &Path) -> Result<Config> {
        let p = Self::path(home);
        if !p.exists() {
            return Ok(Config::default());
        }
        let s = std::fs::read_to_string(p)?;
        Ok(serde_json::from_str(&s)?)
    }

    pub fn save(&self, home: &Path) -> Result<()> {
        std::fs::create_dir_all(home)?;
        let tmp = home.join("config.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(tmp, Self::path(home))?;
        Ok(())
    }

    pub fn inbox_dir(&self) -> Option<PathBuf> {
        self.library_root.as_ref().map(|r| r.join(&self.inbox_name))
    }

    /// Mọi thư mục gốc cần quét: thư viện trước, rồi các nguồn phụ.
    pub fn roots(&self) -> Vec<(PathBuf, &'static str)> {
        let mut v = Vec::new();
        if let Some(r) = &self.library_root {
            v.push((r.clone(), "library"));
        }
        for r in &self.extra_roots {
            v.push((r.clone(), "source"));
        }
        v
    }
}

/// Thư mục dữ liệu app, theo thứ tự:
/// 1. biến `LIBRARIAN_HOME`;
/// 2. `<thư mục cài>\data` – khi chạy từ bản cài bằng bộ cài (có `uninstall.exe` cạnh exe) hoặc đã có sẵn
///    thư mục `data` cạnh exe. Cài app ở ổ E thì mọi thứ (DB, ảnh bìa, cache, log, WebView) nằm ở ổ E, không làm đầy ổ C;
/// 3. `%APPDATA%\my-librarian` (bản dev, bản MSI cài vào Program Files, hoặc thư mục cài không ghi được).
pub fn default_home() -> PathBuf {
    if let Some(h) = std::env::var_os("LIBRARIAN_HOME") {
        return PathBuf::from(h);
    }
    install_home().unwrap_or_else(legacy_home)
}

/// Chỗ dữ liệu cũ (trước khi dữ liệu theo thư mục cài): `%APPDATA%\my-librarian`.
pub fn legacy_home() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("my-librarian")
}

fn install_home() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    install_home_in(&exe_dir)
}

fn install_home_in(exe_dir: &Path) -> Option<PathBuf> {
    let data = exe_dir.join("data");
    if !(data.is_dir() || exe_dir.join("uninstall.exe").is_file()) {
        return None;
    }
    std::fs::create_dir_all(&data).ok()?;
    let probe = data.join(".write-test");
    std::fs::write(&probe, b"").ok()?;
    let _ = std::fs::remove_file(&probe);
    Some(data)
}

/// Lần đầu chạy bản dữ-liệu-theo-thư-mục-cài: chép dữ liệu cũ ở `%APPDATA%\my-librarian` sang `home` rồi xoá bản cũ.
/// Không làm gì nếu đặt `LIBRARIAN_HOME`, `home` chính là chỗ cũ, chỗ cũ không có DB, hoặc `home` đã có DB.
/// Trả về thư mục cũ nếu đã chuyển.
pub fn migrate_legacy_home(home: &Path) -> std::io::Result<Option<PathBuf>> {
    if std::env::var_os("LIBRARIAN_HOME").is_some() {
        return Ok(None);
    }
    let old = legacy_home();
    // Lần chuyển trước không xoá hết được (file đang bị khoá): chỗ cũ không còn DB thì dọn nốt phần sót.
    if old != home && old.is_dir() && !old.join("library.db").exists() && home.join("library.db").is_file() {
        let _ = std::fs::remove_dir_all(&old);
        return Ok(None);
    }
    migrate_dir(&old, home)
}

/// Dữ liệu đang nằm theo thư mục cài (không phải `%APPDATA%`, không do `LIBRARIAN_HOME` chỉ định).
pub fn is_install_home(home: &Path) -> bool {
    std::env::var_os("LIBRARIAN_HOME").is_none() && home != legacy_home()
}

fn migrate_dir(old: &Path, home: &Path) -> std::io::Result<Option<PathBuf>> {
    if old == home || !old.join("library.db").is_file() || home.join("library.db").exists() {
        return Ok(None);
    }
    std::fs::create_dir_all(home)?;
    // DB chép sau cùng: tiến trình khác thấy `library.db` là đã có đủ ảnh bìa, cấu hình…
    let is_db = |n: &str| n.starts_with("library.db");
    for pass_db in [false, true] {
        for e in std::fs::read_dir(old)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if name == "scan.lock" || is_db(&name) != pass_db {
                continue;
            }
            copy_tree(&e.path(), &home.join(&name))?;
        }
    }
    std::fs::remove_dir_all(old)?;
    Ok(Some(old.to_path_buf()))
}

/// Chép đệ quy file/thư mục (dùng được giữa hai ổ đĩa, nơi `rename` không chạy).
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

#[cfg(test)]
mod home_tests {
    use super::*;

    #[test]
    fn install_home_only_for_installed_or_portable() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(install_home_in(d.path()), None);
        std::fs::write(d.path().join("uninstall.exe"), b"").unwrap();
        assert_eq!(install_home_in(d.path()), Some(d.path().join("data")));
        let p = tempfile::tempdir().unwrap();
        std::fs::create_dir(p.path().join("data")).unwrap();
        assert_eq!(install_home_in(p.path()), Some(p.path().join("data")));
    }

    #[test]
    fn migrate_moves_old_data_once() {
        let old = tempfile::tempdir().unwrap();
        let new = tempfile::tempdir().unwrap();
        let home = new.path().join("data");
        std::fs::write(old.path().join("library.db"), b"db").unwrap();
        std::fs::write(old.path().join("scan.lock"), b"").unwrap();
        std::fs::create_dir_all(old.path().join("covers/ab")).unwrap();
        std::fs::write(old.path().join("covers/ab/1.jpg"), b"img").unwrap();
        assert!(migrate_dir(old.path(), &home).unwrap().is_some());
        assert_eq!(std::fs::read(home.join("library.db")).unwrap(), b"db");
        assert_eq!(std::fs::read(home.join("covers/ab/1.jpg")).unwrap(), b"img");
        assert!(!home.join("scan.lock").exists());
        assert!(!old.path().exists());
        // Lần sau: không còn gì để chuyển.
        assert!(migrate_dir(old.path(), &home).unwrap().is_none());
    }
}
