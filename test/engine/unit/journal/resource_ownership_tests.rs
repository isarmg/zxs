use super::*;
use crate::{
    Config, DiskConfig,
    disk::{DiskHandle, DiskRoot, DiskWriter, FenceOwner, private_dir},
    journal::{JournalFrameReader, JournalWriter, OpenJournal, RecordType, log_path, scan_at},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(10);

// This test-only extension preserves the real ENOSPC regression without adding
// a raw-file extraction or replacement API to production JournalIo.
impl JournalIo {
    pub(crate) fn inject_full_device(&mut self) {
        self.with_files(Access::Write, |files, _| {
            files.file = Some(File::options().write(true).open("/dev/full")?);
            Ok(())
        })
        .unwrap();
    }
}

fn fixture() -> (tempfile::TempDir, ValidatedConfig, DiskHandle) {
    let temp = tempfile::tempdir().unwrap();
    let config =
        ValidatedConfig::try_from(Config::development([temp.path().join("disk")]).unwrap())
            .unwrap();
    private_dir(log_path(&config.disks[0]).parent().unwrap()).unwrap();
    let handle = DiskHandle::open(&config, &config.disks[0], false).unwrap();
    (temp, config, handle)
}

fn descriptors(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter(
            |entry| match fs::read_link(entry.as_ref().unwrap().path()) {
                Ok(target) => target == path,
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => panic!("cannot inspect journal descriptor: {error}"),
            },
        )
        .count()
}

fn append(writer: &mut JournalWriter, disk: &DiskConfig) -> Result<crate::journal::Receipt> {
    writer.append(
        disk,
        RecordType::Prepare,
        1,
        &"a".repeat(64),
        &"b".repeat(64),
    )
}

fn io(root: &DiskRoot, disk: &DiskConfig) -> JournalIo {
    root.journal_directory(log_path(disk).parent().unwrap())
        .unwrap()
}

fn unavailable<T>(result: Result<T>) {
    assert!(matches!(result, Err(error) if error.code() == ErrorCode::DiskUnavailable));
}

#[test]
fn idle_writer_and_open_scan_close_real_files_and_parents_without_polling() {
    let (_temp, config, handle) = fixture();
    let disk = &config.disks[0];
    let mut writer = JournalWriter::create_at(&handle.root, disk).unwrap();
    append(&mut writer, disk).unwrap();
    let opened = OpenJournal::scan_at(&handle.root, disk).unwrap();
    let path = log_path(disk);
    let parent = path.parent().unwrap();
    let bytes = fs::read(&path).unwrap();
    assert_eq!(descriptors(&path), 2);
    assert_eq!(descriptors(parent), 2);
    let binding = handle.root.io_binding().unwrap();
    assert_eq!(binding.status().unwrap().ordinary, 2);

    drop(binding.begin_quiesce().unwrap().unwrap());
    assert_eq!(descriptors(&path), 0);
    assert_eq!(descriptors(parent), 0);
    assert_eq!(descriptors(&disk.root), 0);
    assert!(binding.status().unwrap().drained());
    for _ in 0..2 {
        // Even an idempotent receipt cannot revive a revoked writer.
        unavailable(append(&mut writer, disk));
        unavailable(writer.repair_torn_tail());
        unavailable(scan_at(&handle.root, disk));
    }
    unavailable(opened.into_writer());
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn missing_leaf_scan_cannot_create_after_revocation() {
    let (_temp, config, handle) = fixture();
    let disk = &config.disks[0];
    let opened = OpenJournal::scan_at(&handle.root, disk).unwrap();
    let path = log_path(disk);
    assert!(!path.exists());
    assert_eq!(descriptors(path.parent().unwrap()), 1);
    let binding = handle.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert_eq!(descriptors(path.parent().unwrap()), 0);
    unavailable(opened.into_writer());
    assert!(!path.exists());
    assert!(binding.status().unwrap().drained());
}

#[test]
fn idle_read_scanner_and_file_clone_cannot_escape_revocation() {
    let (_temp, config, handle) = fixture();
    let disk = &config.disks[0];
    let mut writer = JournalWriter::create_at(&handle.root, disk).unwrap();
    append(&mut writer, disk).unwrap();
    drop(writer);
    let path = log_path(disk);
    let file = handle
        .root
        .open_optional_read_regular(&path)
        .unwrap()
        .unwrap();
    let mut copy = file.try_clone().unwrap();
    let mut reader = JournalFrameReader::new(file);
    assert_eq!(descriptors(&path), 2);
    let binding = handle.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert_eq!(descriptors(&path), 0);
    assert!(binding.status().unwrap().drained());
    for _ in 0..2 {
        assert!(reader.next_record().is_err());
        assert_eq!(
            copy.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(copy.try_clone().is_err());
    }
}

struct Release(Option<mpsc::SyncSender<()>>);
impl Drop for Release {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            // The bounded worker wait also terminates if its parent panics.
            let _ = release.try_send(());
        }
    }
}

fn fence(root: &DiskRoot) -> (DiskWriter, PathBuf) {
    let path = root.path().join(".writer.lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    (
        DiskWriter::adopt(root, FenceOwner::new(file, None)).unwrap(),
        path,
    )
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

#[derive(Clone, Copy)]
enum Tail {
    Append,
    Read,
    Sync,
    Repair,
}

#[test]
fn busy_append_read_sync_and_repair_keep_original_fence_until_real_cleanup() {
    for action in [Tail::Append, Tail::Read, Tail::Sync, Tail::Repair] {
        let (_temp, config, handle) = fixture();
        let disk = &config.disks[0];
        let (writer, lock_path) = fence(&handle.root);
        let mut journal = io(&handle.root, disk);
        journal.create().unwrap();
        let path = log_path(disk);
        let binding = handle.root.io_binding().unwrap();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (mut journal, result) = std::thread::scope(|threads| {
            let worker = threads.spawn(move || {
                let access = if matches!(action, Tail::Read) {
                    Access::Scan
                } else {
                    Access::Write
                };
                let result = journal.with_files(access, |files, observe| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv_timeout(WAIT).unwrap();
                    match action {
                        Tail::Append => {
                            files.file().write_all(b"unfinished tail")?;
                            files.file().sync_data()?;
                        }
                        Tail::Read => {
                            // The scanner checks cancellation before its next
                            // bounded read, while still owning both real FDs.
                            observe(0)?;
                            let bytes = files.file().read(&mut [0])?;
                            observe(bytes)?;
                        }
                        Tail::Sync => files.sync_open(
                            #[cfg(feature = "fault-injection")]
                            None,
                        )?,
                        Tail::Repair => files.repair(0)?,
                    }
                    Ok(())
                });
                (journal, result)
            });
            let release = Release(Some(release_tx));
            entered_rx.recv_timeout(WAIT).unwrap();
            drop(binding.begin_quiesce().unwrap().unwrap());
            drop(writer);
            assert_eq!(binding.status().unwrap().ordinary, 1);
            assert!(!binding.status().unwrap().drained());
            assert_eq!(descriptors(&path), 1);
            assert_eq!(descriptors(path.parent().unwrap()), 1);
            excluded(&lock_path, true);
            drop(release);
            worker.join().unwrap()
        });
        if matches!(action, Tail::Read) {
            unavailable(result);
        } else {
            result.unwrap();
        }
        assert_eq!(descriptors(&path), 0);
        assert_eq!(descriptors(path.parent().unwrap()), 0);
        assert_eq!(descriptors(&disk.root), 0);
        assert!(binding.status().unwrap().drained());
        excluded(&lock_path, false);
        unavailable(journal.repair(0));
    }
}

#[test]
fn scan_checks_revocation_between_bounded_reads_and_closes_its_real_tail() {
    let (_temp, config, handle) = fixture();
    let disk = &config.disks[0];
    let mut writer = JournalWriter::create_at(&handle.root, disk).unwrap();
    append(&mut writer, disk).unwrap();
    drop(writer);
    let mut journal = io(&handle.root, disk);
    journal.scan(JournalNamespace::Active).unwrap();
    let path = log_path(disk);
    let binding = handle.root.io_binding().unwrap();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let result = std::thread::scope(|threads| {
        let worker = threads.spawn(move || {
            journal.with_files(Access::Scan, |files, observe| {
                let mut file = files.file();
                file.rewind()?;
                let mut entered = false;
                scan_frames_observed(
                    io::BufReader::new(file),
                    JournalNamespace::Active,
                    &mut |bytes| {
                        if bytes != 0 && !entered {
                            entered = true;
                            entered_tx.send(()).unwrap();
                            release_rx.recv_timeout(WAIT).unwrap();
                        }
                        observe(bytes)
                    },
                )
            })
        });
        let release = Release(Some(release_tx));
        entered_rx.recv_timeout(WAIT).unwrap();
        drop(binding.begin_quiesce().unwrap().unwrap());
        assert_eq!(binding.status().unwrap().ordinary, 1);
        assert_eq!(descriptors(&path), 1);
        assert!(!binding.status().unwrap().drained());
        drop(release);
        worker.join().unwrap()
    });
    unavailable(result);
    assert_eq!(descriptors(&path), 0);
    assert_eq!(descriptors(path.parent().unwrap()), 0);
    assert!(binding.status().unwrap().drained());
}

#[test]
fn late_registration_closes_directory_before_the_opening_lease_retires() {
    let (_temp, config, handle) = fixture();
    let path = log_path(&config.disks[0]);
    let binding = handle.root.io_binding().unwrap();
    let lease = binding.admit().unwrap();
    let directory = handle.root.directory(path.parent().unwrap()).unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert_eq!(binding.status().unwrap().ordinary, 1);
    unavailable(JournalIo::new(directory, Some(lease)));
    assert_eq!(descriptors(path.parent().unwrap()), 0);
    assert!(binding.status().unwrap().drained());
}

#[test]
fn unverified_authority_can_read_but_cannot_append_through_a_shared_gate() {
    let (_temp, config, handle) = fixture();
    let disk = &config.disks[0];
    let mut writer = JournalWriter::create_at(&handle.root, disk).unwrap();
    append(&mut writer, disk).unwrap();
    let path = log_path(disk);
    let bytes = fs::read(&path).unwrap();
    let mut changed = (*config).clone();
    let mut extra = Config::development([disk.root.with_file_name("extra")])
        .unwrap()
        .disks
        .remove(0);
    extra.slot_id = "B01".into();
    extra.level = 2;
    extra.upstream_slot_id = Some("P01".into());
    changed.disks.push(extra);
    let changed = ValidatedConfig::try_from(changed).unwrap();
    let other = DiskHandle::open(&changed, &changed.disks[0], false).unwrap();
    let opened = OpenJournal::scan_at(&other.root, disk).unwrap();
    assert_eq!(opened.scan.records.len(), 1);
    assert!(
        matches!(opened.into_writer(), Err(error) if error.code() == ErrorCode::ConfigConflict)
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
    append(&mut writer, disk).unwrap();
}

#[cfg(feature = "fault-injection")]
#[test]
fn failed_open_and_partial_append_release_managed_owners_and_preserve_recovery() {
    for fault in [OpeningFault::FileSync, OpeningFault::DirectorySync] {
        let (_temp, config, handle) = fixture();
        let disk = &config.disks[0];
        let path = log_path(disk);
        let mut journal = io(&handle.root, disk);
        assert!(journal.finish_open(Some(fault)).is_err());
        drop(journal);
        assert_eq!(descriptors(&path), 0);
        assert_eq!(descriptors(path.parent().unwrap()), 0);
        assert_eq!(
            handle.root.io_binding().unwrap().status().unwrap().ordinary,
            0
        );
        let (mut writer, _) = OpenJournal::scan_at(&handle.root, disk)
            .unwrap()
            .into_writer()
            .unwrap();
        writer.fault = Some(JournalFault::PartialWrite);
        assert!(append(&mut writer, disk).is_err());
        assert!(writer.requires_reopen());
        let bytes = fs::read(&path).unwrap();
        assert_eq!(
            append(&mut writer, disk).unwrap_err().code(),
            ErrorCode::RecoveryRequired
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        drop(writer);
        let (mut recovered, _) = OpenJournal::scan_at(&handle.root, disk)
            .unwrap()
            .into_writer()
            .unwrap();
        recovered.repair_torn_tail().unwrap();
        append(&mut recovered, disk).unwrap();
        assert_eq!(scan_at(&handle.root, disk).unwrap().records.len(), 1);
        let binding = handle.root.io_binding().unwrap();
        drop(binding.begin_quiesce().unwrap().unwrap());
        assert_eq!(descriptors(&path), 0);
        assert!(binding.status().unwrap().drained());
    }
}
