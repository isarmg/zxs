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
    root.private_dir(&root.path().join("metadata/objects"))
        .unwrap();
    assert!(require_unselected_target(&root).is_err());
    select_target(&root, &"1".repeat(64)).unwrap();
    require_unselected_target(&root).unwrap();
    assert!(require_target_selection(&root, &"2".repeat(64)).is_err());
}

#[test]
fn selected_native_stage_cannot_adopt_a_generation_marker_namespace() {
    let (_temp, root) = target();
    select_target(&root, &"1".repeat(64)).unwrap();
    root.private_dir(&root.path().join("metadata/pool-membership"))
        .unwrap();
    let error = require_unselected_target(&root).unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnsupportedOperation);
    assert!(root.path().join("metadata/pool-membership").is_dir());
}

#[test]
fn selected_native_stage_cannot_adopt_a_runtime_seal() {
    let (_temp, root) = target();
    select_target(&root, &"1".repeat(64)).unwrap();
    root.write_atomic(
        &root.path().join("disk.json"),
        b"unrelated runtime seal",
        false,
    )
    .unwrap();
    assert_eq!(
        require_unselected_target(&root).unwrap_err().code(),
        ErrorCode::UnsupportedOperation
    );
    assert_eq!(
        fs::read(root.path().join("disk.json")).unwrap(),
        b"unrelated runtime seal"
    );
}

#[test]
fn ordinary_native_verify_does_not_repair_a_missing_immutable_object() {
    let (_temp, root) = target();
    let config = Config::development(crate::test_disks([root.path().to_owned()])).unwrap();
    let disk = &config.disks[0];
    let object = PreparedMetadataObject::new(
        "state-page",
        &json_codec::json!({"retained": true}),
        MAX_METADATA,
    )
    .unwrap();
    let candidate = "3".repeat(64);
    let set = ObjectSet::new(
        std::iter::once((object.kind(), object.hash.as_str(), object.bytes())),
        &candidate,
        b"retained seal",
    )
    .unwrap();
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
    let error = set
        .inventory_target(&root, &AtomicBool::new(false))
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ObjectCollision);
    assert_eq!(fs::read(unknown).unwrap(), b"keep me");
}

#[test]
fn native_staging_record_does_not_rebind_a_second_directory() {
    let (_first_temp, first) = target();
    let (_second_temp, second) = target();
    select_target(&first, &"1".repeat(64)).unwrap();
    let saved = fs::read(first.path().join(TARGET_SELECTION)).unwrap();
    second
        .write_atomic(&second.path().join(TARGET_SELECTION), &saved, false)
        .unwrap();
    second
        .private_dir(&second.path().join("metadata/objects"))
        .unwrap();
    assert!(require_unselected_target(&second).is_err());
    assert_eq!(
        fs::read(second.path().join(TARGET_SELECTION)).unwrap(),
        saved
    );
}

fn error_signature(error: &Error) -> (ErrorCode, Vec<String>) {
    let mut chain = Vec::new();
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = source {
        chain.push(error.to_string());
        source = error.source();
    }
    (error.code(), chain)
}

