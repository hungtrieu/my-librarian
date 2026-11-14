//! Sổ sách đã xoá: tải lại đúng file đã xoá → lần quét sau tự vào Thùng rác; khôi phục từ Thùng rác thì giữ;
//! bản gần giống → nghi trùng (cùng mục "đã xoá"), không tự dọn.

use librarian_core::dedupe::{self, Decision};
use librarian_core::trash::{self, Trasher};
use librarian_core::{deleted, report, testdata, Library};
use std::cell::Cell;
use std::path::{Path, PathBuf};

struct DirTrash {
    dir: PathBuf,
    n: Cell<usize>,
}
impl Trasher for DirTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        self.n.set(self.n.get() + 1);
        let dst = self.dir.join(format!("{}-{}", self.n.get(), path.file_name().unwrap().to_string_lossy()));
        std::fs::rename(path, dst).map_err(|e| e.to_string())
    }
}
impl DirTrash {
    /// Đường dẫn trong "Thùng rác" của lần xoá thứ `n`.
    fn item(&self, n: usize, name: &str) -> PathBuf {
        self.dir.join(format!("{n}-{name}"))
    }
    /// Dọn sạch Thùng rác.
    fn empty(&self) {
        for e in std::fs::read_dir(&self.dir).unwrap() {
            std::fs::remove_file(e.unwrap().path()).unwrap();
        }
    }
}

fn epub(path: &Path, title: &str, body: &str) {
    testdata::write_epub(
        path,
        &testdata::EpubSpec {
            title, authors: &["Nguyễn Nhật Ánh"], subjects: &[], isbn: None, language: "vi", body
        },
    )
    .unwrap();
}

struct Env {
    _tmp: tempfile::TempDir,
    books: PathBuf,
    lib: Library,
    trash: DirTrash,
}

fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let books = tmp.path().join("books");
    epub(&books.join("Văn học").join("Mắt Biếc.epub"), "Mắt Biếc", "nội dung bản gốc");
    epub(&books.join("Văn học").join("Cho tôi xin một vé.epub"), "Cho tôi xin một vé đi tuổi thơ", "khác");
    let trash = DirTrash { dir: tmp.path().join("trash"), n: Cell::new(0) };
    std::fs::create_dir_all(&trash.dir).unwrap();
    let mut lib = Library::open(&tmp.path().join("home")).unwrap();
    lib.set_library_root(&books).unwrap();
    let e = Env { _tmp: tmp, books, lib, trash };
    analyze(&e);
    e
}

fn analyze(e: &Env) -> report::ReportStats {
    report::run_analysis(&e.lib.conn, &e.lib.config, "app", None, Some(&e.trash)).unwrap()
}

fn id_of(e: &Env, file: &str) -> i64 {
    e.lib.conn.query_row("SELECT id FROM books WHERE file_name = ?1", [file], |r| r.get(0)).unwrap()
}

/// Lùi thời điểm xoá trong sổ (để sách thêm "sau đó" khác giây với lúc xoá).
fn backdate(e: &Env) {
    e.lib.conn.execute("UPDATE deleted_books SET deleted_at = deleted_at - 10", []).unwrap();
}

