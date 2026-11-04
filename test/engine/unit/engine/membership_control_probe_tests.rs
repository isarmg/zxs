use super::*;
use crate::engine::membership_transition::pending::admission::verify_named_root;
use std::fs;

#[test]
fn membership_control_probe_closed_binding_rejects_before_unsafe_named_root() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("member")]).unwrap();
    let engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let handle = engine.disk_resources.get(&config.disks[0]).unwrap();
    let binding = handle.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    binding.closed_proof().unwrap();
    let saved = temp.path().join("closed-member");
    fs::rename(&config.disks[0].root, &saved).unwrap();
    std::os::unix::fs::symlink(temp.path().join("forbidden-name"), &config.disks[0].root).unwrap();
    let seal = fs::read(saved.join("disk.json")).unwrap();
    let archive = temp.path().join("archive");
    private_dir(&archive).unwrap();
    assert_eq!(
        verify_named_root(&handle.root).unwrap_err().code(),
        ErrorCode::DiskUnavailable
    );
    assert!(
        matches!(engine.admit_control_archive_directory(&archive), Err(error) if error.code() == ErrorCode::DiskUnavailable)
    );
    assert_eq!(fs::read(saved.join("disk.json")).unwrap(), seal);
    assert_eq!(fs::read_dir(&archive).unwrap().count(), 0);
    binding.closed_proof().unwrap();
}

#[test]
fn membership_control_probe_active_moved_root_retains_exact_identity_failure() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("member")]).unwrap();
    let engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let handle = engine.disk_resources.get(&config.disks[0]).unwrap();
    let before = verify_named_root(&handle.root).unwrap();
    let saved = temp.path().join("admitted-member");
    fs::rename(&config.disks[0].root, &saved).unwrap();
    private_dir(&config.disks[0].root).unwrap();
    let seal = fs::read(saved.join("disk.json")).unwrap();
    let archive = temp.path().join("archive");
    private_dir(&archive).unwrap();
    assert_eq!(
        verify_named_root(&handle.root).unwrap_err().code(),
        ErrorCode::DiskIdentityMismatch
    );
    assert!(
        matches!(engine.admit_control_archive_directory(&archive), Err(error) if error.code() == ErrorCode::DiskIdentityMismatch)
    );
    assert_eq!(
        FileIdentity::from_fd(handle.root.directory(handle.root.path()).unwrap()).unwrap(),
        before
    );
    assert_eq!(fs::read(saved.join("disk.json")).unwrap(), seal);
    assert_eq!(fs::read_dir(&config.disks[0].root).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&archive).unwrap().count(), 0);
}
