//! Ordinary immutable metadata and its unselected-target boundary. Temporary
//! files only; these fixtures do not claim an Engine cutover or writer test.
use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

fn target() -> (tempfile::TempDir, DiskRoot) {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    (temp, root)
}

#[test]
fn unselected_native_namespace_requires_its_local_selection() {
    let (_temp, root) = target();
    require_unselected_target(&root).unwrap();
    root.private_dir(&root.path().join("metadata/objects")).unwrap();
    assert!(require_unselected_target(&root).is_err());
    select_target(&root, &"1".repeat(64)).unwrap();
    require_unselected_target(&root).unwrap();
    assert!(require_target_selection(&root, &"2".repeat(64)).is_err());
}

#[test]
fn selected_native_stage_cannot_adopt_a_generation_marker_namespace() {
    let (_temp, root) = target();
    select_target(&root, &"1".repeat(64)).unwrap();
    root.private_dir(&root.path().join("metadata/pool-membership")).unwrap();
    let error = require_unselected_target(&root).unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnsupportedOperation);
    assert!(root.path().join("metadata/pool-membership").is_dir());
}

#[test]
fn selected_native_stage_cannot_adopt_a_runtime_seal() {
    let (_temp, root) = target();
    select_target(&root, &"1".repeat(64)).unwrap();
    root.write_atomic(&root.path().join("disk.json"), b"unrelated runtime seal", false).unwrap();
    assert_eq!(require_unselected_target(&root).unwrap_err().code(), ErrorCode::UnsupportedOperation);
    assert_eq!(fs::read(root.path().join("disk.json")).unwrap(), b"unrelated runtime seal");
}

#[test]
fn ordinary_native_verify_does_not_repair_a_missing_immutable_object() {
    let (_temp, root) = target();
    let config = Config::development([root.path().to_owned()]).unwrap();
    let disk = &config.disks[0];
    let object = PreparedMetadataObject::new("state-page", &json_codec::json!({"retained": true}), MAX_METADATA).unwrap();
    let candidate = "3".repeat(64);
    let set = ObjectSet::new(std::iter::once((object.kind(), object.hash.as_str(), object.bytes())), &candidate, b"retained seal").unwrap();
    select_target(&root, &"1".repeat(64)).unwrap();
    let stop = AtomicBool::new(false);
    set.install(&root, disk, &stop).unwrap();
    set.verify(&root, disk, &stop).unwrap();
    set.inventory_target(&root, &stop).unwrap();
    let path = crate::journal::object_path(disk, object.kind(), &object.hash);
    fs::remove_file(&path).unwrap();
    assert!(set.verify(&root, disk, &stop).is_err());
    assert!(!path.exists());
}

#[test]
fn native_target_inventory_preserves_unknown_files() {
    let (_temp, root) = target();
    select_target(&root, &"1".repeat(64)).unwrap();
    let candidate = "3".repeat(64);
    let set = ObjectSet::new(std::iter::empty(), &candidate, b"retained seal").unwrap();
    let unknown = root.path().join("metadata/objects/not-owned");
    root.private_dir(unknown.parent().unwrap()).unwrap();
    root.write_atomic(&unknown, b"keep me", false).unwrap();
    let error = set.inventory_target(&root, &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ObjectCollision);
    assert_eq!(fs::read(unknown).unwrap(), b"keep me");
}

#[test]
fn native_staging_record_does_not_rebind_a_second_directory() {
    let (_first_temp, first) = target();
    let (_second_temp, second) = target();
    select_target(&first, &"1".repeat(64)).unwrap();
    let saved = fs::read(first.path().join(TARGET_SELECTION)).unwrap();
    second.write_atomic(&second.path().join(TARGET_SELECTION), &saved, false).unwrap();
    second.private_dir(&second.path().join("metadata/objects")).unwrap();
    assert!(require_unselected_target(&second).is_err());
    assert_eq!(fs::read(second.path().join(TARGET_SELECTION)).unwrap(), saved);
}
