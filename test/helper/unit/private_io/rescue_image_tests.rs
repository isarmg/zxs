//! Deterministic boundary cases for the frozen reader.
use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

fn image(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn publication_and_cold_readback_report_the_same_exact_owned_inode_without_rewriting() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("copy.recovered.sqlite");
    let first = publish_rescue_sqlite(&path, b"metadata", 64, |bytes| {
        assert_eq!(bytes, b"metadata");
        Ok(())
    })
    .unwrap();
    let names = fs::read_dir(temp.path()).unwrap().count();
    let cold = verify_rescue_sqlite(&path, 64, |bytes| {
        assert_eq!(bytes, b"metadata");
        Ok(())
    })
    .unwrap();
    assert_eq!(first.device, cold.device);
    assert_eq!(first.inode, cold.inode);
    assert_eq!(first.sha256, cold.sha256);
    assert_eq!(cold.sha256, rescue_digest(b"metadata"));
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o600);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), names);
    assert!(publish_rescue_sqlite(&path, b"replacement", 64, |_| Ok(())).is_err());
    assert_eq!(fs::read(path).unwrap(), b"metadata");
}

#[test]
fn failed_postpublication_semantic_check_preserves_output_and_reports_partial_success() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("copy.recovered.sqlite");
    let error = publish_rescue_sqlite(&path, b"metadata", 64, |_| {
        Err("different selected records".into())
    })
    .unwrap_err();
    let failure = error.downcast_ref::<PublishedUnverified>().unwrap();
    assert_eq!(failure.path, path);
    assert_eq!(failure.identity.inode, fs::metadata(&path).unwrap().ino());
    assert_eq!(fs::read(&path).unwrap(), b"metadata");
    verify_rescue_sqlite(&path, 64, |_| Ok(())).unwrap();
}

#[test]
fn preexisting_unknown_sidecar_blocks_publication_and_is_not_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("copy.recovered.sqlite");
    let sidecar = temp.path().join("copy.recovered.sqlite-wal");
    fs::create_dir(&sidecar).unwrap();
    assert!(publish_rescue_sqlite(&path, b"metadata", 64, |_| Ok(())).is_err());
    assert!(!path.exists());
    assert!(sidecar.is_dir());
}

#[test]
fn replacing_the_image_with_identical_bytes_during_validation_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("copy.recovered.sqlite");
    let retained = temp.path().join("original.recovered.sqlite");
    image(&path, b"metadata");
    assert!(
        verify_rescue_sqlite(&path, 64, |_| {
            fs::rename(&path, &retained)?;
            image(&path, b"metadata");
            Ok(())
        })
        .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), b"metadata");
    assert_eq!(fs::read(retained).unwrap(), b"metadata");
}

#[test]
fn changing_same_inode_bytes_during_validation_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("copy.recovered.sqlite");
    image(&path, b"metadata");
    assert!(
        verify_rescue_sqlite(&path, 64, |_| {
            fs::write(&path, b"changed!")?;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), b"changed!");
}

#[test]
fn new_sidecar_after_decode_is_preserved_and_invalidates_readback() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("copy.recovered.sqlite");
    let sidecar = temp.path().join("copy.recovered.sqlite-journal");
    image(&path, b"metadata");
    assert!(
        verify_rescue_sqlite(&path, 64, |_| {
            fs::write(&sidecar, b"journal")?;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(fs::read(sidecar).unwrap(), b"journal");
    assert_eq!(fs::read(path).unwrap(), b"metadata");
}

#[test]
fn rebinding_a_parent_name_during_decode_cannot_attest_the_old_path() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("selected");
    let retained = temp.path().join("retained");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("copy.recovered.sqlite");
    image(&path, b"metadata");
    assert!(
        verify_rescue_sqlite(&path, 64, |_| {
            fs::rename(&parent, &retained)?;
            fs::create_dir(&parent)?;
            image(&path, b"metadata");
            Ok(())
        })
        .is_err()
    );
    assert!(retained.join("copy.recovered.sqlite").exists());
    assert!(path.exists());
}

#[test]
fn unsafe_links_modes_and_overbudget_images_are_never_changed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("copy.recovered.sqlite");
    image(&path, b"metadata");
    assert!(verify_rescue_sqlite(&path, 7, |_| Ok(())).is_err());
    let alias = temp.path().join("alias.recovered.sqlite");
    symlink(&path, &alias).unwrap();
    assert!(verify_rescue_sqlite(&alias, 64, |_| Ok(())).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o620)).unwrap();
    assert!(verify_rescue_sqlite(&path, 64, |_| Ok(())).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&path, temp.path().join("hard.recovered.sqlite")).unwrap();
    assert!(verify_rescue_sqlite(&path, 64, |_| Ok(())).is_err());
    assert_eq!(fs::read(path).unwrap(), b"metadata");
}
