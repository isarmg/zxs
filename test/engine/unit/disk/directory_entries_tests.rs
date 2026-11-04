use super::*;
use std::sync::Barrier;

fn disk() -> (tempfile::TempDir, Arc<DiskHandle>, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config =
        ValidatedConfig::try_from(Config::development([directory.path().to_path_buf()]).unwrap())
            .unwrap();
    let handle = Arc::new(DiskHandle::open(&config, &config.disks[0], false).unwrap());
    let child = directory.path().join("entries");
    handle.root.private_dir(&child).unwrap();
    handle
        .root
        .write_atomic(&child.join("payload"), b"value", false)
        .unwrap();
    (directory, handle, child)
}

fn open_descriptors_for(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|entry| match fs::read_link(entry) {
            Ok(target) => target == path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => panic!("cannot inspect test descriptor: {error}"),
        })
        .count()
}

fn assert_cancelled(error: platform_io::fs_safety::Error) {
    let platform_io::fs_safety::Error::Io(error) = error else {
        panic!("revoked cursor did not preserve its I/O source");
    };
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    let source = error
        .get_ref()
        .unwrap()
        .downcast_ref::<crate::Error>()
        .unwrap();
    assert_eq!(source.code(), ErrorCode::DiskUnavailable);
}

#[test]
fn idle_directory_cursor_closes_before_drain_without_another_poll() {
    let (_directory, handle, path) = disk();
    let binding = handle.root.io_binding().unwrap();
    let mut entries = handle.root.entries(&path).unwrap();
    assert_eq!(open_descriptors_for(&path), 1);
    assert_eq!(binding.status().unwrap().ordinary, 1);

    drop(binding.begin_quiesce().unwrap().unwrap());
    assert_eq!(open_descriptors_for(&path), 0);
    assert!(binding.status().unwrap().drained());
    assert_cancelled(entries.next().unwrap().unwrap_err());
    assert!(entries.next().is_none());
    assert!(entries.next().is_none());
    assert_eq!(fs::read(path.join("payload")).unwrap(), b"value");
}

#[test]
fn buffered_names_are_not_returned_after_revocation() {
    let (_directory, handle, path) = disk();
    handle
        .root
        .write_atomic(&path.join("second"), b"other", false)
        .unwrap();
    let mut entries = handle.root.entries(&path).unwrap();
    entries.next().unwrap().unwrap();
    let binding = handle.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert_cancelled(entries.next().unwrap().unwrap_err());
    assert!(entries.next().is_none());
    assert_eq!(open_descriptors_for(&path), 0);
    assert!(binding.status().unwrap().drained());
}

#[test]
fn eof_retires_the_stream_even_while_its_wrapper_remains_alive() {
    let (_directory, handle, path) = disk();
    let binding = handle.root.io_binding().unwrap();
    let mut entries = handle.root.entries(&path).unwrap();
    assert_eq!(entries.next().unwrap().unwrap().as_os_str(), "payload");
    assert_eq!(binding.status().unwrap().ordinary, 1);
    assert!(entries.next().is_none());
    assert_eq!(open_descriptors_for(&path), 0);
    assert_eq!(binding.status().unwrap().ordinary, 0);
    assert!(entries.next().is_none());

    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert!(entries.next().is_none());
}

#[test]
fn dropping_an_unfinished_cursor_releases_its_descriptor_and_operation() {
    let (_directory, handle, path) = disk();
    let binding = handle.root.io_binding().unwrap();
    let entries = handle.root.entries(&path).unwrap();
    assert_eq!(binding.status().unwrap().ordinary, 1);
    assert_eq!(open_descriptors_for(&path), 1);
    drop(entries);
    assert_eq!(open_descriptors_for(&path), 0);
    assert_eq!(binding.status().unwrap().ordinary, 0);
    assert!(!binding.status().unwrap().quiescing);
}

