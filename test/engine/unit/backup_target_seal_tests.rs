//! Temporary-root local persistence fixtures only. These are not end-to-end
//! native Backup admission, source unfencing or power-loss acceptance tests.
use super::*;
use crate::persist::layout;
use std::{fs, os::unix::fs::PermissionsExt};

fn target() -> (tempfile::TempDir, DiskRoot, ValidatedConfig, PreparedSeal) {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    let config =
        ValidatedConfig::try_from(Config::development([root.path().to_owned()]).unwrap()).unwrap();
    let mut seal = crate::disk::identity_at(
        &config,
        &config.disks[0],
        &root.directory(root.path()).unwrap(),
    )
    .unwrap();
    seal.format = DiskSealFormat::MembershipGeneration {
        authority_hash: "4".repeat(64),
    };
    let prepared =
        PreparedSeal::new(&"1".repeat(64), &"2".repeat(64), &"3".repeat(64), seal).unwrap();
    (temp, root, config, prepared)
}

#[test]
fn exact_target_seal_repeats_without_selecting_a_generation_or_genesis() {
    let (_temp, root, config, prepared) = target();
    prepared.install(&root).unwrap();
    let original = prepared.observe(&root).unwrap();
    prepared.install(&root).unwrap();
    assert_eq!(prepared.observe(&root).unwrap(), original);
    require_staging(&root, Some(&prepared.intent.native_installation_hash)).unwrap();
    assert_eq!(
        crate::disk::validate_seal(&config, &config.disks[0])
            .unwrap_err()
            .code(),
        ErrorCode::DiskIdentityMismatch
    );
    assert!(
        crate::recognize_disk(root.directory(root.path()).unwrap(), root.path().to_owned())
            .is_err()
    );
    assert!(
        !root
            .path()
            .join("metadata/pool-membership/generation.json")
            .exists()
    );
    assert!(
        !root
            .path()
            .join("metadata/pool-membership/config.json")
            .exists()
    );
}

#[test]
fn matching_but_unowned_disk_json_is_not_adopted() {
    let (_temp, root, _config, prepared) = target();
    install_preparation_bytes(
        &root,
        &root.path().join(layout::DISK_IDENTITY.as_path()),
        &prepared.seal,
    )
    .unwrap();
    assert_eq!(
        prepared.install(&root).unwrap_err().code(),
        ErrorCode::UnsupportedOperation
    );
    assert_eq!(fs::read(root.path().join(layout::DISK_IDENTITY.as_path())).unwrap(), prepared.seal);
    assert!(!root.path().join(LOCAL_INTENT).exists());
    assert!(!root.path().join(BINDING).exists());
}

#[test]
fn unfinished_original_intent_is_read_only_until_explicit_continuation() {
    let (_temp, root, _config, prepared) = target();
    install_preparation_bytes(&root, &root.path().join(LOCAL_INTENT), &prepared.bytes).unwrap();
    require_staging(&root, Some(&prepared.intent.native_installation_hash)).unwrap();
    assert!(prepared.observe(&root).is_err());
    assert!(!root.path().join(layout::DISK_IDENTITY.as_path()).exists());
    let other = PreparedSeal::new(
        &"5".repeat(64),
        &prepared.intent.native_installation_hash,
        &prepared.intent.admission_hash,
        prepared.intent.seal.clone(),
    )
    .unwrap();
    assert!(other.install(&root).is_err());
    assert!(!root.path().join(layout::DISK_IDENTITY.as_path()).exists());
    prepared.install(&root).unwrap();
    prepared.observe(&root).unwrap();
}

#[test]
fn exact_unbound_seal_finishes_the_same_file_identity() {
    let (_temp, root, _config, prepared) = target();
    install_preparation_bytes(&root, &root.path().join(LOCAL_INTENT), &prepared.bytes).unwrap();
    install_preparation_bytes(
        &root,
        &root.path().join(layout::DISK_IDENTITY.as_path()),
        &prepared.seal,
    )
    .unwrap();
    let original = require_seal(&root, &prepared.seal).unwrap();
    require_staging(&root, Some(&prepared.intent.native_installation_hash)).unwrap();
    assert!(prepared.observe(&root).is_err());
    assert!(!root.path().join(BINDING).exists());
    prepared.install(&root).unwrap();
    assert_eq!(prepared.observe(&root).unwrap(), original);
}

#[test]
fn bound_missing_seal_is_not_regenerated() {
    let (_temp, root, _config, prepared) = target();
    prepared.install(&root).unwrap();
    let binding = fs::read(root.path().join(BINDING)).unwrap();
    fs::remove_file(root.path().join(layout::DISK_IDENTITY.as_path())).unwrap();
    assert!(require_staging(&root, Some(&prepared.intent.native_installation_hash)).is_err());
    assert!(prepared.install(&root).is_err());
    assert!(!root.path().join(layout::DISK_IDENTITY.as_path()).exists());
    assert_eq!(fs::read(root.path().join(BINDING)).unwrap(), binding);
}

#[test]
fn replacement_with_identical_seal_bytes_cannot_rebind_the_file() {
    let (_temp, root, _config, prepared) = target();
    prepared.install(&root).unwrap();
    let binding = fs::read(root.path().join(BINDING)).unwrap();
    fs::rename(
        root.path().join(layout::DISK_IDENTITY.as_path()),
        root.path().join("retained-old-seal"),
    )
    .unwrap();
    install_preparation_bytes(
        &root,
        &root.path().join(layout::DISK_IDENTITY.as_path()),
        &prepared.seal,
    )
    .unwrap();
    assert_eq!(
        prepared.observe(&root).unwrap_err().code(),
        ErrorCode::DiskIdentityMismatch
    );
    assert!(prepared.install(&root).is_err());
    assert_eq!(fs::read(root.path().join(BINDING)).unwrap(), binding);
    assert_eq!(
        fs::read(root.path().join("retained-old-seal")).unwrap(),
        prepared.seal
    );
}

#[test]
fn seal_intent_cannot_move_to_another_physical_directory() {
    let (_first_temp, _first, _config, prepared) = target();
    let (_second_temp, second, _second_config, _) = target();
    install_preparation_bytes(&second, &second.path().join(LOCAL_INTENT), &prepared.bytes).unwrap();
    assert_eq!(
        require_staging(&second, Some(&prepared.intent.native_installation_hash))
            .unwrap_err()
            .code(),
        ErrorCode::DiskIdentityMismatch
    );
    assert!(prepared.install(&second).is_err());
    assert!(!second.path().join(layout::DISK_IDENTITY.as_path()).exists());
}

#[test]
fn seal_staging_requires_the_same_native_stage_and_rejects_genesis_format() {
    let (_temp, root, _config, prepared) = target();
    install_preparation_bytes(&root, &root.path().join(LOCAL_INTENT), &prepared.bytes).unwrap();
    assert!(require_staging(&root, None).is_err());
    assert!(require_staging(&root, Some(&"5".repeat(64))).is_err());
    let mut genesis = prepared.intent.seal.clone();
    genesis.format = DiskSealFormat::ManagedDirectory;
    let invalid =
        PreparedSeal::new(&"1".repeat(64), &"2".repeat(64), &"3".repeat(64), genesis).unwrap();
    assert!(invalid.install(&root).is_err());
    assert!(!root.path().join(layout::DISK_IDENTITY.as_path()).exists());
}
