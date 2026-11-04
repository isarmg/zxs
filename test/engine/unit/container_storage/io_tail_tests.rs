use super::*;
use std::{
    fs::File,
    io,
    os::unix::fs::{FileExt, PermissionsExt},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

fn fixture(bytes: &[u8]) -> (tempfile::TempDir, Arc<DiskHandle>, FileVersion, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("disk");
    private_dir(&path).unwrap();
    let config = ValidatedConfig::try_from(Config::development([path]).unwrap()).unwrap();
    let handle = Arc::new(DiskHandle::open(&config, &config.disks[0], false).unwrap());
    let item = version(
        &State::default(),
        &new_id("history").unwrap(),
        &new_id("file").unwrap(),
        "body",
        bytes,
    );
    let directory = handle
        .root
        .path()
        .join("histories")
        .join(&item.history_tree_id)
        .join("versions")
        .join(&item.version_id);
    handle.root.private_dir(&directory.join("content")).unwrap();
    (temp, handle, item, directory)
}

fn descriptors(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter(
            |entry| match fs::read_link(entry.as_ref().unwrap().path()) {
                Ok(target) => {
                    target == path
                        || target.as_os_str() == format!("{} (deleted)", path.display()).as_str()
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => panic!("cannot inspect descriptor: {error}"),
            },
        )
        .count()
}

fn fence(handle: &DiskHandle) -> (DiskWriter, File) {
    let writer = handle.root.lock_file().unwrap();
    rustix::fs::flock(
        &writer,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    )
    .unwrap();
    let owner = DiskWriter::adopt(&handle.root, FenceOwner::new(writer, None)).unwrap();
    let competitor = File::options()
        .read(true)
        .write(true)
        .open(handle.root.path().join(".writer.lock"))
        .unwrap();
    (owner, competitor)
}

fn excluded(competitor: &File, expected: bool) {
    let result = rustix::fs::flock(
        competitor,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    );
    if expected {
        assert_eq!(result.unwrap_err(), rustix::io::Errno::WOULDBLOCK);
    } else {
        result.unwrap();
    }
}

#[test]
fn idle_verified_copy_closes_fd_without_polling_and_preserves_published_bytes() {
    let bytes = vec![73u8; 131_089];
    let (temp, handle, item, directory) = fixture(&bytes);
    let external = temp.path().join("external");
    fs::write(&external, &bytes).unwrap();
    let destination = directory.join("content/body");
    let copied = verified_full_copy_at(
        &handle.root,
        File::open(&external).unwrap(),
        &destination,
        &item,
        item.logical_size,
    )
    .unwrap();
    assert_eq!(descriptors(&destination), 1);
    assert_eq!(
        handle.root.io_binding().unwrap().status().unwrap().ordinary,
        1
    );
    let binding = handle.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(descriptors(&destination), 0);
    assert_eq!(fs::read(&destination).unwrap(), bytes);
    let (_, file) = copied.into_parts();
    assert_eq!(
        file.check_running().unwrap_err().code(),
        ErrorCode::DiskUnavailable
    );
    drop(file);
    assert_eq!(fs::read(&destination).unwrap(), bytes);
    assert_eq!(fs::read(&external).unwrap(), bytes);
}

struct PausedSource {
    file: File,
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    once: AtomicBool,
}
impl FileExt for PausedSource {
    fn read_at(&self, bytes: &mut [u8], offset: u64) -> io::Result<usize> {
        assert!(
            bytes.len() <= 64 * 1024,
            "copy reads are bounded cancellation intervals"
        );
        if !self.once.swap(true, Ordering::AcqRel) {
            self.entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(30))
                .unwrap();
        }
        self.file.read_at(bytes, offset)
    }
    fn write_at(&self, _: &[u8], _: u64) -> io::Result<usize> {
        unreachable!()
    }
}

#[test]
fn cancelled_copy_keeps_scratch_cleanup_and_real_writer_fenced_until_tail() {
    let bytes = vec![37u8; 2 * 262_144 + 17];
    let (temp, handle, item, directory) = fixture(&bytes);
    let external = temp.path().join("external");
    fs::write(&external, &bytes).unwrap();
    let destination = directory.join("content/body");
    let (owner, competitor) = fence(&handle);
    let binding = handle.root.io_binding().unwrap();
    std::thread::scope(|threads| {
        let (entered, receive) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let source = PausedSource {
            file: File::open(&external).unwrap(),
            entered,
            release: Mutex::new(wait),
            once: AtomicBool::new(false),
        };
        let copy = threads.spawn(|| {
            verified_full_copy_at(&handle.root, source, &destination, &item, item.logical_size)
        });
        receive.recv_timeout(Duration::from_secs(30)).unwrap();
        let scratch = fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .as_encoded_bytes()
                    .starts_with(b".zxs-full-")
            })
            .unwrap();
        assert_eq!(descriptors(&scratch), 1);
        drop(owner);
        drop(binding.begin_quiesce().unwrap().unwrap());
        assert_eq!(binding.status().unwrap().ordinary, 1);
        excluded(&competitor, true);
        release.send(()).unwrap();
        let error = copy.join().unwrap().unwrap_err();
        assert!(
            matches!(&error, CopyFailure::Destination(error) if error.code() == ErrorCode::DiskUnavailable)
        );
        assert!(!destination.exists());
        assert!(!scratch.exists());
        assert_eq!(descriptors(&scratch), 0);
        assert!(binding.status().unwrap().drained());
        excluded(&competitor, false);
    });
    assert_eq!(fs::read(&external).unwrap(), bytes);
}

