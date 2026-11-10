use super::*;
use std::os::unix::fs::PermissionsExt;

fn root() -> (tempfile::TempDir, DiskRoot) {
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    (temp, root)
}

#[test]
fn absent_record_is_only_allowed_during_preflight() {
    let (_temp, root) = root();
    let record = RootRecord {
        name: "completion.json",
        bytes: b"complete",
        maximum: 8,
    };
    record.check_existing(&root).unwrap();
    let expected = root
        .read_bytes(&root.path().join(record.name), record.maximum)
        .unwrap_err();
    let actual = record.require(&root).unwrap_err();
    assert_eq!(actual.code(), expected.code());
    assert_eq!(actual.to_string(), expected.to_string());
    assert!(!root.path().join(record.name).exists());
}

#[test]
fn installation_is_idempotent_and_survives_reopen() {
    let (_temp, root) = root();
    let record = RootRecord {
        name: "completion.json",
        bytes: b"complete",
        maximum: 8,
    };
    record.install(&root).unwrap();
    record.install(&root).unwrap();
    record.check_existing(&root).unwrap();
    record
        .require(&DiskRoot::open(root.path()).unwrap())
        .unwrap();
}

#[test]
fn mismatches_keep_preflight_and_observation_error_codes() {
    let (_temp, root) = root();
    let record = RootRecord {
        name: "completion.json",
        bytes: b"new",
        maximum: 3,
    };
    let path = root.path().join(record.name);
    root.write_atomic(&path, b"old", false).unwrap();
    assert_eq!(
        record.check_existing(&root).unwrap_err().code(),
        ErrorCode::ObjectCollision
    );
    assert_eq!(
        record.require(&root).unwrap_err().code(),
        ErrorCode::ConfigConflict
    );
    assert_eq!(
        record.install(&root).unwrap_err().code(),
        ErrorCode::ObjectCollision
    );
    assert_eq!(root.read_bytes(&path, 3).unwrap(), b"old");
}

#[test]
fn reader_budget_is_not_replaced_by_expected_length() {
    let (_temp, root) = root();
    let record = RootRecord {
        name: "completion.json",
        bytes: b"new",
        maximum: 8,
    };
    let path = root.path().join(record.name);
    root.write_atomic(&path, b"old-long", false).unwrap();
    assert_eq!(
        record.require(&root).unwrap_err().code(),
        ErrorCode::ConfigConflict
    );
    assert_eq!(
        record.check_existing(&root).unwrap_err().code(),
        ErrorCode::ObjectCollision
    );
    let exact = RootRecord {
        maximum: record.bytes.len(),
        ..record
    };
    assert_eq!(
        exact.require(&root).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(
        exact.check_existing(&root).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(root.read_bytes(&path, 8).unwrap(), b"old-long");
}

#[test]
fn unsafe_record_path_is_rejected_without_touching_its_target() {
    let (_temp, root) = root();
    let record = RootRecord {
        name: "completion.json",
        bytes: b"new",
        maximum: 3,
    };
    let target = root.path().join("preserved.json");
    root.write_atomic(&target, b"old", false).unwrap();
    let path = root.path().join(record.name);
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let expected = root.read_bytes(&path, record.maximum).unwrap_err();
    let actual = record.require(&root).unwrap_err();
    assert_eq!(actual.code(), expected.code());
    assert_eq!(actual.to_string(), expected.to_string());
    assert!(record.check_existing(&root).is_err());
    assert!(record.install(&root).is_err());
    assert_eq!(root.read_bytes(&target, 3).unwrap(), b"old");
    assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink());
}
