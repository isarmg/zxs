use super::*;
use std::{
    cell::Cell,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAYLOAD: &[u8] = b"the exact embedded program fixture";

fn original(path: &Path) {
    fs::write(path, b"damaged original").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o444)).unwrap();
}

#[test]
fn exchange_retains_original_inode_and_same_request_retry_does_not_replace_again() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("zxs");
    original(&path);
    let inode = fs::metadata(&path).unwrap().ino();
    let destination = ProgramDestination::open(&path).unwrap();
    let captured = destination.repair_snapshot(PAYLOAD).unwrap();
    assert_eq!(
        destination
            .repair_original_bytes(DIGEST, &captured, PAYLOAD)
            .unwrap()
            .unwrap(),
        b"damaged original"
    );
    // The I/O layer is called only after the facade's independent backup barrier.
    let installed = destination
        .repair_install(DIGEST, &captured, PAYLOAD, || Ok(()))
        .unwrap();
    let retained = root.path().join(repair_stage(DIGEST).unwrap().as_os_str());
    assert_eq!(fs::read(&retained).unwrap(), b"damaged original");
    assert_eq!(fs::metadata(&retained).unwrap().ino(), inode);
    assert_eq!(fs::metadata(&retained).unwrap().mode() & 0o7777, 0o444);
    assert_eq!(fs::read(&path).unwrap(), PAYLOAD);
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o555);
    assert!(
        destination
            .repair_is_published(DIGEST, &captured, PAYLOAD)
            .unwrap()
    );
    let again = destination
        .repair_install(DIGEST, &captured, PAYLOAD, || Ok(()))
        .unwrap();
    assert!(installed == again);
    assert!(destination.repair_snapshot(PAYLOAD).is_err());
}

#[test]
fn missing_program_repair_is_no_clobber_and_recoverable_without_fabricated_original() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("zxs");
    let destination = ProgramDestination::open(&path).unwrap();
    assert!(destination.repair_snapshot(PAYLOAD).unwrap().is_none());
    assert!(
        destination
            .repair_original_bytes(DIGEST, &None, PAYLOAD)
            .unwrap()
            .is_none()
    );
    let first = destination
        .repair_install(DIGEST, &None, PAYLOAD, || Ok(()))
        .unwrap();
    assert!(
        destination
            .repair_install(DIGEST, &None, PAYLOAD, || Ok(()))
            .unwrap()
            == first
    );
    assert!(
        !root
            .path()
            .join(repair_stage(DIGEST).unwrap().as_os_str())
            .exists()
    );
}

#[test]
fn changed_source_or_missing_retained_inode_never_becomes_a_second_repair() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("zxs");
    original(&path);
    let destination = ProgramDestination::open(&path).unwrap();
    let captured = destination.repair_snapshot(PAYLOAD).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(&path, b"another writer").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    assert!(
        destination
            .repair_install(DIGEST, &captured, PAYLOAD, || Ok(()))
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"another writer");
    let next = destination.repair_snapshot(PAYLOAD).unwrap();
    destination
        .repair_install(DIGEST, &next, PAYLOAD, || Ok(()))
        .unwrap();
    fs::rename(
        root.path().join(repair_stage(DIGEST).unwrap().as_os_str()),
        root.path().join("retained-elsewhere"),
    )
    .unwrap();
    assert!(
        destination
            .completed_repair_snapshot(DIGEST, &next, PAYLOAD)
            .is_err()
    );
    assert!(
        destination
            .repair_install(DIGEST, &next, PAYLOAD, || Ok(()))
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), PAYLOAD);
}

#[test]
fn link_unsafe_mode_and_oversized_programs_are_rejected_without_repair() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("zxs");
    let other = root.path().join("other");
    original(&other);
    symlink(&other, &path).unwrap();
    let destination = ProgramDestination::open(&path).unwrap();
    assert!(destination.repair_snapshot(PAYLOAD).is_err());
    fs::remove_file(&path).unwrap();
    fs::hard_link(&other, &path).unwrap();
    assert!(destination.repair_snapshot(PAYLOAD).is_err());
    fs::remove_file(&path).unwrap();
    original(&path);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(destination.repair_snapshot(PAYLOAD).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(MAX_PROGRAM_BYTES as u64 + 1)
        .unwrap();
    assert!(destination.repair_snapshot(PAYLOAD).is_err());
}

#[test]
fn interrupted_staging_reuses_exact_payload_and_conflicts_preserve_both_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("zxs");
    original(&path);
    let destination = ProgramDestination::open(&path).unwrap();
    let captured = destination.repair_snapshot(PAYLOAD).unwrap();
    let calls = Cell::new(0);
    assert!(
        destination
            .repair_install(DIGEST, &captured, PAYLOAD, || {
                calls.set(calls.get() + 1);
                if calls.get() == 2 {
                    Err("baseline changed".into())
                } else {
                    Ok(())
                }
            })
            .is_err()
    );
    let staged = root.path().join(repair_stage(DIGEST).unwrap().as_os_str());
    assert_eq!(fs::read(&path).unwrap(), b"damaged original");
    assert_eq!(fs::read(&staged).unwrap(), PAYLOAD);
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(&staged, b"unknown staging").unwrap();
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o555)).unwrap();
    assert!(
        destination
            .repair_install(DIGEST, &captured, PAYLOAD, || Ok(()))
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"damaged original");
    assert_eq!(fs::read(&staged).unwrap(), b"unknown staging");
}