struct FailedSource;
impl FileExt for FailedSource {
    fn read_at(&self, _: &mut [u8], _: u64) -> io::Result<usize> {
        Err(io::Error::from_raw_os_error(5))
    }
    fn write_at(&self, _: &[u8], _: u64) -> io::Result<usize> {
        unreachable!()
    }
}

#[test]
fn copy_source_errors_and_destination_collisions_keep_their_origin_and_bytes() {
    let bytes = b"immutable payload";
    let (temp, handle, item, directory) = fixture(bytes);
    let destination = directory.join("content/body");
    let error = verified_full_copy_at(
        &handle.root,
        FailedSource,
        &destination,
        &item,
        item.logical_size,
    )
    .unwrap_err();
    assert!(matches!(error, CopyFailure::Source(_)));
    assert!(
        fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .next()
            .is_none()
    );
    let external = temp.path().join("external");
    fs::write(&external, bytes).unwrap();
    let copied = verified_full_copy_at(
        &handle.root,
        File::open(&external).unwrap(),
        &destination,
        &item,
        item.logical_size,
    )
    .unwrap();
    let before = fs::metadata(&destination).unwrap();
    let error = verified_full_copy_at(
        &handle.root,
        File::open(&external).unwrap(),
        &destination,
        &item,
        item.logical_size,
    )
    .unwrap_err();
    assert!(matches!(error, CopyFailure::Destination(_)));
    assert_eq!(fs::metadata(&destination).unwrap().ino(), before.ino());
    assert_eq!(fs::read(&destination).unwrap(), bytes);
    assert_eq!(
        fs::read_dir(destination.parent().unwrap()).unwrap().count(),
        1
    );
    drop(copied);
}

#[test]
fn idle_verified_body_closes_fd_and_stale_copy_cannot_install_replacement() {
    let bytes = b"unchanged body";
    let (temp, handle, item, directory) = fixture(bytes);
    let path = directory.join("content/body");
    let external = temp.path().join("external");
    fs::write(&external, bytes).unwrap();
    let copied = verified_full_copy_at(
        &handle.root,
        File::open(&external).unwrap(),
        &path,
        &item,
        item.logical_size,
    )
    .unwrap();
    let old = directory.join("content/original");
    fs::rename(&path, &old).unwrap();
    fs::write(&path, bytes).unwrap();
    let error =
        install_version_content_at(&handle.root, &directory, PreparedContent::Copied(copied))
            .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PreparationStale);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read(&old).unwrap(), bytes);
    let body = verify_body(&handle.root, &path, &item).unwrap();
    assert_eq!(descriptors(&path), 1);
    let binding = handle.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert_eq!(descriptors(&path), 0);
    assert!(binding.status().unwrap().drained());
    assert_eq!(
        body.check_running().unwrap_err().code(),
        ErrorCode::DiskUnavailable
    );
    drop(body);
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn bound_install_retry_preserves_valid_target_and_removes_only_verified_duplicate() {
    let bytes = b"already published bytes";
    let (temp, handle, item, directory) = fixture(bytes);
    let external = temp.path().join("external");
    fs::write(&external, bytes).unwrap();
    let source = directory.join("content/body");
    let first = verified_full_copy_at(
        &handle.root,
        File::open(&external).unwrap(),
        &source,
        &item,
        item.logical_size,
    )
    .unwrap();
    let target =
        install_version_content_at(&handle.root, &directory, PreparedContent::Copied(first))
            .unwrap();
    let inode = fs::metadata(&target).unwrap().ino();
    let second = verified_full_copy_at(
        &handle.root,
        File::open(&external).unwrap(),
        &source,
        &item,
        item.logical_size,
    )
    .unwrap();
    let shared = source.parent().unwrap().join("shared");
    fs::write(&shared, b"unrelated shared bytes").unwrap();
    assert_eq!(
        install_version_content_at(&handle.root, &directory, PreparedContent::Copied(second))
            .unwrap(),
        target
    );
    assert_eq!(fs::metadata(&target).unwrap().ino(), inode);
    assert_eq!(fs::read(&target).unwrap(), bytes);
    assert!(!source.exists());
    assert_eq!(fs::read(shared).unwrap(), b"unrelated shared bytes");
}

