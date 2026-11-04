//! Existing published purge cleanup, not disk deregistration or new deletion authority.
use super::*;
use std::{
    fs::File,
    io,
    os::unix::fs::MetadataExt,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

struct PendingCleanup {
    temporary: tempfile::TempDir,
    engine: Engine,
    plan: Plan,
    operation: OperationId,
    first: String,
    last: String,
    payloads: Vec<PathBuf>,
}

impl PendingCleanup {
    fn new() -> Self {
        let (temporary, mut engine, repo, _, first, last) =
            fixture_with_members([true, false, true]);
        let plan = engine
            .create_purge_plan(&repo.history_tree_id, vec![first.clone()], "admin")
            .unwrap();
        let (blocker, payloads) = blocked_cleanup(&engine, &first);
        let operation = OperationId::generate().unwrap();
        assert_eq!(
            engine
                .commit_plan(&plan.plan_id, "admin", operation)
                .unwrap_err()
                .code(),
            ErrorCode::UnsafePath
        );
        assert_eq!(
            engine.operation(operation).unwrap().commit_state,
            storage_contract::CommitState::Published
        );
        fs::remove_dir(blocker).unwrap();
        Self {
            temporary,
            engine,
            plan,
            operation,
            first,
            last,
            payloads,
        }
    }

    fn retry(&mut self) -> Result<OperationResult> {
        self.engine
            .commit_plan(&self.plan.plan_id, "admin", self.operation)
    }

    fn directory(&self, disk: &DiskConfig) -> PathBuf {
        version_dir(
            disk,
            &self.engine.state.versions[&self.first].history_tree_id,
            &self.first,
        )
    }

    fn retained_body(&self, disk: &DiskConfig) -> PathBuf {
        let version = &self.engine.state.versions[&self.last];
        crate::container_storage::content_path(
            &version_dir(disk, &version.history_tree_id, &self.last),
            version,
        )
        .unwrap()
    }
}

// Capture actual bytes and file identity, including shared metadata objects.
fn files_below(directory: &Path) -> BTreeMap<PathBuf, (u64, Vec<u8>)> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        let metadata = fs::symlink_metadata(&path).unwrap();
        if metadata.is_dir() {
            files.extend(files_below(&path));
        } else {
            assert!(metadata.is_file());
            let bytes = fs::read(&path).unwrap();
            files.insert(path, (metadata.ino(), bytes));
        }
    }
    files
}

fn assert_files_unchanged(files: &BTreeMap<PathBuf, (u64, Vec<u8>)>) {
    for (path, (inode, bytes)) in files {
        assert_eq!(fs::metadata(path).unwrap().ino(), *inode, "{path:?}");
        assert_eq!(&fs::read(path).unwrap(), bytes, "{path:?}");
    }
}

