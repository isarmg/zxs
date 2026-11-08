use super::*;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
};
fn owner() -> (u32, u32) {
    (
        rustix::process::geteuid().as_raw(),
        rustix::process::getegid().as_raw(),
    )
}
#[test]
fn layout_is_one_noclobber_tree_and_only_pristine_matching_retry_is_allowed() {
    let parent = tempfile::tempdir().unwrap();
    let destination = Destination::open(parent.path(), owner()).unwrap();
    assert!(!destination.matching_or_absent().unwrap());
    assert!(destination.publish("digest", || Ok(())).unwrap());
    assert!(!destination.publish("digest", || Ok(())).unwrap());
    for name in CHILDREN {
        let path = parent.path().join("data").join(name);
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!((metadata.uid(), metadata.gid()), owner());
        assert_eq!(metadata.mode() & 0o7777, 0o700);
        assert!(fs::read_dir(path).unwrap().next().is_none());
    }
    fs::write(parent.path().join("data/state/keep"), b"data").unwrap();
    assert!(destination.publish("digest", || Ok(())).is_err());
    assert_eq!(
        fs::read(parent.path().join("data/state/keep")).unwrap(),
        b"data"
    );
}
#[test]
fn final_links_and_unsafe_permissions_are_not_repaired() {
    let parent = tempfile::tempdir().unwrap();
    let destination = Destination::open(parent.path(), owner()).unwrap();
    let actual = parent.path().join("other");
    fs::create_dir(&actual).unwrap();
    symlink(&actual, parent.path().join("data")).unwrap();
    assert!(destination.matching_or_absent().is_err());
    fs::remove_file(parent.path().join("data")).unwrap();
    fs::create_dir(parent.path().join("data")).unwrap();
    fs::set_permissions(
        parent.path().join("data"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(destination.publish("digest", || Ok(())).is_err());
    assert_eq!(
        fs::metadata(parent.path().join("data")).unwrap().mode() & 0o7777,
        0o755
    );
}
#[test]
fn missing_pending_children_resume_and_lost_identity_refuses_publication() {
    let parent = tempfile::tempdir().unwrap();
    let pending = parent.path().join(".zxs-helper-data-digest.pending");
    fs::create_dir(&pending).unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o700)).unwrap();
    let child = pending.join("state");
    fs::create_dir(&child).unwrap();
    fs::set_permissions(child, fs::Permissions::from_mode(0o700)).unwrap();
    let destination = Destination::open(parent.path(), owner()).unwrap();
    assert!(
        destination
            .publish("digest", || Err("account changed".into()))
            .is_err()
    );
    assert!(!parent.path().join("data").exists());
    assert!(pending.is_dir());
    assert!(destination.publish("digest", || Ok(())).unwrap());
}