#[test]
fn stopped_directory_operations_do_not_rename_remove_or_open_optional_files() {
    let bytes = b"retained";
    let (_temp, handle, _item, directory) = fixture(bytes);
    let source = directory.join("content/source");
    let target = directory.join("content/target");
    let empty = directory.join("content/empty");
    fs::write(&source, bytes).unwrap();
    handle.root.private_dir(&empty).unwrap();
    let binding = handle.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    for result in [
        handle.root.rename_noreplace_synced(&source, &target),
        handle.root.remove_file_synced(&source),
        handle.root.remove_empty_directory_synced(&empty),
        handle.root.open_optional_read_regular(&source).map(|_| ()),
    ] {
        assert_eq!(result.unwrap_err().code(), ErrorCode::DiskUnavailable);
    }
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert!(!target.exists());
    assert!(empty.is_dir());
}

#[test]
fn synced_mutations_keep_private_parent_and_noreplace_contracts() {
    let (_temp, handle, _, directory) = fixture(b"body");
    let source = directory.join("content/source");
    let target = directory.join("content/target");
    fs::write(&source, b"source").unwrap();
    fs::write(&target, b"existing").unwrap();
    assert!(
        handle
            .root
            .rename_noreplace_synced(&source, &target)
            .is_err()
    );
    assert_eq!(fs::read(&source).unwrap(), b"source");
    assert_eq!(fs::read(&target).unwrap(), b"existing");
    let public_parent = directory.join("public");
    fs::create_dir(&public_parent).unwrap();
    fs::set_permissions(&public_parent, fs::Permissions::from_mode(0o755)).unwrap();
    let public_file = public_parent.join("shared");
    fs::write(&public_file, b"shared").unwrap();
    assert!(handle.root.remove_file_synced(&public_file).is_err());
    assert!(
        handle
            .root
            .rename_noreplace_synced(&source, &public_parent.join("target"))
            .is_err()
    );
    assert_eq!(fs::read(public_file).unwrap(), b"shared");
    assert_eq!(fs::read(source).unwrap(), b"source");
}

