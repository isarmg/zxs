//! Target writer resources in private temporary roots only. These fixtures do
//! not run an Engine cutover, publish business data or release a source fence.
use super::*;
use crate::persist::layout;
use std::{fs, os::unix::fs::PermissionsExt};

fn target() -> (tempfile::TempDir, Target, DiskRegistrationIdentity) {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    let config = Config::development([root.path().to_owned()]).unwrap();
    let disk = &config.disks[0];
    let identity = DiskRegistrationIdentity {
        pool_id: config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
    };
    let lock = AdvisoryLock::directory(&root.directory(root.path()).unwrap()).unwrap();
    (temp, Target::new(root, lock), identity)
}

fn reopen(path: &std::path::Path) -> Target {
    let root = DiskRoot::open(path).unwrap();
    let lock = AdvisoryLock::directory(&root.directory(root.path()).unwrap()).unwrap();
    Target::new(root, lock)
}

#[test]
fn retained_target_keeps_both_real_locks_until_its_last_owner() {
    let (temp, target, identity) = target();
    let plan = "1".repeat(64);
    install(&target, &plan, &identity).unwrap();
    let original = observe(&target, &plan, &identity).unwrap();
    let retained = target.retain().unwrap();
    drop(target);
    let independent = DiskRoot::open(temp.path()).unwrap();
    assert!(AdvisoryLock::directory(&independent.directory(independent.path()).unwrap()).is_err());
    let other = independent
        .open_regular_entry(&layout::DISK_WRITER_LOCK)
        .unwrap();
    assert!(
        rustix::fs::flock(&other, rustix::fs::FlockOperation::NonBlockingLockExclusive).is_err()
    );
    retained.require_writer(&original).unwrap();
    drop(retained);
    let resumed = reopen(temp.path());
    resume_existing(std::slice::from_ref(&resumed)).unwrap();
    assert_eq!(observe(&resumed, &plan, &identity).unwrap(), original);
}

#[test]
fn cold_read_of_unfinished_intent_never_creates_a_writer() {
    let (temp, target, identity) = target();
    let plan = "1".repeat(64);
    let bytes = intent(&target, &plan, &identity).unwrap();
    install_preparation_bytes(&target.root, &target.root.path().join(LOCAL_INTENT), &bytes)
        .unwrap();
    drop(target);
    let resumed = reopen(temp.path());
    resume_existing(std::slice::from_ref(&resumed)).unwrap();
    assert!(!temp.path().join(layout::DISK_WRITER_LOCK.as_path()).exists());
    assert!(observe(&resumed, &plan, &identity).is_err());
    assert!(!temp.path().join(BINDING).exists());
    install(&resumed, &plan, &identity).unwrap();
    observe(&resumed, &plan, &identity).unwrap();
}

#[test]
fn exact_unbound_empty_file_can_finish_only_its_original_intent() {
    let (temp, target, identity) = target();
    let plan = "1".repeat(64);
    let bytes = intent(&target, &plan, &identity).unwrap();
    install_preparation_bytes(&target.root, &target.root.path().join(LOCAL_INTENT), &bytes)
        .unwrap();
    install_preparation_bytes(
        &target.root,
        &target.root.path().join(layout::DISK_WRITER_LOCK.as_path()),
        &[],
    )
    .unwrap();
    drop(target);
    let resumed = reopen(temp.path());
    resume_existing(std::slice::from_ref(&resumed)).unwrap();
    let before = resumed.acquire_writer().unwrap();
    assert!(install(&resumed, &"2".repeat(64), &identity).is_err());
    assert!(!temp.path().join(BINDING).exists());
    install(&resumed, &plan, &identity).unwrap();
    assert_eq!(observe(&resumed, &plan, &identity).unwrap(), before);
}

