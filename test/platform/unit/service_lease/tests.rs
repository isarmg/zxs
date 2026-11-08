use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

#[test]
fn one_inode_excludes_another_owner_and_alias_until_guard_drop() {
    let root = tempfile::tempdir().unwrap();
    let uid = rustix::process::geteuid().as_raw();
    let owner = ServiceLease::acquire(root.path(), uid).unwrap();
    assert!(matches!(
        ServiceLease::acquire(root.path(), uid),
        Err(Error::Lock { .. })
    ));
    let alias = root.path().join(".");
    assert!(ServiceLease::acquire(&alias, uid).is_err());
    owner.recheck().unwrap();
    drop(owner);
    ServiceLease::acquire(root.path(), uid).unwrap();
}
#[test]
fn missing_linked_wrong_owner_and_writable_roots_are_rejected_without_creation() {
    let parent = tempfile::tempdir().unwrap();
    let uid = rustix::process::geteuid().as_raw();
    let root = parent.path().join("root");
    assert!(ServiceLease::acquire(&root, uid).is_err());
    assert!(!root.exists());
    fs::create_dir(&root).unwrap();
    assert!(ServiceLease::acquire(&root, uid.wrapping_add(1)).is_err());
    let linked = parent.path().join("linked");
    symlink(&root, &linked).unwrap();
    assert!(ServiceLease::acquire(&linked, uid).is_err());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(matches!(
        ServiceLease::acquire(&root, uid),
        Err(Error::Permissions { .. })
    ));
}
#[test]
fn path_replacement_does_not_silently_transfer_the_held_inode_proof() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    fs::create_dir(&root).unwrap();
    let uid = rustix::process::geteuid().as_raw();
    let guard = ServiceLease::acquire(&root, uid).unwrap();
    fs::rename(&root, parent.path().join("retained")).unwrap();
    fs::create_dir(&root).unwrap();
    assert!(matches!(
        guard.recheck(),
        Err(Error::IdentityChanged { .. })
    ));
}