struct SyscallGate(PathBuf);
impl SyscallGate {
    fn arm(path: &Path, fail: bool) -> Self {
        assert!(
            std::env::var_os("LD_PRELOAD")
                .is_some_and(|value| value.to_string_lossy().contains("io_tails")),
            "requires the io_tails.c interposer"
        );
        if fail {
            fs::write(path.join(".io-eio"), []).unwrap();
        }
        fs::write(path.join(".io-arm"), []).unwrap();
        Self(path.to_path_buf())
    }
    fn wait(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !self.0.join(".io-entered").exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "real syscall did not reach its gate"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn release(&self) {
        fs::write(self.0.join(".io-release"), []).unwrap();
    }
}
impl Drop for SyscallGate {
    fn drop(&mut self) {
        // Also release a blocked syscall if an inspection assertion panics.
        let _ = fs::write(self.0.join(".io-release"), []);
    }
}

#[test]
#[ignore = "requires test/engine/unit/container_storage/io_tails.c as LD_PRELOAD"]
fn hash_read_cancellation_keeps_actual_fd_and_writer_until_syscall_returns() {
    let bytes = vec![29u8; 3 * 65536 + 17];
    let (_temp, handle, item, _directory) = fixture(&bytes);
    let directory = handle.root.path().join("zxs-tail-hash");
    handle.root.private_dir(&directory).unwrap();
    let path = directory.join("body");
    fs::write(&path, &bytes).unwrap();
    let (owner, competitor) = fence(&handle);
    let binding = handle.root.io_binding().unwrap();
    std::thread::scope(|threads| {
        let gate = SyscallGate::arm(&directory, false);
        let work = threads.spawn(|| verify_body(&handle.root, &path, &item));
        gate.wait();
        assert_eq!(descriptors(&path), 1);
        drop(owner);
        drop(binding.begin_quiesce().unwrap().unwrap());
        assert_eq!(binding.status().unwrap().ordinary, 1);
        assert_eq!(descriptors(&path), 1);
        excluded(&competitor, true);
        gate.release();
        let Err(error) = work.join().unwrap() else {
            panic!("cancelled hash returned a proof")
        };
        assert_ne!(error.code(), ErrorCode::ContentCorrupt);
        assert!(binding.status().unwrap().drained());
        assert_eq!(descriptors(&path), 0);
        excluded(&competitor, false);
    });
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
#[ignore = "requires test/engine/unit/container_storage/io_tails.c as LD_PRELOAD"]
fn directory_fsync_cancellation_and_eio_keep_mutation_results_and_real_fence() {
    for rename in [false, true] {
        for fail in [false, true] {
            let (_temp, handle, _, _) = fixture(b"payload");
            let parent = handle.root.path().join(if rename {
                "zxs-tail-rename-source"
            } else {
                "zxs-tail-unlink-source"
            });
            let destination_parent = handle.root.path().join("destination");
            handle.root.private_dir(&parent).unwrap();
            handle.root.private_dir(&destination_parent).unwrap();
            let source = parent.join("body");
            let target = destination_parent.join("body");
            let shared = parent.join("shared");
            fs::write(&source, b"payload").unwrap();
            fs::write(&shared, b"untouched shared bytes").unwrap();
            let (owner, competitor) = fence(&handle);
            let binding = handle.root.io_binding().unwrap();
            std::thread::scope(|threads| {
                let gate = SyscallGate::arm(&parent, fail);
                let work = threads.spawn(|| {
                    if rename {
                        handle.root.rename_noreplace_synced(&source, &target)
                    } else {
                        handle.root.remove_file_synced(&source)
                    }
                });
                gate.wait();
                assert!(!source.exists(), "mutation precedes its fsync tail");
                if rename {
                    assert_eq!(fs::read(&target).unwrap(), b"payload");
                }
                drop(owner);
                drop(binding.begin_quiesce().unwrap().unwrap());
                assert_eq!(binding.status().unwrap().ordinary, 1);
                assert!(descriptors(&parent) > 0);
                excluded(&competitor, true);
                gate.release();
                let error = work.join().unwrap().unwrap_err();
                if fail {
                    let Error::Contract(storage_contract::Error::Io { source, .. }) = error else {
                        panic!("fsync must preserve EIO: {error}")
                    };
                    assert_eq!(source.raw_os_error(), Some(5));
                } else {
                    assert_eq!(error.code(), ErrorCode::DiskUnavailable);
                }
                assert!(binding.status().unwrap().drained());
                assert_eq!(descriptors(&parent), 0);
                assert_eq!(descriptors(&destination_parent), 0);
                excluded(&competitor, false);
            });
            assert!(!source.exists());
            if rename {
                assert_eq!(fs::read(&target).unwrap(), b"payload");
            }
            assert_eq!(fs::read(&shared).unwrap(), b"untouched shared bytes");
        }
    }
}

#[test]
#[ignore = "requires test/engine/unit/container_storage/io_tails.c as LD_PRELOAD"]
fn copy_publication_fsync_cancellation_or_eio_never_cleans_formal_payload() {
    for fail in [false, true] {
        let bytes = vec![43u8; 131_089];
        let (temp, handle, item, _) = fixture(&bytes);
        let directory = handle.root.path().join("zxs-tail-copy-content");
        handle.root.private_dir(&directory).unwrap();
        let destination = directory.join("body");
        let external = temp.path().join("external");
        fs::write(&external, &bytes).unwrap();
        let (owner, competitor) = fence(&handle);
        let binding = handle.root.io_binding().unwrap();
        std::thread::scope(|threads| {
            let gate = SyscallGate::arm(&directory, fail);
            let work = threads.spawn(|| {
                verified_full_copy_at(
                    &handle.root,
                    File::open(&external).unwrap(),
                    &destination,
                    &item,
                    item.logical_size,
                )
            });
            gate.wait();
            assert_eq!(fs::read(&destination).unwrap(), bytes);
            assert_eq!(descriptors(&destination), 1);
            drop(owner);
            drop(binding.begin_quiesce().unwrap().unwrap());
            assert_eq!(binding.status().unwrap().ordinary, 1);
            excluded(&competitor, true);
            gate.release();
            assert!(matches!(
                work.join().unwrap().unwrap_err(),
                CopyFailure::Destination(_)
            ));
            assert!(binding.status().unwrap().drained());
            assert_eq!(descriptors(&destination), 0);
            assert_eq!(descriptors(&directory), 0);
            excluded(&competitor, false);
        });
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        assert_eq!(fs::read(&external).unwrap(), bytes);
        assert!(!fs::read_dir(&directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .as_encoded_bytes()
                .starts_with(b".zxs-full-")
        }));
    }
}