#[test]
fn redownloaded_deleted_book_is_trashed_on_next_scan() {
    let e = setup();
    let orig = e.books.join("Văn học").join("Mắt Biếc.epub");
    let bytes = std::fs::read(&orig).unwrap();
    let id = id_of(&e, "Mắt Biếc.epub");
    let r = trash::trash_books(&e.lib.conn, &e.lib.config, &e.trash, &[id]).unwrap();
    assert_eq!(r.trashed, 1);
    assert_eq!(deleted::count(&e.lib.conn).unwrap(), 1, "đã ghi vào sổ");
    e.trash.empty(); // dọn sạch Thùng rác Windows: sổ vẫn nhớ

    // Tải lại đúng file đó, tên khác, chỗ khác.
    let again = e.books.join("Khác").join("mat-biec (1).epub");
    std::fs::create_dir_all(again.parent().unwrap()).unwrap();
    std::fs::write(&again, &bytes).unwrap();
    let st = analyze(&e);
    assert_eq!(st.auto_rejected, Some(1));
    assert!(!again.exists(), "đã tự vào Thùng rác");
    assert!(report::banner(&report::latest(&e.lib.conn).unwrap().unwrap()).unwrap().contains("đã xoá trước đây"));
    let kind: String =
        e.lib.conn.query_row("SELECT kind FROM batches ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(kind, deleted::REJECT_KIND, "ghi lịch sử thao tác");
    assert_eq!(deleted::count(&e.lib.conn).unwrap(), 1, "không nhân đôi mục trong sổ");

    // Tải lại lần nữa, đúng đường dẫn cũ (file mới, thời điểm sửa khác) → vẫn bị loại.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&orig, &bytes).unwrap();
    let st = analyze(&e);
    assert_eq!(st.auto_rejected, Some(1));
    assert!(!orig.exists());

    // Sách khác không bị đụng.
    assert!(e.books.join("Văn học").join("Cho tôi xin một vé.epub").is_file());
}

#[test]
fn restoring_from_recycle_bin_keeps_the_book_and_forgets_it() {
    let e = setup();
    let orig = e.books.join("Văn học").join("Mắt Biếc.epub");
    let id = id_of(&e, "Mắt Biếc.epub");
    trash::trash_books(&e.lib.conn, &e.lib.config, &e.trash, &[id]).unwrap();
    assert_eq!(deleted::count(&e.lib.conn).unwrap(), 1);
    // "Restore" trong Thùng rác: file về đúng chỗ cũ, giữ nguyên thời điểm sửa.
    std::fs::rename(e.trash.item(1, "Mắt Biếc.epub"), &orig).unwrap();
    let st = analyze(&e);
    assert_eq!(st.auto_rejected, Some(0));
    assert!(orig.is_file(), "khôi phục thì giữ");
    assert_eq!(deleted::count(&e.lib.conn).unwrap(), 0, "gỡ khỏi sổ");
    let status: String = e.lib.conn.query_row("SELECT status FROM books WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(status, "present");
}

#[test]
fn existing_books_are_never_rejected_and_exact_dedupe_is_not_remembered() {
    let e = setup();
    // Hai bản y hệt có sẵn → dọn trùng chắc chắn: nội dung vẫn còn trong thư viện, không ghi sổ.
    let a = e.books.join("Văn học").join("Mắt Biếc.epub");
    let b = e.books.join("Khác").join("Mắt Biếc - copy.epub");
    std::fs::create_dir_all(b.parent().unwrap()).unwrap();
    std::fs::copy(&a, &b).unwrap();
    analyze(&e);
    let groups = dedupe::exact_groups(&e.lib.conn).unwrap();
    assert_eq!(groups.len(), 1);
    dedupe::clean_exact(&e.lib.conn, &e.lib.config, &e.trash, &groups, &Default::default()).unwrap();
    assert_eq!(deleted::count(&e.lib.conn).unwrap(), 0);
    // Xoá tay một cuốn trong khi bản y hệt khác đang có trong thư viện: bản đang có không bao giờ bị loại.
    std::fs::copy(&a, &b).unwrap();
    analyze(&e);
    let id = id_of(&e, "Mắt Biếc - copy.epub");
    trash::trash_books(&e.lib.conn, &e.lib.config, &e.trash, &[id]).unwrap();
    let st = analyze(&e);
    assert_eq!(st.auto_rejected, Some(0));
    assert!(a.is_file());
}

#[test]
fn similar_redownload_goes_to_suspects_with_a_deleted_member() {
    let e = setup();
    let id = id_of(&e, "Mắt Biếc.epub");
    trash::trash_books(&e.lib.conn, &e.lib.config, &e.trash, &[id]).unwrap();
    backdate(&e);
    // Bản khác (ấn bản khác, nội dung khác) cùng tên.
    let other = e.books.join("Khác").join("Mắt Biếc (bản đẹp).epub");
    epub(&other, "Mắt Biếc", "một ấn bản khác hẳn, dài hơn nhiều so với bản cũ đã xoá");
    let st = analyze(&e);
    assert_eq!(st.auto_rejected, Some(0), "không phải y hệt → không tự xoá");
    assert!(other.is_file());
    assert_eq!(st.suspect_groups, 1);
    let groups = dedupe::suspect_groups(&e.lib.conn).unwrap();
    let g = &groups[0];
    assert!(g.has_deleted);
    assert_eq!(g.members.len(), 2);
    let ghost = g.members.iter().find(|b| b.id < 0).expect("có mục đã xoá");
    assert_eq!(ghost.status, "deleted");
    assert!(g.keeper_id > 0, "bản đề xuất giữ luôn là sách đang có");

    // Không tự dọn dù độ tin cậy cao.
    let r = dedupe::clean_suspects_with(&e.lib.conn, &e.lib.config, &e.trash, &groups, 0, None, None).unwrap();
    assert_eq!(r.trashed, 0);
    assert!(other.is_file());

    // "Giữ tất cả" ghi nhớ được cả với mục đã xoá → không hiện lại.
    let ids: Vec<i64> = g.members.iter().map(|b| b.id).collect();
    assert!(dedupe::decide(&e.lib.conn, &ids, Decision::KeepBoth).unwrap() > 0);
    assert!(dedupe::suspect_groups(&e.lib.conn).unwrap().is_empty());
}

#[test]
fn book_kept_when_its_twin_was_deleted_is_not_a_suspect() {
    let e = setup();
    // Hai ấn bản cùng tên có sẵn; người dùng xoá một bản → bản còn lại (có từ trước) không bị hỏi lại.
    epub(&e.books.join("Khác").join("Mắt Biếc 2.epub"), "Mắt Biếc", "ấn bản thứ hai, nội dung khác");
    analyze(&e);
    assert_eq!(dedupe::suspect_groups(&e.lib.conn).unwrap().len(), 1);
    let id = id_of(&e, "Mắt Biếc 2.epub");
    trash::trash_books(&e.lib.conn, &e.lib.config, &e.trash, &[id]).unwrap();
    assert!(dedupe::suspect_groups(&e.lib.conn).unwrap().is_empty());
}

#[test]
fn rejecting_can_be_turned_off_and_respects_read_only() {
    let mut e = setup();
    let orig = e.books.join("Văn học").join("Mắt Biếc.epub");
    let bytes = std::fs::read(&orig).unwrap();
    trash::trash_books(&e.lib.conn, &e.lib.config, &e.trash, &[id_of(&e, "Mắt Biếc.epub")]).unwrap();
    let again = e.books.join("Văn học").join("tải lại.epub");
    std::fs::write(&again, &bytes).unwrap();
    e.lib.config.auto_reject_deleted = false;
    let st = analyze(&e);
    assert_eq!(st.auto_rejected, None);
    assert!(again.is_file());
    e.lib.config.read_only = true;
    e.lib.config.auto_reject_deleted = true;
    analyze(&e);
    assert!(again.is_file());
    // Bật lại: sách thêm từ khi tắt vẫn được xét (mốc chỉ tiến khi đã kiểm tra).
    e.lib.config.read_only = false;
    let st = analyze(&e);
    assert_eq!(st.auto_rejected, Some(1));
    assert!(!again.exists());
}

/// Duyệt nghi trùng 3 ấn bản: giữ 1, xoá 2. Sau đó copy lại cả 3 (copy kiểu Explorer: giữ nguyên thời điểm sửa,
/// 2 bản đã xoá về đúng chỗ cũ) → 2 bản đã xoá tự vào Thùng rác, bản đã giữ thành trùng chắc chắn với chính nó,
/// không hiện lại ở "Nghi trùng".
#[test]
fn copying_back_three_reviewed_editions() {
    let e = setup();
    let dir = e.books.join("Văn học");
    epub(&dir.join("Mắt Biếc (bản 2).epub"), "Mắt Biếc", "ấn bản thứ hai, nội dung khác hẳn bản gốc");
    epub(&dir.join("Mắt Biếc (bản 3).epub"), "Mắt Biếc", "ấn bản thứ ba, lại khác nữa, dài hơn một chút");
    let backup = e._tmp.path().join("backup");
    std::fs::create_dir_all(&backup).unwrap();
    for f in ["Mắt Biếc.epub", "Mắt Biếc (bản 2).epub", "Mắt Biếc (bản 3).epub"] {
        std::fs::copy(dir.join(f), backup.join(f)).unwrap();
    }
    analyze(&e);
    let groups = dedupe::suspect_groups(&e.lib.conn).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].members.len(), 3);

    // Duyệt: giữ bản gốc, xoá hai bản kia; dọn sạch Thùng rác.
    let drop = [id_of(&e, "Mắt Biếc (bản 2).epub"), id_of(&e, "Mắt Biếc (bản 3).epub")];
    assert_eq!(trash::trash_books(&e.lib.conn, &e.lib.config, &e.trash, &drop).unwrap().trashed, 2);
    // Thời gian thật: sách có từ lâu, xoá cách đây một lúc, copy lại bây giờ.
    e.lib.conn.execute("UPDATE books SET added_at = added_at - 20", []).unwrap();
    backdate(&e);
    e.trash.empty();
    assert!(dedupe::suspect_groups(&e.lib.conn).unwrap().is_empty());

    // Copy lại cả 3: hai bản đã xoá về đúng chỗ cũ, bản đã giữ sang chỗ khác (chỗ cũ đang có file).
    std::fs::copy(backup.join("Mắt Biếc (bản 2).epub"), dir.join("Mắt Biếc (bản 2).epub")).unwrap();
    std::fs::copy(backup.join("Mắt Biếc (bản 3).epub"), dir.join("Mắt Biếc (bản 3).epub")).unwrap();
    std::fs::create_dir_all(e.books.join("Khác")).unwrap();
    std::fs::copy(backup.join("Mắt Biếc.epub"), e.books.join("Khác").join("Mắt Biếc.epub")).unwrap();
    let st = analyze(&e);
    assert_eq!(st.auto_rejected, Some(2), "copy giữ thời điểm sửa không bị nhầm là khôi phục");
    assert!(!dir.join("Mắt Biếc (bản 2).epub").exists());
    assert!(!dir.join("Mắt Biếc (bản 3).epub").exists());
    assert_eq!(deleted::count(&e.lib.conn).unwrap(), 2, "sổ vẫn nhớ hai bản đã xoá");
    assert_eq!(st.exact_files, 1, "bản đã giữ copy lại = trùng chắc chắn");
    assert_eq!(st.suspect_groups, 0);
}
