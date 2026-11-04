//! Real caller/syscall-boundary regressions. Run on the Linux GitHub runner
//! with journal_io_tails.c preloaded; these are not physical disk-stall tests.
//! No test calls JournalIo::with_files or extracts a production descriptor.
use crate::{
    Config, DiskConfig, Error, ErrorCode, Result, ValidatedConfig, canonical_json,
    disk::{DiskHandle, DiskRoot, DiskWriter, FenceOwner, private_dir},
    journal::{JournalWriter, OpenJournal, Receipt, Record, RecordType, log_path},
};
use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Syscall {
    Read,
    Write,
    DataSync,
    DirectorySync,
    Truncate,
    CloseFile,
    CloseDirectory,
}

impl Syscall {
    fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::DataSync => "data-sync",
            Self::DirectorySync => "directory-sync",
            Self::Truncate => "truncate",
            Self::CloseFile => "close-file",
            Self::CloseDirectory => "close-directory",
        }
    }
}

struct SyscallGate(PathBuf);

impl SyscallGate {
    fn arm(path: &Path, point: Syscall, fail: bool) -> Self {
        assert!(
            std::env::var_os("LD_PRELOAD")
                .is_some_and(|value| value.to_string_lossy().contains("journal_io_tails")),
            "requires journal_io_tails.c as LD_PRELOAD; an unobserved gate is a failure"
        );
        assert!(!fail || !matches!(point, Syscall::CloseFile | Syscall::CloseDirectory));
        if fail {
            fs::write(path.join(".journal-tail-eio"), []).unwrap();
        }
        fs::write(path.join(format!(".journal-tail-arm-{}", point.name())), []).unwrap();
        Self(path.to_path_buf())
    }

    fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.0.join(".journal-tail-entered").exists() {
            assert!(
                Instant::now() < deadline,
                "caller never reached its syscall"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn release(&self) {
        fs::write(self.0.join(".journal-tail-release"), []).unwrap();
    }
}

impl Drop for SyscallGate {
    fn drop(&mut self) {
        // Declared inside the scope so a failed assertion releases the worker
        // before scope teardown joins it. The interposer also has a 30s bound.
        let _ = fs::write(self.0.join(".journal-tail-release"), []);
    }
}

fn fixture() -> (tempfile::TempDir, ValidatedConfig, DiskHandle) {
    let temp = tempfile::tempdir().unwrap();
    let config = ValidatedConfig::try_from(
        Config::development([temp.path().join("zxs-journal-syscall-tail")]).unwrap(),
    )
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

fn fence(root: &DiskRoot) -> (DiskWriter, File) {
    let lock = root.lock_file().unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    let owner = DiskWriter::adopt(root, FenceOwner::new(lock, None)).unwrap();
    let competitor = File::options()
        .read(true)
        .write(true)
        .open(root.path().join(".writer.lock"))
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

fn error<T>(result: Result<T>) -> Error {
    match result {
        Err(error) => error,
        Ok(_) => panic!("operation unexpectedly succeeded"),
    }
}

fn unavailable<T>(result: Result<T>) {
    assert_eq!(error(result).code(), ErrorCode::DiskUnavailable);
}

fn os_error(error: Error, errno: i32) {
    let Error::Contract(storage_contract::Error::Io { source, .. }) = error else {
        panic!("journal syscall lost its original I/O source: {error}");
    };
    assert_eq!(source.raw_os_error(), Some(errno));
}

fn append(writer: &mut JournalWriter, disk: &DiskConfig, kind: RecordType) -> Result<Receipt> {
    writer.append(disk, kind, 1, &"a".repeat(64), &"b".repeat(64))
}

fn accept_frame(previous: &Receipt) -> Vec<u8> {
    let record = Record {
        record_type: RecordType::Accept,
        sequence: 1,
        candidate_hash: "a".repeat(64),
        object_hash: "b".repeat(64),
        previous_frame_hash: previous.frame_hash.clone(),
    };
    let payload = canonical_json(&record).unwrap();
    let mut bytes = b"ZXSLOG\0\0".to_vec();
    bytes.extend(u32::try_from(payload.len()).unwrap().to_le_bytes());
    bytes.extend(payload);
    let hash = blake3::hash(&bytes);
    bytes.extend(hash.as_bytes());
    bytes
}

fn revoked(writer: &mut JournalWriter, handle: &DiskHandle, disk: &DiskConfig) {
    // Neither a cached receipt nor an otherwise no-op repair revives an owner.
    unavailable(append(writer, disk, RecordType::Prepare));
    unavailable(append(writer, disk, RecordType::Accept));
    unavailable(writer.repair_torn_tail());
    unavailable(OpenJournal::scan_at(&handle.root, disk));
}

fn blocked_tail<T: Send>(
    handle: &DiskHandle,
    disk: &DiskConfig,
    point: Syscall,
    fail: bool,
    run: impl FnOnce() -> T + Send,
) -> T {
    let path = log_path(disk);
    let parent = path.parent().unwrap();
    let binding = handle.root.io_binding().unwrap();
    let (owner, competitor) = fence(&handle.root);
    std::thread::scope(|threads| {
        let gate = SyscallGate::arm(parent, point, fail);
        let work = threads.spawn(run);
        gate.wait();
        assert_eq!(descriptors(&path), 1, "{point:?}");
        assert_eq!(descriptors(parent), 1, "{point:?}");
        drop(owner);
        drop(binding.begin_quiesce().unwrap().unwrap());
        assert_eq!(binding.status().unwrap().ordinary, 1, "{point:?}");
        assert!(!binding.status().unwrap().drained());
        assert_eq!(descriptors(&path), 1);
        assert_eq!(descriptors(parent), 1);
        assert!(descriptors(&disk.root) > 0);
        excluded(&competitor, true);
        // New operations must fail while the original admitted syscall is
        // still blocked, not only after its worker has returned.
        unavailable(OpenJournal::scan_at(&handle.root, disk));
        gate.release();
        let result = work.join().unwrap();
        assert_eq!(descriptors(&path), 0);
        assert_eq!(descriptors(parent), 0);
        assert_eq!(descriptors(&disk.root), 0);
        assert!(binding.status().unwrap().drained());
        excluded(&competitor, false);
        result
    })
}

// This is a fresh, unbound offline reopen after the old domain has drained,
// not an attempt to reactivate the revoked handle or certify online removal.
fn recover_accept(disk: &DiskConfig, before: &[u8], frame: &[u8]) {
    let opened = OpenJournal::scan(disk).unwrap();
    assert_eq!(fs::read(log_path(disk)).unwrap(), before);
    let (mut recovered, _) = opened.into_writer().unwrap();
    recovered.repair_torn_tail().unwrap();
    let receipt = append(&mut recovered, disk, RecordType::Accept).unwrap();
    assert_eq!(receipt.record.record_type, RecordType::Accept);
    assert_eq!(
        receipt.frame_hash,
        blake3::Hash::from_bytes(frame[frame.len() - 32..].try_into().unwrap())
            .to_hex()
            .as_str()
    );
    drop(recovered);
    let scanned = OpenJournal::scan(disk).unwrap();
    assert!(!scanned.scan.torn_tail);
    assert_eq!(scanned.scan.records.len(), 2);
}

#[test]
#[ignore = "requires test/engine/unit/journal/journal_io_tails.c as LD_PRELOAD"]
fn append_write_and_sync_keep_receipts_errors_and_fence_until_syscall_returns() {
    for point in [Syscall::Write, Syscall::DataSync] {
        for fail in [false, true] {
            let (_temp, config, handle) = fixture();
            let disk = &config.disks[0];
            let path = log_path(disk);
            let mut writer = JournalWriter::create_at(&handle.root, disk).unwrap();
            let first = append(&mut writer, disk, RecordType::Prepare).unwrap();
            let prefix = fs::read(&path).unwrap();
            let frame = accept_frame(&first);
            let result = blocked_tail(&handle, disk, point, fail, || {
                append(&mut writer, disk, RecordType::Accept)
            });
            assert_eq!(writer.requires_reopen(), fail);
            if fail {
                os_error(error(result), 5);
                assert_eq!(writer.records().len(), 1);
            } else {
                let receipt = result.unwrap();
                assert_eq!(receipt.record.previous_frame_hash, first.frame_hash);
                assert_eq!(writer.records().last().unwrap().0, receipt.record);
                assert_eq!(writer.records().len(), 2);
            }
            let expected = if fail && point == Syscall::Write {
                prefix.clone()
            } else {
                [prefix.as_slice(), frame.as_slice()].concat()
            };
            assert_eq!(fs::read(&path).unwrap(), expected);
            revoked(&mut writer, &handle, disk);
            assert_eq!(fs::read(&path).unwrap(), expected);
            drop(writer);
            recover_accept(disk, &expected, &frame);
            assert_eq!(fs::read(&path).unwrap(), [prefix, frame].concat());
        }
    }
}

#[test]
#[ignore = "requires test/engine/unit/journal/journal_io_tails.c as LD_PRELOAD"]
fn failed_running_append_requires_reopen_before_retry_or_cached_receipt() {
    for point in [Syscall::Write, Syscall::DataSync] {
        let (_temp, config, handle) = fixture();
        let disk = &config.disks[0];
        let path = log_path(disk);
        let mut writer = JournalWriter::create_at(&handle.root, disk).unwrap();
        let first = append(&mut writer, disk, RecordType::Prepare).unwrap();
        let prefix = fs::read(&path).unwrap();
        let frame = accept_frame(&first);
        let (owner, competitor) = fence(&handle.root);
        let result = std::thread::scope(|threads| {
            let gate = SyscallGate::arm(path.parent().unwrap(), point, true);
            let work = threads.spawn(|| append(&mut writer, disk, RecordType::Accept));
            gate.wait();
            excluded(&competitor, true);
            gate.release();
            work.join().unwrap()
        });
        os_error(error(result), 5);
        assert!(writer.requires_reopen());
        assert_eq!(writer.records().len(), 1);
        let bytes = fs::read(&path).unwrap();
        let expected = if point == Syscall::Write {
            prefix.clone()
        } else {
            [prefix.as_slice(), frame.as_slice()].concat()
        };
        assert_eq!(bytes, expected);
        for kind in [RecordType::Prepare, RecordType::Accept] {
            assert_eq!(
                error(append(&mut writer, disk, kind)).code(),
                ErrorCode::RecoveryRequired
            );
        }
        assert_eq!(
            error(writer.repair_torn_tail()).code(),
            ErrorCode::RecoveryRequired
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        drop(writer);
        assert_eq!(descriptors(&path), 0);
        assert_eq!(descriptors(path.parent().unwrap()), 0);
        excluded(&competitor, true);
        drop(owner);
        excluded(&competitor, false);
        drop(competitor);
        recover_accept(disk, &bytes, &frame);
        assert_eq!(fs::read(&path).unwrap(), [prefix, frame].concat());
    }
}

#[cfg(feature = "fault-injection")]
#[test]
#[ignore = "requires test/engine/unit/journal/journal_io_tails.c as LD_PRELOAD"]
fn partial_append_blocked_in_write_preserves_enospc_and_exact_torn_prefix() {
    let (_temp, config, handle) = fixture();
    let disk = &config.disks[0];
    let path = log_path(disk);
    let mut writer = JournalWriter::create_at(&handle.root, disk).unwrap();
    let first = append(&mut writer, disk, RecordType::Prepare).unwrap();
    let prefix = fs::read(&path).unwrap();
    let frame = accept_frame(&first);
    writer.fault = Some(crate::journal::JournalFault::PartialWrite);
    let result = blocked_tail(&handle, disk, Syscall::Write, false, || {
        append(&mut writer, disk, RecordType::Accept)
    });
    os_error(error(result), 28);
    assert!(writer.requires_reopen());
    assert_eq!(writer.records().len(), 1);
    let expected = [prefix.as_slice(), &frame[..frame.len() / 2]].concat();
    assert_eq!(fs::read(&path).unwrap(), expected);
    revoked(&mut writer, &handle, disk);
    drop(writer);
    let opened = OpenJournal::scan(disk).unwrap();
    assert!(opened.scan.torn_tail);
    assert_eq!(opened.scan.valid_bytes, prefix.len() as u64);
    assert_eq!(opened.scan.records.len(), 1);
    drop(opened);
    recover_accept(disk, &expected, &frame);
    assert_eq!(fs::read(&path).unwrap(), [prefix, frame].concat());
}

#[test]
#[ignore = "requires test/engine/unit/journal/journal_io_tails.c as LD_PRELOAD"]
fn into_writer_open_syncs_keep_new_and_existing_leaf_fenced_until_tail() {
    for existing in [false, true] {
        for point in [Syscall::DataSync, Syscall::DirectorySync] {
            for fail in [false, true] {
                let (_temp, config, handle) = fixture();
                let disk = &config.disks[0];
                let path = log_path(disk);
                if existing {
                    let mut writer = JournalWriter::create_at(&handle.root, disk).unwrap();
                    append(&mut writer, disk, RecordType::Prepare).unwrap();
                }
                let bytes = if existing {
                    fs::read(&path).unwrap()
                } else {
                    Vec::new()
                };
                let opened = OpenJournal::scan_at(&handle.root, disk).unwrap();
                assert_eq!(path.exists(), existing);
                assert_eq!(opened.scan.records.len(), usize::from(existing));
                let result = blocked_tail(&handle, disk, point, fail, || opened.into_writer());
                if fail {
                    os_error(error(result), 5);
                } else {
                    let (mut writer, records) = result.unwrap();
                    assert_eq!(records.len(), usize::from(existing));
                    assert!(!writer.requires_reopen());
                    revoked(&mut writer, &handle, disk);
                }
                assert_eq!(fs::read(&path).unwrap(), bytes);
                let (mut reopened, _) = OpenJournal::scan(disk).unwrap().into_writer().unwrap();
                append(&mut reopened, disk, RecordType::Prepare).unwrap();
                drop(reopened);
                assert_eq!(OpenJournal::scan(disk).unwrap().scan.records.len(), 1);
            }
        }
    }
}

#[test]
#[ignore = "requires test/engine/unit/journal/journal_io_tails.c as LD_PRELOAD"]
fn repair_truncate_and_sync_keep_exact_prefix_and_reopen_state_after_stop() {
    for point in [Syscall::Truncate, Syscall::DataSync] {
        for fail in [false, true] {
            let (_temp, config, handle) = fixture();
            let disk = &config.disks[0];
            let path = log_path(disk);
            let mut writer = JournalWriter::create_at(&handle.root, disk).unwrap();
            let first = append(&mut writer, disk, RecordType::Prepare).unwrap();
            drop(writer);
            let prefix = fs::read(&path).unwrap();
            File::options()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"ZXSLOG\0\0")
                .unwrap();
            let torn = fs::read(&path).unwrap();
            let opened = OpenJournal::scan_at(&handle.root, disk).unwrap();
            assert!(opened.scan.torn_tail);
            assert_eq!(opened.scan.valid_bytes, prefix.len() as u64);
            let (mut writer, _) = opened.into_writer().unwrap();
            let result = blocked_tail(&handle, disk, point, fail, || writer.repair_torn_tail());
            assert_eq!(writer.requires_reopen(), fail);
            assert_eq!(writer.needs_recovery(), fail);
            if fail {
                os_error(error(result), 5);
            } else {
                result.unwrap();
            }
            assert_eq!(writer.records().len(), 1);
            let expected = if fail && point == Syscall::Truncate {
                &torn
            } else {
                &prefix
            };
            assert_eq!(&fs::read(&path).unwrap(), expected);
            revoked(&mut writer, &handle, disk);
            drop(writer);
            let frame = accept_frame(&first);
            recover_accept(disk, expected, &frame);
            assert_eq!(fs::read(&path).unwrap(), [prefix, frame].concat());
        }
    }
}

#[test]
#[ignore = "requires test/engine/unit/journal/journal_io_tails.c as LD_PRELOAD"]
fn scan_read_cancellation_preserves_syscall_error_and_unchanged_bytes() {
    for fail in [false, true] {
        let (_temp, config, handle) = fixture();
        let disk = &config.disks[0];
        let path = log_path(disk);
        let mut writer = JournalWriter::create_at(&handle.root, disk).unwrap();
        append(&mut writer, disk, RecordType::Prepare).unwrap();
        drop(writer);
        let bytes = fs::read(&path).unwrap();
        let result = blocked_tail(&handle, disk, Syscall::Read, fail, || {
            OpenJournal::scan_at(&handle.root, disk)
        });
        if fail {
            os_error(error(result), 5);
        } else {
            unavailable(result);
        }
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let scanned = OpenJournal::scan(disk).unwrap();
        assert!(!scanned.scan.torn_tail);
        assert_eq!(scanned.scan.records.len(), 1);
    }
}

#[test]
#[ignore = "requires test/engine/unit/journal/journal_io_tails.c as LD_PRELOAD"]
fn idle_writer_close_syscalls_keep_original_flock_until_file_and_parent_close() {
    for point in [Syscall::CloseFile, Syscall::CloseDirectory] {
        let (_temp, config, handle) = fixture();
        let disk = &config.disks[0];
        let path = log_path(disk);
        let parent = path.parent().unwrap();
        let mut writer = JournalWriter::create_at(&handle.root, disk).unwrap();
        append(&mut writer, disk, RecordType::Prepare).unwrap();
        let bytes = fs::read(&path).unwrap();
        let binding = handle.root.io_binding().unwrap();
        let (owner, competitor) = fence(&handle.root);
        std::thread::scope(|threads| {
            let gate = SyscallGate::arm(parent, point, false);
            drop(owner);
            // The idle caller is never polled/dropped. Quiesce itself must
            // close its descriptors, retaining accounting until real close.
            let work = threads.spawn(|| drop(binding.begin_quiesce().unwrap().unwrap()));
            gate.wait();
            assert_eq!(descriptors(&path), usize::from(point == Syscall::CloseFile));
            assert_eq!(descriptors(parent), 1);
            assert_eq!(binding.status().unwrap().ordinary, 1);
            assert!(!binding.status().unwrap().drained());
            excluded(&competitor, true);
            gate.release();
            work.join().unwrap();
            assert!(binding.status().unwrap().drained());
            assert_eq!(descriptors(&path), 0);
            assert_eq!(descriptors(parent), 0);
            assert_eq!(descriptors(&disk.root), 0);
            excluded(&competitor, false);
        });
        revoked(&mut writer, &handle, disk);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
