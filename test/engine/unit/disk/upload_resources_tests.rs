use super::*;
use crate::{Config, ValidatedConfig, disk::DiskHandle};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{Arc, mpsc},
    time::Duration,
};

fn fixture() -> (
    tempfile::TempDir,
    Arc<DiskHandle>,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("disk");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let config = ValidatedConfig::try_from(Config::development([root.clone()]).unwrap()).unwrap();
    let handle = Arc::new(DiskHandle::open(&config, &config.disks[0], false).unwrap());
    let directory = root.join("workspace");
    handle.root.private_dir(&directory).unwrap();
    let source = temp.path().join("external-source");
    fs::write(&source, vec![37u8; 2 * 65536 + 17]).unwrap();
    (temp, handle, directory, source)
}

fn descriptors(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter(|entry| {
            let entry = entry.as_ref().unwrap();
            match fs::read_link(entry.path()) {
                Ok(target) => {
                    target == path
                        || target.as_os_str() == format!("{} (deleted)", path.display()).as_str()
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => panic!("cannot inspect test descriptor: {error}"),
            }
        })
        .count()
}

fn name(value: &str) -> EntryName {
    EntryName::new(value).unwrap()
}

#[test]
fn idle_upload_closes_its_real_directory_and_external_descriptor_on_revocation() {
    let (_temp, handle, directory, source) = fixture();
    let upload = UploadIo::new(&handle.root, &directory, File::open(&source).unwrap()).unwrap();
    let binding = handle.root.io_binding().unwrap();
    assert_eq!(binding.status().unwrap().ordinary, 1);
    assert_eq!(descriptors(&directory), 1);
    assert_eq!(descriptors(&source), 1);
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(descriptors(&directory), 0);
    assert_eq!(descriptors(&source), 0);
    assert_eq!(
        upload.fingerprint_source(u64::MAX).unwrap_err().code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(
        upload
            .copy(name("scratch.prepared"), 65536, u64::MAX)
            .unwrap_err()
            .code(),
        ErrorCode::DiskUnavailable
    );
    assert!(fs::read_dir(&directory).unwrap().next().is_none());
    assert_eq!(fs::read(&source).unwrap().len(), 2 * 65536 + 17);
}

#[test]
fn verified_scratch_is_cleaned_before_drain_and_late_drop_cannot_write_again() {
    let (_temp, handle, directory, source) = fixture();
    let upload = UploadIo::new(&handle.root, &directory, File::open(&source).unwrap()).unwrap();
    let scratch = directory.join("scratch.prepared");
    upload
        .copy(name("scratch.prepared"), 65536, u64::MAX)
        .unwrap();
    assert_eq!(descriptors(&scratch), 1);
    assert_eq!(
        fs::metadata(&scratch).unwrap().permissions().mode() & 0o777,
        0o400
    );
    let binding = handle.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(descriptors(&scratch), 0);
    assert_eq!(descriptors(&directory), 0);
    assert!(!scratch.exists());
    fs::write(&scratch, b"later unrelated file").unwrap();
    drop(upload);
    assert_eq!(fs::read(&scratch).unwrap(), b"later unrelated file");
    assert!(source.exists());
}

#[test]
fn retained_verification_revokes_the_fd_without_removing_published_bytes() {
    let (_temp, handle, directory, source) = fixture();
    let path = directory.join("retained.full");
    fs::copy(&source, &path).unwrap();
    let bytes = fs::read(&source).unwrap();
    let hash = ContentHash::from_digest(blake3::hash(&bytes));
    let upload = UploadIo::new(&handle.root, &directory, File::open(&source).unwrap()).unwrap();
    upload
        .verify_retained(&name("retained.full"), bytes.len() as u64, &hash, u64::MAX)
        .unwrap();
    assert_eq!(descriptors(&path), 1);
    let binding = handle.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(descriptors(&path), 0);
    assert_eq!(descriptors(&directory), 0);
    drop(upload);
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn cancellation_during_copy_keeps_real_resources_counted_until_cleanup_tail() {
    let (_temp, handle, directory, source) = fixture();
    let upload = UploadIo::new(&handle.root, &directory, File::open(&source).unwrap()).unwrap();
    let writer = handle.root.lock_file().unwrap();
    rustix::fs::flock(
        &writer,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    )
    .unwrap();
    let fence =
        crate::disk::DiskWriter::adopt(&handle.root, crate::disk::FenceOwner::new(writer, None))
            .unwrap();
    let competitor = File::options()
        .read(true)
        .write(true)
        .open(handle.root.path().join(".writer.lock"))
        .unwrap();
    let binding = handle.root.io_binding().unwrap();
    let timeout = Duration::from_secs(30);
    std::thread::scope(|threads| {
        let (entered, entered_rx) = mpsc::channel();
        // Scope-local ownership drops this sender before scoped joins if an
        // inspection assertion panics, releasing the waiting copy immediately.
        let (release, release_rx) = mpsc::channel();
        let copy_upload = &upload;
        let copying = threads.spawn(move || {
            let mut access = copy_upload.resources.access_checked().unwrap();
            let mut checkpoints = 0;
            access
                .value()
                .resource
                .copy(name("scratch.prepared"), 65536, u64::MAX, &mut || {
                    checkpoints += 1;
                    if checkpoints == 5 {
                        entered
                            .send(())
                            .expect("copy observer still has its receiver");
                        release_rx
                            .recv_timeout(timeout)
                            .expect("copy observer released the checkpoint");
                    }
                    copy_upload.resources.check_running()
                })
        });
        entered_rx
            .recv_timeout(timeout)
            .expect("copy reached checkpoint five");
        let temporary = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .as_encoded_bytes()
                    .starts_with(b".zxs-full-")
            })
            .unwrap();
        let written = File::open(&temporary).unwrap();
        assert_eq!(written.metadata().unwrap().len(), 65536);
        // Removing the Engine's owner cannot release a copying worker's fence.
        drop(fence);
        let transition = binding.begin_quiesce().unwrap().unwrap();
        assert_eq!(
            rustix::fs::flock(
                &competitor,
                rustix::fs::FlockOperation::NonBlockingLockExclusive
            )
            .unwrap_err(),
            rustix::io::Errno::WOULDBLOCK
        );
        assert_eq!(binding.status().unwrap().ordinary, 1);
        assert_eq!(descriptors(&directory), 1);
        assert_eq!(descriptors(&temporary), 2);
        assert!(temporary.exists());
        release
            .send(())
            .expect("copy still waits at checkpoint five");
        assert_eq!(
            copying.join().unwrap().unwrap_err().code(),
            ErrorCode::DiskUnavailable
        );
        assert_eq!(
            written.metadata().unwrap().len(),
            65536,
            "no chunk was written after cancellation was observed"
        );
        assert_eq!(
            descriptors(&temporary),
            1,
            "only the test's inspection descriptor remains"
        );
        assert_eq!(descriptors(&directory), 0);
        assert_eq!(descriptors(&source), 0);
        assert!(fs::read_dir(&directory).unwrap().next().is_none());
        assert_eq!(
            rustix::fs::flock(
                &competitor,
                rustix::fs::FlockOperation::NonBlockingLockExclusive
            )
            .unwrap_err(),
            rustix::io::Errno::WOULDBLOCK
        );
        drop(transition);
        assert!(binding.status().unwrap().drained());
        rustix::fs::flock(
            &competitor,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )
        .unwrap();
    });
}

#[test]
fn cleanup_never_removes_replaced_scratch_or_replacement_directory_files() {
    for replace_directory in [false, true] {
        let (_temp, handle, directory, source) = fixture();
        let upload = UploadIo::new(&handle.root, &directory, File::open(&source).unwrap()).unwrap();
        let scratch = directory.join("scratch.prepared");
        upload
            .copy(name("scratch.prepared"), 65536, u64::MAX)
            .unwrap();
        let moved = if replace_directory {
            let moved = directory.with_extension("moved");
            fs::rename(&directory, &moved).unwrap();
            fs::create_dir(&directory).unwrap();
            moved.join("scratch.prepared")
        } else {
            let moved = scratch.with_extension("moved");
            fs::rename(&scratch, &moved).unwrap();
            moved
        };
        fs::write(&scratch, b"unrelated replacement").unwrap();
        drop(upload);
        assert_eq!(fs::read(&scratch).unwrap(), b"unrelated replacement");
        assert_eq!(moved.exists(), !replace_directory);
        assert!(source.exists());
    }
}

#[test]
fn cancellation_after_promotion_preserves_the_immutable_generation_for_retry() {
    let (_temp, handle, directory, source) = fixture();
    let upload = UploadIo::new(&handle.root, &directory, File::open(&source).unwrap()).unwrap();
    upload
        .copy(name("scratch.prepared"), 65536, u64::MAX)
        .unwrap();
    let target = directory.join("retained.full");
    let binding = handle.root.io_binding().unwrap();
    let result = {
        let mut access = upload.resources.access_checked().unwrap();
        let resources = &mut access.value().resource;
        let identity = resources.directory.identity();
        resources.finish(
            &name("retained.full"),
            identity,
            UploadFinish::Publish {
                max_workspace_bytes: u64::MAX,
                max_entries: usize::MAX,
            },
            &mut || {
                if target.exists() {
                    drop(binding.begin_quiesce()?.unwrap());
                }
                upload.resources.check_running()
            },
        )
    };
    assert_eq!(result.unwrap_err().code(), ErrorCode::DiskUnavailable);
    assert!(binding.status().unwrap().drained());
    assert_eq!(descriptors(&target), 0);
    assert_eq!(fs::read(&target).unwrap(), fs::read(&source).unwrap());
    assert!(!directory.join("scratch.prepared").exists());
}

#[test]
fn failed_copy_keeps_existing_destination_and_releases_only_its_temporary() {
    for maximum in [1, u64::MAX] {
        let (_temp, handle, directory, source) = fixture();
        let scratch = directory.join("scratch.prepared");
        fs::write(&scratch, b"preexisting file").unwrap();
        let upload = UploadIo::new(&handle.root, &directory, File::open(&source).unwrap()).unwrap();
        assert!(
            upload
                .copy(name("scratch.prepared"), 65536, maximum)
                .is_err()
        );
        drop(upload);
        assert_eq!(fs::read(&scratch).unwrap(), b"preexisting file");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        assert_eq!(descriptors(&directory), 0);
        assert_eq!(
            handle.root.io_binding().unwrap().status().unwrap().ordinary,
            0
        );
    }
}

#[test]
fn unwinding_copy_keeps_temporary_cleanup_owned_until_revocation() {
    let (_temp, handle, directory, source) = fixture();
    let upload = UploadIo::new(&handle.root, &directory, File::open(&source).unwrap()).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut access = upload.resources.access_checked().unwrap();
        let mut checkpoints = 0;
        let _ =
            access
                .value()
                .resource
                .copy(name("scratch.prepared"), 65536, u64::MAX, &mut || {
                    checkpoints += 1;
                    assert_ne!(
                        checkpoints, 5,
                        "stop a live copy after its first written block"
                    );
                    Ok(())
                });
    }));
    assert!(result.is_err());
    let binding = handle.root.io_binding().unwrap();
    assert_eq!(binding.status().unwrap().ordinary, 1);
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(descriptors(&directory), 0);
    assert!(fs::read_dir(&directory).unwrap().next().is_none());
    assert!(source.exists());
}

#[test]
fn cancellation_after_scratch_rename_cleans_the_current_owned_name() {
    let (_temp, handle, directory, source) = fixture();
    let upload = UploadIo::new(&handle.root, &directory, File::open(&source).unwrap()).unwrap();
    let scratch = directory.join("scratch.prepared");
    let binding = handle.root.io_binding().unwrap();
    let result = {
        let mut access = upload.resources.access_checked().unwrap();
        access
            .value()
            .resource
            .copy(name("scratch.prepared"), 65536, u64::MAX, &mut || {
                if scratch.exists() {
                    drop(binding.begin_quiesce()?.unwrap());
                }
                upload.resources.check_running()
            })
    };
    assert_eq!(result.unwrap_err().code(), ErrorCode::DiskUnavailable);
    assert!(binding.status().unwrap().drained());
    assert_eq!(descriptors(&scratch), 0);
    assert_eq!(descriptors(&directory), 0);
    assert!(fs::read_dir(&directory).unwrap().next().is_none());
    assert!(source.exists());
}
