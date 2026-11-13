//! E5-01 trên Windows thật: file vào Thùng rác và Restore được.
//! Chạm vào Thùng rác của người dùng (một file tạm, được khôi phục rồi xoá ngay) nên để `#[ignore]`:
//!     cargo test -p librarian-core --test windows_recycle_bin -- --ignored
#![cfg(windows)]

use librarian_core::trash::{SystemTrasher, Trasher};

#[test]
#[ignore]
fn system_trasher_goes_to_recycle_bin_and_restores() {
    let dir = tempfile::tempdir().unwrap();
    let name = format!("47-librarian-test-{}.epub", std::process::id());
    let p = dir.path().join(&name);
    std::fs::write(&p, b"test").unwrap();
    SystemTrasher.trash(&p).unwrap();
    assert!(!p.exists());
    let items: Vec<_> =
        trash::os_limited::list().unwrap().into_iter().filter(|i| i.name == std::ffi::OsString::from(&name)).collect();
    assert_eq!(items.len(), 1, "file phải nằm trong Thùng rác");
    trash::os_limited::restore_all(items).unwrap();
    assert!(p.exists(), "Restore trả file về chỗ cũ");
    std::fs::remove_file(&p).unwrap();
}
