//! Mixed SYSTEM/original/target final-record behavior only. No synthetic
//! record in this fixture grants a native Publication or admission.
use super::*;
use std::{fs, os::unix::fs::{MetadataExt, PermissionsExt}};

fn root() -> (tempfile::TempDir, DiskRoot) {
    let tmp = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700))
        .tempdir().unwrap();
    let held = DiskRoot::open(tmp.path()).unwrap();
    (tmp, held)
}

#[test]
fn ordinary_tail_fills_only_missing_copies_regardless_of_participant_write_order() {
    let (_a, system) = root();
    let (_b, original) = root();
    let (_c, target) = root();
    let bytes = b"actual complete common record";
    let retained = original.path().join(COMPLETE);
    original.write_atomic(&retained, bytes, false).unwrap();
    let before = fs::metadata(&retained).unwrap().ino();
    let mut tail = Completions::default();
    for member in [&system, &original, &target] { tail.observe(member).unwrap(); }
    assert!(tail.any());
    for member in [&system, &original, &target] { tail.review(member, bytes).unwrap(); }
    assert!(!target.path().join(COMPLETE).exists());
    assert!(!system.path().join(COMPLETE).exists());
    for member in [&target, &original, &system] { tail.finish(member, bytes).unwrap(); }
    for member in [&system, &original, &target] { tail.verify(member, bytes).unwrap(); }
    assert_eq!(fs::metadata(retained).unwrap().ino(), before);
}

#[test]
fn ordinary_tail_does_not_recreate_a_lost_or_equal_replaced_observed_receipt() {
    let (_a, peer) = root();
    let (_b, target) = root();
    let (_c, other) = root();
    let bytes = b"complete set";
    let named = target.path().join(COMPLETE);
    target.write_atomic(&named, bytes, false).unwrap();
    let mut tail = Completions::default();
    tail.observe(&target).unwrap();
    tail.observe(&peer).unwrap();
    assert!(tail.observe(&target).is_err());
    tail.review(&target, bytes).unwrap();
    tail.review(&peer, bytes).unwrap();
    let held = fs::File::open(&named).unwrap();
    target.write_atomic(&named, bytes, true).unwrap();
    assert_ne!(fs::metadata(&named).unwrap().ino(), held.metadata().unwrap().ino());
    assert!(tail.review(&target, bytes).is_err());
    assert!(tail.finish(&target, bytes).is_err());
    assert_eq!(fs::read(&named).unwrap(), bytes);
    fs::remove_file(&named).unwrap();
    assert!(tail.finish(&target, bytes).is_err());
    assert!(!named.exists());
    assert!(tail.finish(&other, bytes).is_err());
    assert!(!other.path().join(COMPLETE).exists());
    assert!(!peer.path().join(COMPLETE).exists());
}
