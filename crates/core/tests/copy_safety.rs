//! Chép sang ổ khác an toàn: file tạm ẩn → so SHA-256 → đổi tên → mới xoá bản gốc; lần chép bị ngắt (mất điện, app
//! bị tắt) để lại file tạm thì lần sau tự dọn, bản gốc không bao giờ mất.

use librarian_core::moves;

#[test]
fn copy_is_verified_and_interrupted_copies_are_cleaned_up() {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("LIBRARIAN_HOME", tmp.path().join("home"));
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let body: Vec<u8> = (0..9_000_000u32).map(|i| (i % 251) as u8).collect(); // > 2 khúc 4 MB
    std::fs::write(a.join("x.pdf"), &body).unwrap();

    moves::copy_verify_replace(&a.join("x.pdf"), &b.join("x.pdf")).unwrap();
    assert_eq!(std::fs::read(b.join("x.pdf")).unwrap(), body);
    assert!(!a.join("x.pdf").exists(), "bản gốc chỉ bị xoá sau khi bản chép khớp");
    assert!(!moves::temp_copy_path(&b.join("x.pdf")).exists());
    assert!(!tmp.path().join("home").join("copy-inflight.txt").exists());

    // Đích đã có file: không ghi đè, bản gốc giữ nguyên, không để lại file tạm.
    std::fs::write(a.join("y.pdf"), b"moi").unwrap();
    std::fs::write(b.join("y.pdf"), b"cu").unwrap();
    assert!(moves::copy_verify_replace(&a.join("y.pdf"), &b.join("y.pdf")).is_err());
    assert_eq!(std::fs::read(b.join("y.pdf")).unwrap(), b"cu");
    assert!(a.join("y.pdf").is_file());
    assert!(!moves::temp_copy_path(&b.join("y.pdf")).exists());

    // Giả lập mất điện giữa lúc chép: còn file tạm + ghi chú "đang chép". Lần sau dọn file tạm, bản gốc còn nguyên.
    let half = moves::temp_copy_path(&b.join("z.pdf"));
    std::fs::write(&half, &body[..1000]).unwrap();
    std::fs::write(tmp.path().join("home").join("copy-inflight.txt"), half.to_string_lossy().as_bytes()).unwrap();
    std::fs::write(a.join("z.pdf"), &body).unwrap();
    moves::recover_interrupted_copy();
    assert!(!half.exists());
    assert!(!tmp.path().join("home").join("copy-inflight.txt").exists());
    moves::copy_verify_replace(&a.join("z.pdf"), &b.join("z.pdf")).unwrap();
    assert_eq!(std::fs::read(b.join("z.pdf")).unwrap(), body);
}
