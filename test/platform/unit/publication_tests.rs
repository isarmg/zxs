use super::*;
use crate::fs_safety::linux::MountPolicy;
use std::fs;

#[test]
fn published_staging_cleanup_preserves_a_reused_temporary_name() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = OpenAt2Root::open(temporary.path(), MountPolicy::AllowNestedMounts).unwrap();
    let name = "temporary";
    fs::write(temporary.path().join(name), b"published").unwrap();
    let mut cleanup = StagingCleanup {
        directory: directory.directory(),
        name: name.to_owned(),
        armed: true,
    };
    renameat(directory.directory(), name, directory.directory(), "final").unwrap();
    cleanup.armed = false;
    fs::write(temporary.path().join(name), b"another creator").unwrap();
    drop(cleanup);
    assert_eq!(
        fs::read(temporary.path().join(name)).unwrap(),
        b"another creator"
    );
    assert_eq!(
        fs::read(temporary.path().join("final")).unwrap(),
        b"published"
    );
}

#[test]
fn failed_explicit_staging_cleanup_preserves_error_and_never_retries_on_drop() {
    let root = tempfile::tempdir().unwrap();
    let directory = File::open(root.path()).unwrap();
    fs::create_dir(root.path().join("staged")).unwrap();
    let mut cleanup = StagingCleanup {
        directory: &directory,
        name: "staged".into(),
        armed: true,
    };
    assert_eq!(
        cleanup.cleanup().unwrap_err().kind(),
        std::io::ErrorKind::IsADirectory
    );
    assert!(!cleanup.armed);
    drop(cleanup);
    assert!(root.path().join("staged").is_dir());
}

#[test]
fn failed_drop_cleanup_does_not_panic_when_the_diagnostic_sink_is_full() {
    let root = tempfile::tempdir().unwrap();
    let directory = File::open(root.path()).unwrap();
    fs::create_dir(root.path().join("staged")).unwrap();
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let subscriber = tracing::collector::fmt()
        .with_writer(full)
        .log_internal_errors(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        drop(StagingCleanup {
            directory: &directory,
            name: "staged".into(),
            armed: true,
        });
    });
    assert!(root.path().join("staged").is_dir());
}

#[test]
fn cleanup_failure_keeps_both_typed_causes() {
    use std::error::Error;
    let failure = CleanupFailure {
        operation: Box::new(std::io::Error::from_raw_os_error(5)),
        cleanup: std::io::Error::from_raw_os_error(21),
    };
    assert_eq!(
        failure
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(5)
    );
    assert_eq!(failure.cleanup.raw_os_error(), Some(21));
    assert!(failure.source().unwrap().to_string().contains("os error 5"));
    assert!(failure.to_string().contains("os error 21"));
}

#[test]
fn bundle_inspection_is_read_only_and_final_missing_files_refuse() {
    let temp = tempfile::tempdir().unwrap();
    let host = BundleDestination::open(temp.path()).unwrap();
    let files: &[(&str, &[u8])] = &[("plan.json", b"plan"), ("zxs.service", b"unit")];
    assert!(matches!(
        host.inspect("intent", files).unwrap(),
        BundleState::Absent
    ));
    let pending = temp.path().join("intent.pending");
    private_dir(&pending);
    private_file(&pending.join("plan.json"), b"plan");
    assert!(matches!(
        host.inspect("intent", files).unwrap(),
        BundleState::Pending
    ));
    assert!(!pending.join("zxs.service").exists());
    fs::rename(&pending, temp.path().join("intent")).unwrap();
    assert!(host.inspect("intent", files).is_err());
    assert!(!temp.path().join("intent/zxs.service").exists());
}

#[test]
fn conflicting_or_different_bundle_evidence_is_retained() {
    let temp = tempfile::tempdir().unwrap();
    let host = BundleDestination::open(temp.path()).unwrap();
    let files: &[(&str, &[u8])] = &[("plan.json", b"plan")];
    host.publish("intent", files).unwrap();
    assert!(matches!(
        host.inspect("intent", files).unwrap(),
        BundleState::Published
    ));
    assert!(
        host.inspect("intent", &[("plan.json", b"changed")])
            .is_err()
    );
    private_dir(&temp.path().join("intent.pending"));
    assert!(host.inspect("intent", files).is_err());
    assert_eq!(
        fs::read(temp.path().join("intent/plan.json")).unwrap(),
        b"plan"
    );
    let other = temp.path().join("other.pending");
    private_dir(&other);
    private_file(&other.join("unknown"), b"retain");
    assert!(host.inspect("other", files).is_err());
    assert_eq!(fs::read(other.join("unknown")).unwrap(), b"retain");
}

fn private_dir(path: &std::path::Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn private_file(path: &std::path::Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn bundle_inspection_rejects_unsafe_or_linked_files_without_mutation() {
    use std::os::unix::fs::symlink;
    for kind in ["mode", "hardlink", "symlink"] {
        let temporary = tempfile::tempdir().unwrap();
        let folder = temporary.path().join("operation.pending");
        private_dir(&folder);
        let entry = folder.join("record");
        let outside = temporary.path().join("outside");
        private_file(&outside, b"original");
        match kind {
            "mode" => {
                private_file(&entry, b"original");
                fs::set_permissions(&entry, fs::Permissions::from_mode(0o644)).unwrap();
            }
            "hardlink" => fs::hard_link(&outside, &entry).unwrap(),
            "symlink" => symlink(&outside, &entry).unwrap(),
            _ => unreachable!(),
        }
        let host = BundleDestination::open(temporary.path()).unwrap();
        assert!(
            host.inspect("operation", &[("record", b"original")])
                .is_err(),
            "{kind}"
        );
        assert_eq!(fs::read(&outside).unwrap(), b"original");
        assert_eq!(fs::read(&entry).unwrap(), b"original");
        assert!(!temporary.path().join("operation").exists());
        assert!(folder.is_dir());
    }
}

#[test]
fn publication_receipt_keeps_original_inode_alive_during_caller_readback() {
    let temporary = tempfile::tempdir().unwrap();
    let root = OpenAt2Root::open(temporary.path(), MountPolicy::AllowNestedMounts).unwrap();
    let path = temporary.path().join("rescue.sqlite");
    let receipt = write_atomic_at_mode_receipt(
        &root,
        &EntryName::new("rescue.sqlite").unwrap(),
        b"original",
        false,
        0o600,
    )
    .unwrap();
    let original = fs::metadata(&path).unwrap();
    assert_eq!(receipt.identity().inode, original.ino());
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"replacement").unwrap();
    assert_eq!(receipt._file.metadata().unwrap().nlink(), 0);
    assert_eq!(receipt._file.metadata().unwrap().ino(), original.ino());
    assert_ne!(fs::metadata(&path).unwrap().ino(), original.ino());
}