#[test]
fn native_recipe_reuses_one_encoding_but_rechecks_conflicting_and_replaced_targets() {
    let (_first_temp, first) = target();
    let (_second_temp, second) = target();
    let config = Config::development(crate::test_disks([
        first.path().to_owned(), second.path().to_owned(),
    ])).unwrap();
    let object = PreparedMetadataObject::new(
        "state-page", &json_codec::json!({"retained": true}), MAX_METADATA,
    ).unwrap();
    let candidate = "3".repeat(64);
    let set = ObjectSet::new(
        std::iter::once((object.kind(), object.hash.as_str(), object.bytes())),
        &candidate, b"retained seal",
    ).unwrap();
    let recipe = set.objects.get(&(object.kind(), object.hash.as_str())).unwrap();
    let cancelled = AtomicBool::new(true);
    let reference = preparation_checkpoint(&cancelled).unwrap_err();
    let observed = set.install(&first, &config.disks[0], &cancelled).unwrap_err();
    assert_eq!(error_signature(&observed), error_signature(&reference));
    assert!(recipe.encoding.get().is_none());
    assert!(!first.path().join("metadata").exists());

    let stop = AtomicBool::new(false);
    set.install(&first, &config.disks[0], &stop).unwrap();
    let first_encoding = recipe.encoding.get().unwrap();
    set.install(&first, &config.disks[0], &stop).unwrap();
    assert!(std::ptr::eq(first_encoding, recipe.encoding.get().unwrap()));
    assert_eq!(set.objects.values().filter(|item| item.encoding.get().is_some()).count(), 1);

    let path = crate::journal::object_path(&config.disks[1], object.kind(), &object.hash);
    second.private_dir(path.parent().unwrap()).unwrap();
    second.write_atomic(&path, b"conflicting target bytes", false).unwrap();
    let reference = object.store_at_root(&second, &config.disks[1]).unwrap_err();
    let observed = set.install(&second, &config.disks[1], &stop).unwrap_err();
    assert_eq!(error_signature(&observed), error_signature(&reference));
    assert_eq!(fs::read(&path).unwrap(), b"conflicting target bytes");
    assert!(!set.seal_path(&config.disks[1]).exists());
    assert!(std::ptr::eq(first_encoding, recipe.encoding.get().unwrap()));

    fs::remove_file(&path).unwrap();
    set.install(&second, &config.disks[1], &stop).unwrap();
    set.verify(&second, &config.disks[1], &stop).unwrap();
    assert!(std::ptr::eq(first_encoding, recipe.encoding.get().unwrap()));
    assert_eq!(fs::read(&path).unwrap(), object.bytes());

    let changed = br#"{"retained":false}"#;
    fs::write(&path, changed).unwrap();
    assert_eq!(set.verify(&second, &config.disks[1], &stop).unwrap_err().code(),
        ErrorCode::ObjectCorrupt);
    assert_eq!(fs::read(&path).unwrap(), changed);
    fs::write(&path, object.bytes()).unwrap();

    // Replacing the second named root cannot reuse the first root's success.
    let old_path = second.path().to_owned();
    let parked = old_path.with_extension("parked");
    fs::rename(&old_path, &parked).unwrap();
    fs::create_dir(&old_path).unwrap();
    fs::set_permissions(&old_path, fs::Permissions::from_mode(0o700)).unwrap();
    let reference = object.store_at_root(&second, &config.disks[1]).unwrap_err();
    let observed = set.install(&second, &config.disks[1], &stop).unwrap_err();
    assert_eq!(error_signature(&observed), error_signature(&reference));
    assert_eq!(fs::read_dir(&old_path).unwrap().count(), 0);
    assert_eq!(
        fs::read(parked.join(path.strip_prefix(&old_path).unwrap())).unwrap(),
        object.bytes(),
    );
    fs::remove_dir(&old_path).unwrap();
    fs::rename(&parked, &old_path).unwrap();
    set.verify(&second, &config.disks[1], &stop).unwrap();
}

#[test]
fn native_recipe_failure_keeps_original_per_object_write_and_error_order() {
    let (_temp, root) = target();
    let config = Config::development(crate::test_disks([root.path().to_owned()])).unwrap();
    let disk = &config.disks[0];
    let object = PreparedMetadataObject::new(
        "state-page", &json_codec::json!({"retained": true}), MAX_METADATA,
    ).unwrap();
    let candidate = "3".repeat(64);
    let bad_hash = "f".repeat(64);
    assert!(object.hash < bad_hash);
    let set = ObjectSet::new(
        [
            (object.kind(), object.hash.as_str(), object.bytes()),
            (object.kind(), bad_hash.as_str(), b"{bad".as_slice()),
        ].into_iter(),
        &candidate, b"retained seal",
    ).unwrap();
    let stop = AtomicBool::new(false);
    let reference = storage_contract::strict_json(b"{bad").map_err(Error::from).unwrap_err();
    let observed = set.install(&root, disk, &stop).unwrap_err();
    assert_eq!(error_signature(&observed), error_signature(&reference));
    let path = crate::journal::object_path(disk, object.kind(), &object.hash);
    assert_eq!(fs::read(&path).unwrap(), object.bytes());
    assert!(set.objects.get(&(object.kind(), object.hash.as_str())).unwrap()
        .encoding.get().is_some());
    assert!(set.objects.get(&(object.kind(), bad_hash.as_str())).unwrap()
        .encoding.get().is_none());
    assert!(!set.seal_path(disk).exists());

    // A later malformed recipe cannot preempt an earlier current disk fault,
    // including when that earlier object's pure encoding was already retained.
    fs::write(&path, b"first object changed").unwrap();
    let reference = object.store_at_root(&root, disk).unwrap_err();
    let observed = set.install(&root, disk, &stop).unwrap_err();
    assert_eq!(error_signature(&observed), error_signature(&reference));
    assert_eq!(fs::read(&path).unwrap(), b"first object changed");
    assert!(!set.seal_path(disk).exists());
}