#[test]
fn unowned_writer_name_is_preserved_and_not_adopted() {
    let (_temp, target, identity) = target();
    install_preparation_bytes(
        &target.root,
        &target.root.path().join(layout::DISK_WRITER_LOCK.as_path()),
        &[],
    )
    .unwrap();
    assert_eq!(
        resume_existing(std::slice::from_ref(&target))
            .unwrap_err()
            .code(),
        ErrorCode::ObjectCollision
    );
    assert_eq!(
        install(&target, &"1".repeat(64), &identity)
            .unwrap_err()
            .code(),
        ErrorCode::ObjectCollision
    );
    assert_eq!(
        fs::read(target.root.path().join(layout::DISK_WRITER_LOCK.as_path())).unwrap(),
        Vec::<u8>::new()
    );
    assert!(!target.root.path().join(LOCAL_INTENT).exists());
}

#[test]
fn bound_missing_writer_is_not_regenerated_by_inspection_or_continuation() {
    let (temp, target, identity) = target();
    let plan = "1".repeat(64);
    install(&target, &plan, &identity).unwrap();
    let receipt = fs::read(temp.path().join(BINDING)).unwrap();
    drop(target);
    fs::remove_file(temp.path().join(layout::DISK_WRITER_LOCK.as_path())).unwrap();
    let resumed = reopen(temp.path());
    assert!(resume_existing(std::slice::from_ref(&resumed)).is_err());
    assert!(install(&resumed, &plan, &identity).is_err());
    assert!(!temp.path().join(layout::DISK_WRITER_LOCK.as_path()).exists());
    assert_eq!(fs::read(temp.path().join(BINDING)).unwrap(), receipt);
}

#[test]
fn replaced_empty_writer_inode_cannot_be_rebound() {
    let (temp, target, identity) = target();
    let plan = "1".repeat(64);
    install(&target, &plan, &identity).unwrap();
    let receipt = fs::read(temp.path().join(BINDING)).unwrap();
    fs::rename(
        temp.path().join(layout::DISK_WRITER_LOCK.as_path()),
        temp.path().join("retained-old-writer"),
    )
    .unwrap();
    install_preparation_bytes(
        &target.root,
        &target.root.path().join(layout::DISK_WRITER_LOCK.as_path()),
        &[],
    )
    .unwrap();
    assert_eq!(
        target.recheck_writer().unwrap_err().code(),
        ErrorCode::DiskIdentityMismatch
    );
    drop(target);
    let resumed = reopen(temp.path());
    assert_eq!(
        resume_existing(std::slice::from_ref(&resumed))
            .unwrap_err()
            .code(),
        ErrorCode::DiskIdentityMismatch
    );
    assert!(install(&resumed, &plan, &identity).is_err());
    assert_eq!(fs::read(temp.path().join(BINDING)).unwrap(), receipt);
    assert!(temp.path().join("retained-old-writer").is_file());
}

#[test]
fn copied_writer_intent_cannot_rebind_another_physical_root() {
    let (_first_temp, first, identity) = target();
    let (_second_temp, second, _) = target();
    let bytes = intent(&first, &"1".repeat(64), &identity).unwrap();
    install_preparation_bytes(&second.root, &second.root.path().join(LOCAL_INTENT), &bytes)
        .unwrap();
    assert!(resume_existing(std::slice::from_ref(&second)).is_err());
    assert!(!second.root.path().join(layout::DISK_WRITER_LOCK.as_path()).exists());
    assert_eq!(
        fs::read(second.root.path().join(LOCAL_INTENT)).unwrap(),
        bytes
    );
}

#[test]
fn nonempty_writer_under_an_intent_is_preserved() {
    let (_temp, target, identity) = target();
    let plan = "1".repeat(64);
    let bytes = intent(&target, &plan, &identity).unwrap();
    install_preparation_bytes(&target.root, &target.root.path().join(LOCAL_INTENT), &bytes)
        .unwrap();
    install_preparation_bytes(
        &target.root,
        &target.root.path().join(layout::DISK_WRITER_LOCK.as_path()),
        b"not a lock",
    )
    .unwrap();
    assert!(resume_existing(std::slice::from_ref(&target)).is_err());
    assert!(install(&target, &plan, &identity).is_err());
    assert_eq!(
        fs::read(target.root.path().join(layout::DISK_WRITER_LOCK.as_path())).unwrap(),
        b"not a lock"
    );
    assert!(!target.root.path().join(BINDING).exists());
}
