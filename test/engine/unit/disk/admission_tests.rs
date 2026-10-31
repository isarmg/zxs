use super::*;
use std::process::Command;

#[test]
fn typed_parent_io_handles_root_children_and_missing_nested_directories() {
    let temporary = tempfile::tempdir().unwrap();
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = DiskRoot::open(temporary.path()).unwrap();
    root.private_dir(temporary.path()).unwrap();
    assert!(matches!(
        root.write_atomic(temporary.path(), b"not a child", false),
        Err(Error::Filesystem(
            platform_io::fs_safety::Error::UnsafeRelativePath(_)
        ))
    ));
    for relative in ["root-child", "missing/nested/leaf"] {
        let path = temporary.path().join(relative);
        root.write_atomic(&path, b"owned", false).unwrap();
        assert_eq!(root.read_bytes(&path, 5).unwrap(), b"owned");
        assert!(root.open_optional_regular(&path).unwrap().is_some());
        assert!(root.optional_metadata(&path).unwrap().unwrap().is_file());
        root.remove_file(&path).unwrap();
        assert!(root.open_optional_regular(&path).unwrap().is_none());
    }
    assert_eq!(
        fs::metadata(temporary.path().join("missing/nested"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );
}

#[test]
fn immutable_existing_bytes_keep_collision_and_leaf_absence_semantics() {
    let temporary = tempfile::tempdir().unwrap();
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = temporary.path().join("object");
    let root = DiskRoot::open(temporary.path()).unwrap();
    root.write_atomic(&path, b"original", false).unwrap();
    root.write_atomic(&path, b"original", false).unwrap();
    for bytes in [b"different".as_slice(), b"short", b"original and more"] {
        let error = root.write_atomic(&path, bytes, false).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ObjectCollision);
        assert_eq!(fs::read(&path).unwrap(), b"original");
    }
    let absent_parent = temporary.path().join("absent");
    let error = root
        .open_optional_regular(&absent_parent.join("object"))
        .unwrap_err();
    assert!(matches!(error, Error::Filesystem(_)));
    assert!(
        root.open_optional_regular(&temporary.path().join("absent-leaf"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn cold_seal_identity_uses_the_opened_root_after_named_root_rebinding() {
    let temporary = tempfile::tempdir().unwrap();
    let address = temporary.path().join("member");
    private_dir(&address).unwrap();
    fs::write(address.join("object"), b"held bytes").unwrap();
    let mut config = Config::development([address.clone()]);
    config.mode = Mode::Production;
    let config = ValidatedConfig::try_from(config).unwrap();
    let disk = &config.disks[0];
    let root = DiskRoot::open(&address).unwrap();
    let held_directory = root.directory(&address).unwrap();
    let moved = temporary.path().join("held");
    fs::rename(&address, &moved).unwrap();
    private_dir(&address).unwrap();
    fs::write(address.join("object"), b"replacement bytes").unwrap();

    let seal = identity_at(&config, disk, &held_directory).unwrap();
    let observed = identity(&config, disk).unwrap();
    assert_eq!(seal.inode, fs::metadata(&moved).unwrap().ino());
    assert_ne!(seal.inode, observed.inode);
    assert_eq!(seal.device, fs::metadata(&moved).unwrap().dev());
    assert_eq!(
        root.read_bytes(&address.join("object"), 64).unwrap(),
        b"held bytes"
    );
}

fn isolated_mount_namespace(name: &str, exercise: impl FnOnce()) {
    const CHILD: &str = "ZXS_DISK_ADMISSION_MOUNT_NAMESPACE";
    if std::env::var(CHILD).as_deref() == Ok(name) {
        exercise();
        return;
    }
    // Only this exact libtest child is elevated, with no inherited environment.
    // Pass the recursion marker after sudo's environment reset.
    // Keep sudo's user namespace: mapping only root into a new one loses access
    // to private runner-owned build paths. A private mount namespace isolates mounts.
    let status = Command::new("/usr/bin/sudo")
        .args(["-n", "--", "/usr/bin/env", "-i"])
        .arg(format!("{CHILD}={name}"))
        .args(["/usr/bin/unshare", "--mount", "--propagation", "private"])
        .arg(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", name, "--nocapture"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "isolated mount namespace test failed: {status}"
    );
}

fn mount(arguments: &[&str], source: &Path, destination: &Path) {
    let status = Command::new("/usr/bin/mount")
        .args(arguments)
        .arg(source)
        .arg(destination)
        .status()
        .unwrap();
    assert!(status.success(), "mount failed: {status}");
}

#[test]
#[ignore = "requires non-interactive sudo and util-linux for an isolated mount test child"]
fn immutable_existing_bind_leaf_is_rejected_before_read_or_sync() {
    isolated_mount_namespace(
        "disk::admission_tests::immutable_existing_bind_leaf_is_rejected_before_read_or_sync",
        || {
            let temporary = tempfile::tempdir().unwrap();
            let parent = temporary.path().join("member");
            private_dir(&parent).unwrap();
            let source = temporary.path().join("source");
            let destination = parent.join("object");
            fs::write(&source, b"identical").unwrap();
            fs::write(&destination, b"identical").unwrap();
            assert_eq!(fs::metadata(&source).unwrap().nlink(), 1);
            mount(&["--bind"], &source, &destination);
            // The leaf is already mounted when the owner is admitted. A dev/ino
            // comparison or safe-parent-only open cannot replace NO_XDEV here.
            let root = DiskRoot::open(&parent).unwrap();
            for result in [
                root.write_atomic(&destination, b"identical", false),
                write_atomic(&destination, b"identical", false),
            ] {
                let error = result.unwrap_err();
                assert!(matches!(
                    error,
                    Error::Filesystem(platform_io::fs_safety::Error::Io(ref source))
                        if source.raw_os_error() == Some(rustix::io::Errno::XDEV.raw_os_error())
                ));
            }
            assert_eq!(fs::read(&source).unwrap(), b"identical");
        },
    );
}

#[test]
#[ignore = "requires non-interactive sudo and util-linux for an isolated mount test child"]
fn cold_directory_admission_keeps_held_identity_after_mount_replacement() {
    isolated_mount_namespace(
        "disk::admission_tests::cold_directory_admission_keeps_held_identity_after_mount_replacement",
        || {
            let temporary = tempfile::tempdir().unwrap();
            let address = temporary.path().join("member");
            let replacement = temporary.path().join("replacement");
            private_dir(&address).unwrap();
            private_dir(&replacement).unwrap();
            mount(
                &["-t", "tmpfs", "-o", "mode=0700"],
                Path::new("held"),
                &address,
            );
            mount(
                &["-t", "tmpfs", "-o", "mode=0700"],
                Path::new("replacement"),
                &replacement,
            );
            fs::write(address.join("object"), b"held bytes").unwrap();
            let root = DiskRoot::open(&address).unwrap();
            let held_directory = root.directory(&address).unwrap();
            let config = ValidatedConfig::try_from(Config::development([address.clone()])).unwrap();
            let held_seal = identity_at(&config, &config.disks[0], &held_directory).unwrap();
            let status = Command::new("/usr/bin/umount")
                .arg("--lazy")
                .arg(&address)
                .status()
                .unwrap();
            assert!(status.success(), "lazy unmount failed: {status}");
            mount(&["--bind"], &replacement, &address);
            let named = DiskRoot::open(&address).unwrap();
            let named_seal = identity_at(
                &config,
                &config.disks[0],
                &named.directory(&address).unwrap(),
            )
            .unwrap();
            assert_ne!(held_seal.device, named_seal.device);
            let still_held = identity_at(&config, &config.disks[0], &held_directory).unwrap();
            validate_stored_seal(&held_seal, &still_held).unwrap();
            assert_eq!(
                validate_stored_seal(&held_seal, &named_seal)
                    .unwrap_err()
                    .code(),
                ErrorCode::DiskIdentityMismatch
            );
            assert_eq!(
                root.read_bytes(&address.join("object"), 64).unwrap(),
                b"held bytes"
            );
        },
    );
}
