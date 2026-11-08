use super::*;
use std::{fs, os::unix::fs::PermissionsExt};
#[test]
fn management_markers_are_exact_no_clobber_and_keep_directory_owner() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let held = OpenAt2Root::open(temp.path(), MountPolicy::SameMount).unwrap();
    let directory = administrative(&held, temp.path().to_str().unwrap()).unwrap();
    publish_marker(&directory, INTENT, &management_initialization_record()).unwrap();
    publish_marker(&directory, INTENT, &management_initialization_record()).unwrap();
    assert!(publish_marker(&directory, INTENT, b"different").is_err());
    assert_eq!(
        fs::read(temp.path().join(INTENT)).unwrap(),
        management_initialization_record()
    );
    let root = fs::metadata(temp.path()).unwrap();
    let marker = fs::metadata(temp.path().join(INTENT)).unwrap();
    assert_eq!((marker.uid(), marker.gid()), (root.uid(), root.gid()));
    assert_eq!(marker.mode() & 0o7777, 0o600);
}
#[test]
fn unknown_installation_data_is_preserved_and_rebound_names_cannot_receive_marker() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("held");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let held = OpenAt2Root::open(&path, MountPolicy::SameMount).unwrap();
    let directory = administrative(&held, path.to_str().unwrap()).unwrap();
    fs::write(path.join("config.json"), b"existing-pool").unwrap();
    assert!(check_entries(&held, &["setup.lock"]).is_err());
    fs::rename(&path, temp.path().join("original")).unwrap();
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(publish_marker(&directory, AUTHORITY, MANAGEMENT_AUTHORITY_BYTES).is_err());
    assert!(!path.join(AUTHORITY).exists());
    assert_eq!(
        fs::read(temp.path().join("original/config.json")).unwrap(),
        b"existing-pool"
    );
}