#[test]
fn stopped_cleanup_target_retains_original_bytes_and_shared_objects() {
    let mut pending = PendingCleanup::new();
    let disk = pending.engine.config.disks[1].clone();
    assert!(!disk.metadata_member);
    let handle = Arc::clone(pending.engine.disk_resources.get(&disk).unwrap());
    let directory = pending.directory(&disk);
    let original = files_below(&disk.root);
    let state = canonical_json(pending.engine.state()).unwrap();
    let receipt = pending.engine.operation(pending.operation).unwrap();
    let binding = handle.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(
        handle
            .root
            .directory_identity(&directory)
            .unwrap_err()
            .code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(
        handle
            .root
            .remove_file_if_present_synced(&pending.payloads[1])
            .unwrap_err()
            .code(),
        ErrorCode::DiskUnavailable
    );

    // Preserve the established unavailable-target skip. A publication receipt
    // does not claim that this stopped disk's physical cleanup has completed.
    assert_eq!(pending.retry().unwrap().result, receipt.result);
    assert_eq!(canonical_json(pending.engine.state()).unwrap(), state);
    assert_eq!(files_below(&disk.root), original);
    assert!(!directory.join("purged.json").exists());
    assert_eq!(fs::read(&pending.payloads[1]).unwrap(), b"old body");
    assert_eq!(
        fs::read(pending.retained_body(&disk)).unwrap(),
        b"latest body"
    );
    for payload in [&pending.payloads[0], &pending.payloads[2]] {
        assert!(
            !payload.exists(),
            "healthy targets retain their cleanup scope"
        );
    }
}

#[test]
fn cleanup_rejects_shared_payload_identity_without_unlinking_either_name() {
    let mut pending = PendingCleanup::new();
    let disk = pending.engine.config.disks[0].clone();
    let source = pending.payloads[0].clone();
    let shared = pending.temporary.path().join("shared-payload");
    fs::hard_link(&source, &shared).unwrap();
    let inode = fs::metadata(&source).unwrap().ino();
    let immutable = files_below(&disk.root.join("metadata"));
    let state = canonical_json(pending.engine.state()).unwrap();
    assert_eq!(
        pending.retry().unwrap_err().code(),
        ErrorCode::ContainerLayoutInvalid
    );
    for path in [&source, &shared] {
        assert_eq!(fs::read(path).unwrap(), b"old body");
        assert_eq!(fs::metadata(path).unwrap().ino(), inode);
        assert_eq!(fs::metadata(path).unwrap().nlink(), 2);
    }
    assert_files_unchanged(&immutable);
    assert_eq!(canonical_json(pending.engine.state()).unwrap(), state);
    assert_eq!(
        fs::read(pending.retained_body(&disk)).unwrap(),
        b"latest body"
    );
    for payload in &pending.payloads[1..] {
        assert_eq!(fs::read(payload).unwrap(), b"old body");
    }
}

#[test]
fn cleanup_retry_distinguishes_unlinked_and_absent_payload_without_republishing() {
    let mut pending = PendingCleanup::new();
    let disk = pending.engine.config.disks[0].clone();
    let handle = Arc::clone(pending.engine.disk_resources.get(&disk).unwrap());
    let state = canonical_json(pending.engine.state()).unwrap();
    let immutable = files_below(&disk.root.join("metadata"));
    assert!(
        handle
            .root
            .remove_file_if_present_synced(&pending.payloads[0])
            .unwrap()
    );
    assert!(
        !handle
            .root
            .remove_file_if_present_synced(&pending.payloads[0])
            .unwrap()
    );
    pending.retry().unwrap();
    assert!(pending.payloads.iter().all(|payload| !payload.exists()));
    assert_eq!(canonical_json(pending.engine.state()).unwrap(), state);
    assert_files_unchanged(&immutable);
    assert_eq!(
        fs::read(pending.retained_body(&disk)).unwrap(),
        b"latest body"
    );
}

#[test]
fn cleanup_missing_payload_parent_preserves_original_not_found_error() {
    let mut pending = PendingCleanup::new();
    let disk = pending.engine.config.disks[0].clone();
    let payload = pending.payloads[0].clone();
    let parent = payload.parent().unwrap();
    let container = parent.parent().unwrap();
    let retained = pending.temporary.path().join("retained-container");
    // Move the whole owner out of the namespace. Leaving its owner marker
    // without versions is a layout error before the unlink can be attempted.
    fs::rename(container, &retained).unwrap();
    let directory = pending.directory(&disk);
    let content = crate::container_storage::content_path_at(
        &pending.engine.disk_resources.get(&disk).unwrap().root,
        &directory,
        &pending.engine.state.versions[&pending.first],
        &mut |_| Ok(()),
    )
    .unwrap();
    assert!(!content.parent().unwrap().exists());
    let original = files_below(&retained);
    let immutable = files_below(&disk.root.join("metadata"));
    let state = canonical_json(pending.engine.state()).unwrap();
    let error = pending.retry().unwrap_err();
    let Error::Filesystem(platform_io::fs_safety::Error::Io(source)) = error else {
        panic!("missing cleanup parent lost its I/O source: {error}");
    };
    assert_eq!(source.kind(), io::ErrorKind::NotFound);
    assert!(directory.join("purged.json").is_file());
    assert_eq!(files_below(&retained), original);
    assert_files_unchanged(&immutable);
    assert_eq!(canonical_json(pending.engine.state()).unwrap(), state);
    assert!(!container.exists());
    assert!(!parent.exists());
    for payload in &pending.payloads[1..] {
        assert_eq!(fs::read(payload).unwrap(), b"old body");
    }
}

#[test]
fn cleanup_missing_owned_versions_directory_preserves_layout_rejection() {
    let mut pending = PendingCleanup::new();
    let disk = pending.engine.config.disks[0].clone();
    let payload = pending.payloads[0].clone();
    let parent = payload.parent().unwrap();
    let retained = parent.with_file_name("retained-versions");
    fs::rename(parent, &retained).unwrap();
    let original = files_below(&retained);
    let immutable = files_below(&disk.root.join("metadata"));
    let state = canonical_json(pending.engine.state()).unwrap();
    let error = pending.retry().unwrap_err();
    assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid);
    assert_eq!(error.message(), "owned container has no version directory");
    assert_eq!(files_below(&retained), original);
    assert_files_unchanged(&immutable);
    assert_eq!(canonical_json(pending.engine.state()).unwrap(), state);
    assert!(!parent.exists());
    for payload in &pending.payloads[1..] {
        assert_eq!(fs::read(payload).unwrap(), b"old body");
    }
}

fn descriptors(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter(
            |entry| match fs::read_link(entry.as_ref().unwrap().path()) {
                Ok(target) => target == path,
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => panic!("cannot inspect descriptor: {error}"),
            },
        )
        .count()
}

// Its marker is consumed only after the selected payload name is absent.
struct CleanupFsyncGate(PathBuf);

