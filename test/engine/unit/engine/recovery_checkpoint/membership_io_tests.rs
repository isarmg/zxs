use super::*;
use std::{
    fs::{self, File},
    io,
    sync::{Arc, mpsc},
    time::Duration,
};

fn disk() -> (tempfile::TempDir, Arc<DiskHandle>, DiskWriter) {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config =
        ValidatedConfig::try_from(Config::development([temp.path().to_path_buf()]).unwrap())
            .unwrap();
    let handle = Arc::new(DiskHandle::open(&config, &config.disks[0], false).unwrap());
    let lock = handle.root.lock_file().unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    let writer = DiskWriter::adopt(&handle.root, FenceOwner::new(lock, None)).unwrap();
    (temp, handle, writer)
}

fn descriptors(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|entry| match fs::read_link(entry) {
            Ok(target) => target == path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => panic!("cannot inspect checkpoint descriptor: {error}"),
        })
        .count()
}

fn writer_excluded(path: &Path, expected: bool) {
    let file = File::options().read(true).write(true).open(path).unwrap();
    let result = rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive);
    if expected {
        assert_eq!(result.unwrap_err(), rustix::io::Errno::WOULDBLOCK);
    } else {
        result.unwrap();
    }
}

#[test]
fn managed_hash_preserves_empty_partial_and_multiple_chunk_bytes() {
    let (_temp, handle, _writer) = disk();
    let path = handle.root.path().join("evidence");
    for length in [0, 65_535, 65_536, 65_537, 3 * 65_536 + 17] {
        let bytes = (0..length)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        handle.root.write_atomic(&path, &bytes, true).unwrap();
        assert_eq!(
            file_hash(&handle.root, &path).unwrap(),
            blake3::hash(&bytes).to_hex().to_string()
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(descriptors(&path), 0);
        assert_eq!(
            handle.root.io_binding().unwrap().status().unwrap().ordinary,
            0
        );
    }
}

#[test]
fn optional_marker_keeps_leaf_absence_and_unsafe_parent_errors_distinct() {
    let (_temp, handle, _writer) = disk();
    let relative = Path::new("histories/history_test/file-containers/file_test/namespace/name");
    let parent = handle.root.path().join(relative);
    let marker = parent.join(".zxs-container-owner.json");
    assert!(named_container(&handle.root, relative).is_err());
    handle.root.private_dir(&parent).unwrap();
    assert!(!named_container(&handle.root, relative).unwrap());
    assert!(
        handle
            .root
            .open_optional_read_regular(&marker)
            .unwrap()
            .is_none()
    );

    handle
        .root
        .write_atomic(&marker, b"\"file_test\"", false)
        .unwrap();
    assert!(named_container(&handle.root, relative).unwrap());
    handle
        .root
        .write_atomic(&marker, b"\"another_file\"", true)
        .unwrap();
    assert!(!named_container(&handle.root, relative).unwrap());
    handle
        .root
        .write_atomic(&marker, b"not json", true)
        .unwrap();
    assert!(named_container(&handle.root, relative).is_err());
    assert_eq!(fs::read(&marker).unwrap(), b"not json");
    fs::remove_file(&marker).unwrap();

    let target = handle.root.path().join("owner-target");
    handle
        .root
        .write_atomic(&target, b"\"file_test\"", false)
        .unwrap();
    std::os::unix::fs::symlink(&target, &marker).unwrap();
    assert!(named_container(&handle.root, relative).is_err());
    fs::remove_file(&marker).unwrap();
    fs::hard_link(&target, &marker).unwrap();
    assert!(named_container(&handle.root, relative).is_err());
    fs::remove_file(&marker).unwrap();
    fs::create_dir(&marker).unwrap();
    assert!(named_container(&handle.root, relative).is_err());
    fs::remove_dir(&marker).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &marker,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    assert!(named_container(&handle.root, relative).is_err());
    fs::remove_file(&marker).unwrap();

    let moved = parent.with_extension("held");
    fs::rename(&parent, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &parent).unwrap();
    assert!(named_container(&handle.root, relative).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"\"file_test\"");
}

#[test]
fn marker_and_envelope_reads_keep_their_byte_limits() {
    let (_temp, handle, _writer) = disk();
    let relative = Path::new("histories/history_test/file-containers/file_test/namespace/name");
    let parent = handle.root.path().join(relative);
    handle.root.private_dir(&parent).unwrap();
    let marker = parent.join(".zxs-container-owner.json");
    let mut bytes = vec![b' '; 65_536];
    bytes.extend_from_slice(b"\"file_test\"");
    bytes.extend_from_slice(&vec![b' '; 65_537]);
    handle.root.write_atomic(&marker, &bytes, false).unwrap();
    assert!(named_container(&handle.root, relative).unwrap());
    assert_eq!(fs::read(&marker).unwrap(), bytes);
    for maximum in [MAX_CHECKPOINT_BYTES, MAX_METADATA] {
        File::options()
            .write(true)
            .open(&marker)
            .unwrap()
            .set_len(maximum as u64 + 1)
            .unwrap();
        let file = handle
            .root
            .open_optional_read_regular(&marker)
            .unwrap()
            .unwrap();
        assert_eq!(
            checkpoint_bytes(file, maximum).unwrap_err().code(),
            ErrorCode::ResourceLimit
        );
        assert_eq!(fs::metadata(&marker).unwrap().len(), maximum as u64 + 1);
        assert_eq!(descriptors(&marker), 0);
    }
}

#[test]
fn idle_checkpoint_handles_close_without_polling_or_dropping_the_wrappers() {
    let (_temp, handle, _writer) = disk();
    let evidence = handle.root.path().join("evidence");
    let marker = handle.root.path().join("marker");
    handle
        .root
        .write_atomic(&evidence, b"checkpoint evidence", false)
        .unwrap();
    handle
        .root
        .write_atomic(&marker, b"\"file_test\"", false)
        .unwrap();
    let idle_hash = handle.root.open_read_regular(&evidence).unwrap();
    let idle_marker = handle
        .root
        .open_optional_read_regular(&marker)
        .unwrap()
        .unwrap();
    assert_eq!(descriptors(&evidence), 1);
    assert_eq!(descriptors(&marker), 1);
    let binding = handle.root.io_binding().unwrap();
    let transition = binding.begin_quiesce().unwrap().unwrap();
    assert_eq!(descriptors(&evidence), 0);
    assert_eq!(descriptors(&marker), 0);
    drop(transition);
    assert!(binding.status().unwrap().drained());
    writer_excluded(&handle.root.path().join(".writer.lock"), false);
    assert!(hash_reader(idle_hash).is_err());
    assert!(checkpoint_bytes(idle_marker, MAX_METADATA).is_err());
    assert_eq!(
        file_hash(&handle.root, &evidence).unwrap_err().code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(fs::read(&evidence).unwrap(), b"checkpoint evidence");
    assert_eq!(fs::read(&marker).unwrap(), b"\"file_test\"");
}

// This controls the wrapper's return tail, not a blocked syscall or close.
struct PausedFile {
    file: File,
    fail_at_eof: bool,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

impl Read for PausedFile {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        assert_eq!(output.len(), 64 * 1024);
        let result = if self.fail_at_eof {
            self.file.read_exact(output).map(|()| output.len())
        } else {
            self.file.read(output)
        };
        // The real read has completed, but the managed resource's read guard
        // still owns the exact File through this return tail.
        self.entered.send(()).unwrap();
        self.release.recv_timeout(Duration::from_secs(30)).unwrap();
        result
    }
}

fn cancel_hash_tail(fail_at_eof: bool) {
    let (_temp, handle, writer) = disk();
    let root = &handle.root;
    let path = root.path().join("evidence");
    let bytes = b"checkpoint evidence";
    root.write_atomic(&path, bytes, false).unwrap();
    let idle_path = root.path().join("idle-marker");
    root.write_atomic(&idle_path, b"\"file_test\"", false)
        .unwrap();
    let idle = root
        .open_optional_read_regular(&idle_path)
        .unwrap()
        .unwrap();
    let binding = root.io_binding().unwrap();
    let (entered, received) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let reader = binding.admit().unwrap().hold(PausedFile {
        file: root.open_regular(&path).unwrap(),
        fail_at_eof,
        entered,
        release: wait,
    });
    std::thread::scope(|threads| {
        let worker = threads.spawn(move || hash_reader(reader));
        received.recv_timeout(Duration::from_secs(30)).unwrap();
        drop(writer);
        let writer_path = root.path().join(".writer.lock");
        writer_excluded(&writer_path, true);
        let transition = binding.begin_quiesce().unwrap().unwrap();
        drop(transition);
        assert_eq!(binding.status().unwrap().ordinary, 1);
        assert!(!binding.status().unwrap().drained());
        assert_eq!(descriptors(&path), 1);
        assert_eq!(descriptors(&idle_path), 0);
        writer_excluded(&writer_path, true);
        release.send(()).unwrap();
        let error = worker.join().unwrap().unwrap_err();
        let Error::Contract(storage_contract::Error::Io { source, .. }) = error else {
            panic!("hash read lost its I/O source");
        };
        if fail_at_eof {
            assert_eq!(source.kind(), io::ErrorKind::UnexpectedEof);
        } else {
            assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(
                source
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<Error>()
                    .unwrap()
                    .code(),
                ErrorCode::DiskUnavailable
            );
        }
        assert!(binding.status().unwrap().drained());
        assert_eq!(descriptors(&path), 0);
        assert_eq!(descriptors(root.path()), 0);
        writer_excluded(&writer_path, false);
    });
    assert!(checkpoint_bytes(idle, MAX_METADATA).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read(&idle_path).unwrap(), b"\"file_test\"");
}

#[test]
fn cancelled_hash_keeps_real_file_and_writer_until_successful_read_tail_finishes() {
    cancel_hash_tail(false);
}

#[test]
fn cancelled_hash_keeps_real_file_and_writer_until_failed_read_tail_finishes() {
    cancel_hash_tail(true);
}
