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
        assert!(
            !publish(&root, "zxs.service", b"unit", false)
                .unwrap()
                .created
        );
        assert!(publish(&root, "new-unit", b"new", false).unwrap().created);
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

#[test]
fn completed_configuration_cannot_be_refilled_and_a_live_receipt_rejects_a_same_byte_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let root = OpenAt2Root::open(temp.path(), MountPolicy::SameMount).unwrap();
    assert!(publish(&root, "zxs.conf", b"config", true).is_err());
    assert!(!temp.path().join("zxs.conf").exists());
    if rustix::process::geteuid().as_raw() != 0 {
        return;
    }
    let held = publish(&root, "zxs.conf", b"config", false).unwrap();
    assert!(held.created);
    fs::rename(temp.path().join("zxs.conf"), temp.path().join("retained")).unwrap();
    assert!(publish(&root, "zxs.conf", b"config", true).is_err());
    assert!(!temp.path().join("zxs.conf").exists());
    fs::write(temp.path().join("zxs.conf"), b"config").unwrap();
    fs::set_permissions(
        temp.path().join("zxs.conf"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(held.require_current().is_err());
    assert_eq!(fs::read(temp.path().join("retained")).unwrap(), b"config");
}

#[test]
fn a_late_same_byte_configuration_is_not_adopted_by_an_earlier_absent_capture() {
    if rustix::process::geteuid().as_raw() != 0 {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = OpenAt2Root::open(temp.path(), MountPolicy::SameMount).unwrap();
    let before = matching(&root, "zxs.service", b"same").unwrap();
    assert!(before.is_none());
    let first = publish(&root, "zxs.service", b"same", false).unwrap();
    assert!(publish_captured(&root, "zxs.service", b"same", before, false).is_err());
    first.require_current().unwrap();
    assert_eq!(fs::read(temp.path().join("zxs.service")).unwrap(), b"same");
}
