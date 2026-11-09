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
