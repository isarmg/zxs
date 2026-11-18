//! No system path is opened. Real held descriptors, no service operations.
use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn same_bytes_replaced_inode_and_open_name_loss_do_not_reauthenticate() {
    if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 {
        return; // The production proof specifically requires uid 0.
    }
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("zxs.service");
    let secret = b"not-a-configuration-template: keep this original private";
    fs::write(&path, secret).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let parent = Parent::open("test-parent", OpenAt2Root::open(tmp.path(), MountPolicy::SameMount).unwrap()).unwrap();
    let kept = HeldOriginal::capture(&parent, Target::SystemdService).unwrap();
    assert!(kept.require(&parent).is_ok());
    let description = json_codec::to_string(&kept.original).unwrap();
    assert!(!description.contains(std::str::from_utf8(secret).unwrap()));
    assert!(description.contains(&platform_io::lower_hex(&Sha256::digest(secret))));
    fs::rename(&path, tmp.path().join("held-original")).unwrap();
    assert!(kept.require(&parent).is_err());
    fs::write(&path, secret).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(kept.require(&parent).is_err());
}

#[test]
fn unknown_mode_hardlinks_and_symlinks_are_not_a_preview_source() {
    if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 { return; }
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("zxs.conf");
    fs::write(&path, b"fixed fixture").unwrap();
    let parent = Parent::open("test-parent", OpenAt2Root::open(tmp.path(), MountPolicy::SameMount).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(HeldOriginal::capture(&parent, Target::SambaConfiguration).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::hard_link(&path,tmp.path().join("other")).unwrap();
    assert!(HeldOriginal::capture(&parent, Target::SambaConfiguration).is_err());
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("other", &path).unwrap();
    assert!(HeldOriginal::capture(&parent, Target::SambaConfiguration).is_err());
}
