use super::*;
use std::{ffi::OsStr, fs, os::unix::fs::symlink};
#[test]
fn root_is_published_without_replacement_and_matching_retry_keeps_content() {
    // A parallel test's fork can retain a CLOEXEC lease until its exec boundary.
    if std::env::var_os("ZXS_HOST_ROOT_LAST_OWNER_TEST_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "private_io::host_root::tests::root_is_published_without_replacement_and_matching_retry_keeps_content",
                "--test-threads=1",
            ])
            .env("ZXS_HOST_ROOT_LAST_OWNER_TEST_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let parent = tempfile::tempdir().unwrap();
    let root = HostRoot::open(parent.path(), OsStr::new("zxs")).unwrap();
    assert!(!root.present().unwrap());
    let (created, lease) = root.prepare().unwrap();
    let identity = lease.identity();
    assert!(
        platform_io::service_lease::ServiceLease::acquire(
            &parent.path().join("zxs"),
            rustix::process::geteuid().as_raw(),
        )
        .is_err()
    );
    assert!(created);
    fs::write(parent.path().join("zxs/retain"), b"data").unwrap();
    drop(lease);
    let (created, again) = root.prepare().unwrap();
    assert!(!created);
    assert_eq!(again.identity(), identity);
    assert_eq!(fs::read(parent.path().join("zxs/retain")).unwrap(), b"data");
    assert!(!parent.path().join(".zxs-helper-host.pending").exists());
}
#[test]
fn pending_empty_root_can_resume_but_unrecognized_material_is_preserved() {
    let parent = tempfile::tempdir().unwrap();
    let pending = parent.path().join(".zxs-helper-host.pending");
    fs::create_dir(&pending).unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o755)).unwrap();
    let root = HostRoot::open(parent.path(), OsStr::new("zxs")).unwrap();
    fs::write(pending.join("unknown"), b"retain").unwrap();
    assert!(root.prepare().is_err());
    assert_eq!(fs::read(pending.join("unknown")).unwrap(), b"retain");
    fs::remove_file(pending.join("unknown")).unwrap();
    assert!(root.prepare().unwrap().0);
}
#[test]
fn existing_unsafe_or_linked_roots_are_not_repaired_or_replaced() {
    let parent = tempfile::tempdir().unwrap();
    let root = HostRoot::open(parent.path(), OsStr::new("zxs")).unwrap();
    let elsewhere = parent.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    symlink(&elsewhere, parent.path().join("zxs")).unwrap();
    assert!(root.prepare().is_err());
    assert!(
        fs::symlink_metadata(parent.path().join("zxs"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
