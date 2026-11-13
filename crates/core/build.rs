//! Toolchain GNU (MinGW/WinLibs – dùng khi máy không có Visual Studio Build Tools):
//! `unrar_sys` liên kết `-lpthread` dạng động, còn std của Rust liên kết `libpthread.a` tĩnh →
//! trùng ký hiệu khi link. Đặt một bản sao của `libpthread.a` tên `libpthread.dll.a` vào thư mục
//! tìm kiếm đứng trước thư mục hệ thống để `-lpthread` cũng thành tĩnh. Kết quả: .exe không cần
//! `libwinpthread-1.dll`. MSVC không bị ảnh hưởng.
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.ends_with("windows-gnu") {
        return;
    }
    let gcc = std::env::var("CC").unwrap_or_else(|_| "gcc".into());
    let Ok(out) = Command::new(&gcc).arg("-print-file-name=libpthread.a").output() else {
        println!("cargo:warning=không chạy được {gcc} để tìm libpthread.a");
        return;
    };
    let src = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    if !src.is_absolute() || !src.is_file() {
        println!("cargo:warning=không tìm thấy libpthread.a tĩnh ({})", src.display());
        return;
    }
    let dir = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("pthread-static");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&src, dir.join("libpthread.dll.a")).unwrap();
    println!("cargo:rustc-link-search=native={}", dir.display());
}
