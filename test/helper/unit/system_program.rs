use super::*;
use std::os::unix::fs::PermissionsExt;
#[test]
fn program_selection_is_fixed_and_has_no_shell_or_caller_path() {
    assert_eq!(
        Program::SystemdSysusers.location(),
        ("bin", "systemd-sysusers", "/usr/bin/systemd-sysusers")
    );
    assert_eq!(
        Program::Testparm.location(),
        ("bin", "testparm", "/usr/bin/testparm")
    );
    assert_eq!(
        Program::Pdbedit.location(),
        ("bin", "pdbedit", "/usr/bin/pdbedit")
    );
    assert_eq!(
        Program::Smbpasswd.location(),
        ("bin", "smbpasswd", "/usr/bin/smbpasswd")
    );
}
#[test]
fn executable_snapshot_rejects_shell_wrappers_without_running_them() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("program");
    std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let file = File::open(path).unwrap();
    assert!(snapshot(&file, "fixture-program").is_err());
}
#[test]
fn file_stamp_changes_when_payload_length_or_permissions_change() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("existing");
    std::fs::write(&path, b"one").unwrap();
    let first = FileStamp::of(&std::fs::metadata(&path).unwrap());
    std::fs::write(&path, b"second").unwrap();
    let second = FileStamp::of(&std::fs::metadata(&path).unwrap());
    assert!(first != second);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let protected = FileStamp::of(&std::fs::metadata(&path).unwrap());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(protected != FileStamp::of(&std::fs::metadata(&path).unwrap()));
}
