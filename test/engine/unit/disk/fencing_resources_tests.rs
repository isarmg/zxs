use super::*;
use crate::{Config, ValidatedConfig, disk::DiskHandle};
use std::{
    fs,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::Path,
    sync::{Barrier, mpsc},
    time::Duration,
};

fn locked(path: &Path) -> File {
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    file
}

fn excluded(path: &Path, expected: bool) {
    let file = File::options().read(true).write(true).open(path).unwrap();
    let result = rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive);
    if expected {
        assert_eq!(result.unwrap_err(), rustix::io::Errno::WOULDBLOCK);
    } else {
        result.unwrap();
    }
}

fn descriptors(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter(
            |entry| match fs::read_link(entry.as_ref().unwrap().path()) {
                Ok(target) => target == path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => panic!("cannot inspect descriptor: {error}"),
            },
        )
        .count()
}

fn fixture() -> (tempfile::TempDir, Vec<Arc<DiskHandle>>, WriterLocks) {
    let temp = tempfile::tempdir().unwrap();
    let paths = [temp.path().join("primary"), temp.path().join("backup")];
    for path in &paths {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = ValidatedConfig::try_from(Config::development(paths).unwrap()).unwrap();
    let mut locks = WriterLocks::new(locked(&temp.path().join("writer.lock")));
    let mut handles = Vec::new();
    for disk in &config.disks {
        let handle = Arc::new(DiskHandle::open(&config, disk, false).unwrap());
        let writer = FenceOwner::new(locked(&disk.root.join(".writer.lock")), None);
        locks.insert(
            disk.disk_id.clone(),
            DiskWriter::adopt(&handle.root, writer).unwrap(),
        );
        handles.push(handle);
    }
    (temp, handles, locks)
}

#[test]
fn idle_root_and_fence_owners_close_only_target_after_control_tail() {
    let (temp, handles, locks) = fixture();
    let root = &handles[0].root;
    let writer_path = root.path().join(".writer.lock");
    let other_writer = handles[1].root.path().join(".writer.lock");
    let retained = locks.retain().unwrap();
    let disk_id = &handles[0].seal.disk_id;
    let before = locks.disks[disk_id]
        .fence
        .descriptor
        .files
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .writer
        .as_raw_fd();
    let after = retained.disks[disk_id]
        .fence
        .descriptor
        .files
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .writer
        .as_raw_fd();
    assert_eq!(
        before, after,
        "retaining fencing must not dup the OS descriptor"
    );
    assert_eq!(descriptors(&writer_path), 1);
    {
        let files = retained.disks[disk_id]
            .fence
            .descriptor
            .files
            .lock()
            .unwrap();
        assert!(
            rustix::io::fcntl_getfd(&files.as_ref().unwrap().writer)
                .unwrap()
                .contains(rustix::io::FdFlags::CLOEXEC)
        );
    }
    let path = root.path().join("value");
    root.write_atomic(&path, b"data", false).unwrap();
    let idle = root.open_leased_regular(&path).unwrap();
    let child_path = root.path().join("child");
    root.private_dir(&child_path).unwrap();
    let child = root.private_child(&child_path).unwrap();
    let binding = root.io_binding().unwrap();
    let transition = binding.begin_quiesce().unwrap().unwrap();
    assert_eq!(descriptors(&path), 0);
    assert_eq!(descriptors(root.path()), 1);
    assert_eq!(descriptors(&child_path), 1);
    excluded(&writer_path, true);
    assert!(!binding.status().unwrap().drained());
    drop(transition);
    assert!(binding.status().unwrap().drained());
    assert_eq!(descriptors(root.path()), 0);
    assert_eq!(descriptors(&child_path), 0);
    assert_eq!(descriptors(&writer_path), 0);
    excluded(&writer_path, false);
    excluded(&other_writer, true);
    excluded(&temp.path().join("writer.lock"), true);
    assert_eq!(
        idle.check_running().unwrap_err().code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(
        child.entries(&child_path).err().unwrap().code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(
        retained.retain().err().unwrap().code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(fs::read(&path).unwrap(), b"data");
}

#[test]
fn engine_owner_drop_keeps_busy_tail_fenced_until_its_real_descriptor_closes() {
    let (_temp, handles, locks) = fixture();
    let root = &handles[0].root;
    let path = root.path().join("value");
    root.write_atomic(&path, b"data", false).unwrap();
    let writer = root.path().join(".writer.lock");
    let binding = root.io_binding().unwrap();
    let timeout = Duration::from_secs(30);
    std::thread::scope(|threads| {
        let (entered, received) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let path = &path;
        let reader = threads.spawn(move || {
            let mut first = true;
            root.read_bytes_observed(path, 64, &mut |_| {
                if first {
                    first = false;
                    entered.send(()).unwrap();
                    wait.recv_timeout(timeout).unwrap();
                }
                Ok(())
            })
        });
        received.recv_timeout(timeout).unwrap();
        drop(locks);
        excluded(&writer, true);
        let transition = binding.begin_quiesce().unwrap().unwrap();
        drop(transition);
        assert!(!binding.status().unwrap().drained());
        excluded(&writer, true);
        assert_eq!(descriptors(path), 1);
        release.send(()).unwrap();
        let error = reader.join().unwrap().unwrap_err();
        assert_eq!(error.code(), ErrorCode::DiskUnavailable);
        assert!(!error.is_retryable());
        assert!(binding.status().unwrap().drained());
        assert_eq!(descriptors(path), 0);
        assert_eq!(descriptors(root.path()), 0);
        excluded(&writer, false);
        assert_eq!(fs::read(path).unwrap(), b"data");
    });
}

#[test]
fn dropping_last_root_and_final_tail_cannot_publish_closed_before_root_fd_close() {
    for _ in 0..32 {
        let (_temp, mut handles, locks) = fixture();
        let handle = handles.remove(0);
        let root = handle.root.path().to_owned();
        let binding = handle.root.io_binding().unwrap().clone();
        let transition = binding.begin_quiesce().unwrap().unwrap();
        let barrier = Barrier::new(2);
        std::thread::scope(|threads| {
            threads.spawn(|| {
                barrier.wait();
                drop(handle);
            });
            barrier.wait();
            drop(transition);
            assert!(binding.status().unwrap().drained());
            assert_eq!(descriptors(&root), 0);
            excluded(&root.join(".writer.lock"), false);
        });
        drop(locks);
    }
}

#[test]
fn last_fence_owner_in_drop_cannot_publish_closed_before_its_actual_fd_closes() {
    let (_temp, handles, mut locks) = fixture();
    let root = &handles[0].root;
    let writer = root.path().join(".writer.lock");
    let owner = locks.disks.remove(&handles[0].seal.disk_id).unwrap();
    let weak = Arc::downgrade(&owner.fence);
    let descriptor = owner.fence.descriptor();
    let binding = root.io_binding().unwrap();
    let timeout = Duration::from_secs(30);
    std::thread::scope(|threads| {
        // Block the real descriptor close, not the admission gate. The owner's
        // last Arc can disappear while its destructor is waiting on this guard.
        // Keep the guard inside the scope so assertion unwinding releases it
        // before scoped joins, rather than hanging a failing test.
        let guard = descriptor.files.lock().unwrap();
        let (entered, entered_rx) = mpsc::channel();
        let dropping = threads.spawn(move || {
            entered.send(()).unwrap();
            drop(owner);
        });
        entered_rx.recv_timeout(timeout).unwrap();
        let started = std::time::Instant::now();
        // Test choreography only: force the exact destructor-in-flight window.
        // Do not upgrade here: dropping a transient last Arc on this thread
        // would wait on our own guard. Safety assertions below use real FDs.
        while weak.strong_count() != 0 {
            assert!(
                started.elapsed() < timeout,
                "last fence owner did not enter Drop"
            );
            std::thread::yield_now();
        }
        excluded(&writer, true);
        let transition = binding.begin_quiesce().unwrap().unwrap();
        let (closed, closed_rx) = mpsc::channel();
        let closing = threads.spawn(move || {
            drop(transition);
            closed.send(()).unwrap();
        });
        while binding.status().unwrap().control != 0 {
            assert!(
                started.elapsed() < timeout,
                "control tail did not start final closure"
            );
            std::thread::yield_now();
        }
        assert!(!binding.status().unwrap().drained());
        assert_eq!(descriptors(&writer), 1);
        excluded(&writer, true);
        assert_eq!(
            closed_rx
                .recv_timeout(Duration::from_millis(50))
                .unwrap_err(),
            mpsc::RecvTimeoutError::Timeout
        );
        drop(guard);
        closed_rx.recv_timeout(timeout).unwrap();
        dropping.join().unwrap();
        closing.join().unwrap();
        assert!(binding.status().unwrap().drained());
        assert_eq!(descriptors(&writer), 0);
        excluded(&writer, false);
    });
}

#[test]
fn closure_proof_retires_only_its_writer_and_preserves_system_fencing() {
    let (temporary, handles, locks) = fixture();
    let target = &handles[0];
    let binding = target.root.io_binding().unwrap();
    let tail = binding.admit().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.closed_proof().is_err());
    excluded(&target.root.path().join(".writer.lock"), true);
    drop(tail);
    let proof = binding.closed_proof().unwrap();
    let remaining = locks.excluding_closed(&proof).unwrap();
    assert_eq!(remaining.len(), locks.len() - 1);
    assert!(!remaining.disks.contains_key(&target.seal.disk_id));
    assert_eq!(
        remaining.system_identity().unwrap(),
        locks.system_identity().unwrap()
    );
    assert_eq!(remaining.activity().unwrap().len(), 1);
    assert_eq!(
        locks.activity().err().unwrap().code(),
        ErrorCode::DiskUnavailable
    );
    excluded(&target.root.path().join(".writer.lock"), false);
    excluded(&handles[1].root.path().join(".writer.lock"), true);
    excluded(&temporary.path().join("writer.lock"), true);
    assert_eq!(
        remaining.excluding_closed(&proof).err().unwrap().code(),
        ErrorCode::DiskIdentityMismatch
    );
}
