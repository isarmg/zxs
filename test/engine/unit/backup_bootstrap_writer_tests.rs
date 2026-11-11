//! Temporary lock-file boundary fixtures only. They do not claim full Backup
//! publication/cutover recovery, physical media safety or runtime validation.
use super::*;
use crate::persist::layout;
use std::{fs, os::unix::fs::PermissionsExt};
fn root() -> (tempfile::TempDir, DiskRoot) {
    let temporary = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = DiskRoot::open(temporary.path()).unwrap();
    (temporary, root)
}
#[test]
fn cold_backup_lock_never_creates_a_missing_writer() {
    let (_temp, root) = root();
    assert!(lock_existing(&root, &layout::DISK_WRITER_LOCK).is_err());
    assert!(!root.path().join(layout::DISK_WRITER_LOCK.as_path()).exists());
}
#[test]
fn cold_backup_lock_retains_unrelated_writer_bytes() {
    let (_temp, root) = root();
    root.write_atomic(
        &root.path().join(layout::DISK_WRITER_LOCK.as_path()),
        b"unrelated",
        false,
    )
    .unwrap();
    assert!(lock_existing(&root, &layout::DISK_WRITER_LOCK).is_err());
    assert_eq!(
        fs::read(root.path().join(layout::DISK_WRITER_LOCK.as_path())).unwrap(),
        b"unrelated"
    );
}
#[test]
fn cold_backup_lock_is_exclusive_and_transferred_without_reacquisition() {
    let (_temp, root) = root();
    root.write_atomic(
        &root.path().join(layout::DISK_WRITER_LOCK.as_path()),
        b"",
        false,
    )
    .unwrap();
    let file = lock_existing(&root, &layout::DISK_WRITER_LOCK).unwrap();
    let expected = FileIdentity::from_fd(&file).unwrap();
    assert!(lock_existing(&root, &layout::DISK_WRITER_LOCK).is_err());
    let owner = FenceOwner::new(file, None);
    assert_eq!(owner.writer_identity().unwrap(), expected);
    assert!(lock_existing(&root, &layout::DISK_WRITER_LOCK).is_err());
    drop(owner);
    let reacquired = lock_existing(&root, &layout::DISK_WRITER_LOCK).unwrap();
    assert_eq!(FileIdentity::from_fd(&reacquired).unwrap(), expected);
}
#[test]
fn cold_backup_lock_rejects_a_hardlinked_writer_without_removal() {
    let (_temp, root) = root();
    root.write_atomic(
        &root.path().join(layout::DISK_WRITER_LOCK.as_path()),
        b"",
        false,
    )
    .unwrap();
    fs::hard_link(
        root.path().join(layout::DISK_WRITER_LOCK.as_path()),
        root.path().join("unrelated-link"),
    )
    .unwrap();
    assert!(lock_existing(&root, &layout::DISK_WRITER_LOCK).is_err());
    assert!(root.path().join("unrelated-link").is_file());
}
