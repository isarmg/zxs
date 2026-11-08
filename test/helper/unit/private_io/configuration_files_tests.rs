use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

#[test]
fn configuration_files_never_adopt_different_bytes_modes_or_links() {
    let temp = tempfile::tempdir().unwrap();
    let root = OpenAt2Root::open(temp.path(), MountPolicy::SameMount).unwrap();
    let target = temp.path().join("zxs.service");
    fs::write(&target, b"actual").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matching(&root, "zxs.service", b"other!").is_err());
    fs::set_permissions(&target, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(matching(&root, "zxs.service", b"actual").is_err());
    assert_eq!(fs::read(&target).unwrap(), b"actual");
    symlink(&target, temp.path().join("linked")).unwrap();
    assert!(matching(&root, "linked", b"actual").is_err());
    fs::hard_link(&target, temp.path().join("hard")).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matching(&root, "hard", b"actual").is_err());
}

#[test]
fn exact_file_matching_requires_actual_root_ownership_not_only_readability() {
    let temp = tempfile::tempdir().unwrap();
    let root = OpenAt2Root::open(temp.path(), MountPolicy::SameMount).unwrap();
    let target = temp.path().join("zxs.service");
    fs::write(&target, b"unit").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    if rustix::process::geteuid().as_raw() == 0 {
        assert!(matching(&root, "zxs.service", b"unit").unwrap().is_some());
        assert!(!publish(&root, "zxs.service", b"unit").unwrap());
        assert!(publish(&root, "new-unit", b"new").unwrap());
        assert_eq!(
            fs::metadata(temp.path().join("new-unit")).unwrap().mode() & 0o7777,
            0o644
        );
    } else {
        assert!(matching(&root, "zxs.service", b"unit").is_err());
        assert!(trusted(root.directory()).is_err());
    }
    assert_eq!(fs::read(target).unwrap(), b"unit");
}

#[test]
fn configuration_absence_and_budget_are_not_compatibility_or_implicit_creation() {
    let temp = tempfile::tempdir().unwrap();
    let root = OpenAt2Root::open(temp.path(), MountPolicy::SameMount).unwrap();
    assert!(matching(&root, "absent", b"content").unwrap().is_none());
    assert!(matching(&root, "absent", &vec![0; MAX_CONFIG_BYTES + 1]).is_err());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}