impl CleanupFsyncGate {
    fn arm(payload: &Path, errno: Option<i32>) -> Self {
        assert!(
            std::env::var_os("LD_PRELOAD")
                .is_some_and(|value| value.to_string_lossy().contains("maintenance_io_tails")),
            "requires maintenance/io_tails.c compiled as maintenance_io_tails.so"
        );
        let directory = payload.parent().unwrap();
        fs::write(
            directory.join(".cleanup-watch"),
            payload.as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        match errno {
            None => {}
            Some(2) => fs::write(directory.join(".cleanup-enoent"), []).unwrap(),
            Some(5) => fs::write(directory.join(".cleanup-eio"), []).unwrap(),
            Some(errno) => panic!("unsupported injected errno: {errno}"),
        }
        fs::write(directory.join(".cleanup-arm"), []).unwrap();
        Self(directory.to_path_buf())
    }

    fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.0.join(".cleanup-entered").exists() {
            assert!(
                Instant::now() < deadline,
                "cleanup did not reach real fsync"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn release(&self) {
        fs::write(self.0.join(".cleanup-release"), []).unwrap();
    }
}

impl Drop for CleanupFsyncGate {
    fn drop(&mut self) {
        // Release the syscall even if a test assertion fails before join.
        let _ = fs::write(self.0.join(".cleanup-release"), []);
    }
}

#[test]
#[ignore = "requires maintenance/io_tails.c as maintenance_io_tails.so in LD_PRELOAD"]
fn cleanup_unlink_fsync_cancellation_and_io_errors_keep_count_fence_and_immutable_bytes() {
    for (already_absent, errno) in [
        (false, None),
        (false, Some(5)),
        (false, Some(2)),
        (true, None),
        (true, Some(5)),
        (true, Some(2)),
    ] {
        let mut pending = PendingCleanup::new();
        let disk = pending.engine.config.disks[0].clone();
        let handle = Arc::clone(pending.engine.disk_resources.get(&disk).unwrap());
        let binding = handle.root.io_binding().unwrap();
        let payload = pending.payloads[0].clone();
        let directory = pending.directory(&disk);
        let retained = pending.retained_body(&disk);
        let retained_inode = fs::metadata(&retained).unwrap().ino();
        let immutable = files_below(&disk.root.join("metadata"));
        let version_files = files_below(&directory);
        assert!(!immutable.is_empty());
        assert!(!version_files.is_empty());
        let state = canonical_json(pending.engine.state()).unwrap();
        let competitor = File::options()
            .read(true)
            .write(true)
            .open(disk.root.join(".writer.lock"))
            .unwrap();
        if already_absent {
            // Model a preceding attempt that unlinked but did not confirm sync.
            fs::remove_file(&payload).unwrap();
        }
        std::thread::scope(|threads| {
            let gate = CleanupFsyncGate::arm(&payload, errno);
            let work = threads.spawn(|| pending.retry());
            gate.wait();
            assert!(
                !payload.exists(),
                "the actual unlink precedes its fsync tail"
            );
            assert!(directory.join("purged.json").is_file());
            let mut transition = binding.begin_quiesce().unwrap().unwrap();
            assert_eq!(binding.status().unwrap().ordinary, 1);
            assert_eq!(
                transition.take_drained_control(binding).unwrap_err().code(),
                ErrorCode::DiskUnavailable
            );
            drop(transition);
            assert!(!binding.status().unwrap().drained());
            assert!(descriptors(payload.parent().unwrap()) > 0);
            assert_eq!(descriptors(&directory), 0);
            assert_eq!(
                rustix::fs::flock(
                    &competitor,
                    rustix::fs::FlockOperation::NonBlockingLockExclusive,
                )
                .unwrap_err(),
                rustix::io::Errno::WOULDBLOCK
            );
            gate.release();
            let error = work.join().unwrap().unwrap_err();
            if let Some(errno) = errno {
                let Error::Contract(storage_contract::Error::Io { source, .. }) = error else {
                    panic!("cleanup fsync lost its original I/O error: {error}");
                };
                assert_eq!(source.raw_os_error(), Some(errno));
            } else {
                assert_eq!(error.code(), ErrorCode::DiskUnavailable);
            }
            assert!(binding.status().unwrap().drained());
            assert_eq!(descriptors(payload.parent().unwrap()), 0);
            assert_eq!(descriptors(&disk.root), 0);
            rustix::fs::flock(
                &competitor,
                rustix::fs::FlockOperation::NonBlockingLockExclusive,
            )
            .unwrap();
        });
        assert!(!payload.exists());
        assert_eq!(fs::metadata(&retained).unwrap().ino(), retained_inode);
        assert_eq!(fs::read(&retained).unwrap(), b"latest body");
        assert_files_unchanged(&immutable);
        assert_files_unchanged(&version_files);
        assert_eq!(canonical_json(pending.engine.state()).unwrap(), state);
        for payload in &pending.payloads[1..] {
            assert_eq!(fs::read(payload).unwrap(), b"old body");
        }
    }
}