#[test]
fn a_stream_opened_before_quiesce_cannot_escape_late_registration() {
    let (_directory, handle, path) = disk();
    let binding = handle.root.io_binding().unwrap();
    let operation = handle.root.operation().unwrap();
    let entries = operation.entries(&path).unwrap();
    assert_eq!(open_descriptors_for(&path), 1);
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert_eq!(binding.status().unwrap().ordinary, 1);

    assert!(matches!(
        DiskDirectoryEntries::new(entries, operation.lease),
        Err(error) if error.code() == ErrorCode::DiskUnavailable
    ));
    assert_eq!(open_descriptors_for(&path), 0);
    assert!(binding.status().unwrap().drained());
}

#[test]
fn failed_stream_open_releases_the_opening_operation() {
    let (_directory, handle, path) = disk();
    let binding = handle.root.io_binding().unwrap();
    assert!(handle.root.entries(&path.join("missing")).is_err());
    assert!(handle.root.entries(&path.join("payload")).is_err());
    assert_eq!(binding.status().unwrap().ordinary, 0);
    assert_eq!(open_descriptors_for(&path), 0);
    assert_eq!(fs::read(path.join("payload")).unwrap(), b"value");
}

#[test]
fn derived_root_and_parent_cursors_keep_the_same_revocation_domain() {
    let (_directory, handle, path) = disk();
    let binding = handle.root.io_binding().unwrap();
    let child = handle.root.private_child(&path).unwrap();
    // The retained root FD is explicitly outside this iterator-only slice.
    let retained_root_descriptors = open_descriptors_for(&path);
    assert_eq!(retained_root_descriptors, 1);
    let mut parent_entries = handle.root.entries(&path).unwrap();
    let mut child_entries = child.entries(&path).unwrap();
    assert_eq!(open_descriptors_for(&path), retained_root_descriptors + 2);
    assert_eq!(binding.status().unwrap().ordinary, 2);

    drop(binding.begin_quiesce().unwrap().unwrap());
    assert_eq!(open_descriptors_for(&path), retained_root_descriptors);
    assert!(binding.status().unwrap().drained());
    assert_cancelled(parent_entries.next().unwrap().unwrap_err());
    assert_cancelled(child_entries.next().unwrap().unwrap_err());
    assert!(parent_entries.next().is_none());
    assert!(child_entries.next().is_none());
}

#[test]
fn unbound_bootstrap_cursor_stays_distinct_and_closes_at_eof() {
    let (_directory, handle, _path) = disk();
    let bootstrap = tempfile::tempdir().unwrap();
    fs::set_permissions(bootstrap.path(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(bootstrap.path().join("bootstrap"), b"value").unwrap();
    let root = DiskRoot::open(bootstrap.path()).unwrap();
    let retained_root_descriptors = open_descriptors_for(bootstrap.path());
    let mut entries = root.entries(bootstrap.path()).unwrap();
    let binding = handle.root.io_binding().unwrap();
    assert_eq!(binding.status().unwrap().ordinary, 0);
    drop(binding.begin_quiesce().unwrap().unwrap());

    assert_eq!(entries.next().unwrap().unwrap().as_os_str(), "bootstrap");
    assert!(entries.next().is_none());
    assert_eq!(
        open_descriptors_for(bootstrap.path()),
        retained_root_descriptors
    );
    assert!(entries.next().is_none());
}

#[test]
fn concurrent_next_and_quiesce_cannot_leave_a_live_directory_stream() {
    for _ in 0..32 {
        let (_directory, handle, path) = disk();
        let binding = handle.root.io_binding().unwrap();
        let mut entries = handle.root.entries(&path).unwrap();
        let barrier = Barrier::new(2);
        let (mut entries, next) = std::thread::scope(|threads| {
            let reader = threads.spawn(|| {
                barrier.wait();
                let next = entries.next();
                (entries, next)
            });
            barrier.wait();
            drop(binding.begin_quiesce().unwrap().unwrap());
            reader.join().unwrap()
        });
        assert_eq!(open_descriptors_for(&path), 0);
        assert!(binding.status().unwrap().drained());
        match next.unwrap() {
            Ok(name) => {
                assert_eq!(name.as_os_str(), "payload");
                assert_cancelled(entries.next().unwrap().unwrap_err());
            }
            Err(error) => assert_cancelled(error),
        }
        assert!(entries.next().is_none());
    }
}
