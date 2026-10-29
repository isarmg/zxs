use super::*;
#[cfg(feature = "fault-injection")]
use std::cell::Cell;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// Timeouts are deadlock watchdogs only; ordering uses gates and counters.
const WATCHDOG: Duration = Duration::from_secs(30);
const CHUNK: u32 = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProofMoment {
    Before,
    After,
}

#[derive(Clone, Copy, Debug)]
#[cfg(feature = "fault-injection")]
enum TargetAction {
    NoSpace,
    Panic,
}

#[derive(Clone, Copy, Debug)]
#[cfg(feature = "fault-injection")]
pub(super) struct TargetFault {
    target: usize,
    moment: ProofMoment,
    action: TargetAction,
}

// Fault ownership is local to the invoking test thread; workers receive only
// their attempt/index's fault. No counting instrumentation in business code.
#[cfg(feature = "fault-injection")]
thread_local! {
    static SOURCE_PANIC: Cell<Option<ProofMoment>> = const { Cell::new(None) };
    static TARGET_FAULT: Cell<Option<TargetFault>> = const { Cell::new(None) };
    static WORKER_FAULT: Cell<Option<TargetFault>> = const { Cell::new(None) };
}

#[cfg(feature = "fault-injection")]
pub(super) fn source_checkpoint(moment: ProofMoment) {
    if SOURCE_PANIC.with(|fault| {
        if fault.get() == Some(moment) {
            fault.take();
            true
        } else {
            false
        }
    }) {
        panic!("source proof panic");
    }
}

#[cfg(feature = "fault-injection")]
pub(super) fn take_target_fault() -> Option<TargetFault> {
    TARGET_FAULT.with(Cell::take)
}

#[cfg(feature = "fault-injection")]
pub(super) fn arm_worker_fault(index: usize, fault: Option<TargetFault>) {
    WORKER_FAULT.with(|worker| worker.set(fault.filter(|fault| fault.target == index)));
}

#[cfg(feature = "fault-injection")]
pub(super) fn target_checkpoint(moment: ProofMoment) -> std::result::Result<(), CopyFailure> {
    let fault = WORKER_FAULT.with(|fault| {
        if fault.get().is_some_and(|fault| fault.moment == moment) {
            fault.take()
        } else {
            None
        }
    });
    match fault.map(|fault| fault.action) {
        None => Ok(()),
        Some(TargetAction::NoSpace) => Err(CopyFailure::Destination(Error::new(
            ErrorCode::NoSpace,
            "target write exhausted space",
        ))),
        Some(TargetAction::Panic) => panic!("target proof panic"),
    }
}

fn version_for(bytes: &[u8]) -> FileVersion {
    let manifest = ChecksumManifest {
        chunk_bytes: CHUNK,
        logical_size: bytes.len() as u64,
        chunks: bytes
            .chunks(CHUNK as usize)
            .map(|chunk| *blake3::hash(chunk).as_bytes())
            .collect(),
    };
    FileVersion {
        history_tree_id: new_id("history"),
        version_id: new_id("ver"),
        file_id: new_id("file"),
        creation_primary_slot_id: "P01".into(),
        logical_size: bytes.len() as u64,
        version_sequence: 1,
        content_hash: ContentHash::from_digest(blake3::hash(bytes)),
        checksum_chunk_bytes: CHUNK,
        checksum_manifest_hash: blake3::hash(&manifest.encode().unwrap())
            .to_hex()
            .to_string(),
        creation_name: "body.bin".into(),
        storage_name: "v000001_body.bin".into(),
        created_at: "2026-10-28T00:00:00Z".into(),
        source_version_ids: Vec::new(),
        created_as_full_checkpoint: true,
    }
}

/// Five full chunks and a short tail.
fn body() -> Vec<u8> {
    (0..5 * CHUNK as usize + 12_345)
        .map(|index| (index % 251) as u8)
        .collect()
}

struct Roots {
    temp: tempfile::TempDir,
    roots: Vec<DiskRoot>,
}

impl Roots {
    fn new(count: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let roots = (0..count)
            .map(|index| {
                let path = temp.path().join(format!("disk-{index}"));
                crate::disk::private_dir(&path).unwrap();
                DiskRoot::open(&path).unwrap()
            })
            .collect();
        Self { temp, roots }
    }

    fn source(&self, bytes: &[u8]) -> File {
        let path = self.temp.path().join("source.bin");
        fs::write(&path, bytes).unwrap();
        File::open(path).unwrap()
    }

    fn targets(&self) -> Vec<FanOutTarget<'_>> {
        self.roots
            .iter()
            .map(|root| FanOutTarget {
                root,
                destination: root.path().join("staging/op/content/body.bin"),
            })
            .collect()
    }
}

/// Every entry left in a target's destination directory.
fn entries(directory: &Path) -> Vec<String> {
    match fs::read_dir(directory) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("{error}"),
    }
}

#[test]
fn every_target_gets_an_independent_verified_synced_copy_from_one_read() {
    let roots = Roots::new(3);
    let bytes = body();
    let version = version_for(&bytes);
    let source = roots.source(&bytes);
    let targets = roots.targets();
    let threads = Mutex::new(Vec::new());
    let installed =
        verified_fan_out_copy_at(&source, &targets, &version, u64::MAX, |index, copy| {
            threads.lock().unwrap().push(std::thread::current().id());
            assert_eq!(
                ChecksumManifest::decode(copy.checksum_bytes())
                    .unwrap()
                    .chunks
                    .len(),
                6
            );
            let (copied, file) = copy.into_parts();
            assert_eq!(copied.version_id, version.version_id);
            Ok((index, file.metadata().unwrap().ino()))
        })
        .unwrap();
    assert_eq!(
        installed
            .iter()
            .map(|(index, _)| *index)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    // Each install ran on its own target thread, never on the reader.
    let threads = threads.into_inner().unwrap();
    assert!(!threads.contains(&std::thread::current().id()));
    let mut identities = Vec::new();
    for (target, (_, inode)) in targets.iter().zip(&installed) {
        let metadata = fs::metadata(&target.destination).unwrap();
        assert_eq!(fs::read(&target.destination).unwrap(), bytes);
        assert_eq!(metadata.ino(), *inode);
        assert_eq!(metadata.nlink(), 1, "no hard link represents a copy");
        assert_eq!(metadata.mode() & 0o777, 0o400);
        identities.push((metadata.dev(), metadata.ino()));
        assert_eq!(
            entries(target.destination.parent().unwrap()),
            ["body.bin"],
            "no temporary file remains"
        );
    }
    identities.sort();
    identities.dedup();
    assert_eq!(identities.len(), 3, "three distinct physical files");
    // Changing one copy leaves the others intact: nothing is shared.
    let first = &targets[0].destination;
    fs::set_permissions(first, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(first, b"changed").unwrap();
    for target in &targets[1..] {
        assert_eq!(fs::read(&target.destination).unwrap(), bytes);
    }
}

#[test]
fn source_proof_and_encoded_allocation_are_shared_by_every_independent_copy() {
    for size in [0, 1, CHUNK as usize, CHUNK as usize + 1, body().len()] {
        for count in [2, 3, MAX_FAN_OUT_TARGETS] {
            let roots = Roots::new(count);
            let bytes: Vec<_> = (0..size).map(|index| (index % 251) as u8).collect();
            let version = version_for(&bytes);
            let source = roots.source(&bytes);
            let targets = roots.targets();
            let installed =
                verified_fan_out_copy_at(&source, &targets, &version, u64::MAX, |index, copy| {
                    let summary = Arc::clone(&copy.summary);
                    // Materialization writes the very same encoded allocation,
                    // not a newly encoded manifest on each destination.
                    targets[index].root.write_atomic(
                        &targets[index].destination.with_file_name("checksums.bin"),
                        copy.checksum_bytes(),
                        false,
                    )?;
                    let (_, file) = copy.into_parts();
                    Ok((summary, file.metadata()?.len()))
                })
                .unwrap();
            for (index, (summary, written)) in installed.iter().enumerate() {
                assert!(Arc::ptr_eq(summary, &installed[0].0));
                assert_eq!(
                    summary.checksum_bytes.as_ptr(),
                    installed[0].0.checksum_bytes.as_ptr()
                );
                assert_eq!(
                    blake3::hash(&summary.checksum_bytes).to_hex().as_str(),
                    version.checksum_manifest_hash
                );
                assert_eq!(*written, size as u64);
                assert_eq!(fs::read(&targets[index].destination).unwrap(), bytes);
                assert_eq!(
                    fs::read(targets[index].destination.with_file_name("checksums.bin")).unwrap(),
                    summary.checksum_bytes
                );
            }
        }
    }
}

#[test]
fn the_source_stream_yields_one_body_while_targets_store_k_independent_bodies() {
    let roots = Roots::new(3);
    let bytes = body();
    let source = roots.source(&bytes);
    let version = version_for(&bytes);
    let targets = roots.targets();
    let mut reader = SourceReader::new(&source, &version, u64::MAX);
    let summary = OnceLock::new();
    let mut source_bytes = 0;
    let mut source_chunks = 0;
    let (failure, copies) = fan_out(
        targets.len(),
        || {
            // Observe the existing source-stream boundary, not synthetic
            // hash events or claimed kernel syscall byte counts.
            let chunk = reader.next(&summary)?;
            if let Some(bytes) = &chunk {
                source_bytes += bytes.len();
                source_chunks += 1;
            }
            Ok(chunk)
        },
        |index, stream| copy_target(&targets[index], &summary, stream),
    );
    assert!(failure.is_none());
    assert_eq!(source_bytes, bytes.len());
    assert_eq!(source_chunks, bytes.len().div_ceil(CHUNK as usize));
    assert_eq!(reader.size, bytes.len() as u64);
    assert!(
        reader.chunks.is_empty(),
        "digest storage moves into encoding and is released at EOF"
    );
    let target_bytes: u64 = copies
        .into_iter()
        .map(|copy| {
            let (_, file) = copy.unwrap().unwrap().into_parts();
            file.metadata().unwrap().len()
        })
        .sum();
    assert_eq!(target_bytes, bytes.len() as u64 * targets.len() as u64);
    for target in &targets {
        assert_eq!(fs::read(&target.destination).unwrap(), bytes);
    }
}

#[test]
fn serial_verified_exports_reuse_the_encoding_from_source_validation() {
    let roots = Roots::new(1);
    let bytes = body();
    let version = version_for(&bytes);
    for expected in [None, Some(&version)] {
        let source = roots.source(&bytes);
        let name = if expected.is_some() {
            "verified-export"
        } else {
            "unbound-copy"
        };
        let destination = roots.roots[0].path().join(name);
        let copied =
            crate::disk::full_copy_to_path(source, &destination, CHUNK, u64::MAX, expected)
                .unwrap();
        assert_eq!(copied.size, version.logical_size);
        assert_eq!(copied.hash, version.content_hash);
        if expected.is_some() {
            let validation_proof = Arc::clone(copied.summary.as_ref().unwrap());
            let verified = copied.into_verified();
            assert!(Arc::ptr_eq(&validation_proof, &verified.summary));
            assert_eq!(
                validation_proof.checksum_bytes.as_ptr(),
                verified.checksum_bytes().as_ptr()
            );
            assert_eq!(
                blake3::hash(verified.checksum_bytes()).to_hex().as_str(),
                version.checksum_manifest_hash
            );
            // Export-only callers can drop this without another encoding;
            // materializing callers borrow these same validated bytes.
            drop(verified);
        } else {
            assert!(copied.summary.is_none(), "unbound copies need no encoding");
            assert_eq!(
                copied.checksums.chunks.len(),
                bytes.len().div_ceil(CHUNK as usize)
            );
        }
        assert_eq!(fs::read(destination).unwrap(), bytes);
    }
}

#[test]
fn source_proof_does_not_replace_actual_target_file_readback() {
    let roots = Roots::new(3);
    let bytes = body();
    let version = version_for(&bytes);
    let source = roots.source(&bytes);
    let targets = roots.targets();
    let summaries = verified_fan_out_copy_at(&source, &targets, &version, u64::MAX, |_, copy| {
        Ok(Arc::clone(&copy.summary))
    })
    .unwrap();
    let first = &targets[0].destination;
    fs::set_permissions(first, Permissions::from_mode(0o600)).unwrap();
    let mut corrupted = bytes.clone();
    corrupted[CHUNK as usize] ^= 0xff;
    fs::write(first, corrupted).unwrap();
    for (index, target) in targets.iter().enumerate() {
        let manifest = ChecksumManifest::decode(&summaries[index].checksum_bytes).unwrap();
        let result = crate::disk::verify_open_payload_observed(
            File::open(&target.destination).unwrap(),
            manifest,
            &version,
            &mut |_| Ok(()),
        );
        if index == 0 {
            assert_eq!(result.unwrap_err().code(), ErrorCode::ChecksumMismatch);
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn a_source_that_differs_from_the_version_publishes_nothing() {
    let roots = Roots::new(3);
    let bytes = body();
    let version = version_for(&bytes);
    let mut changed = bytes.clone();
    changed[3 * CHUNK as usize] ^= 0xff;
    let source = roots.source(&changed);
    let targets = roots.targets();
    let failure = verified_fan_out_copy_at(
        &source,
        &targets,
        &version,
        u64::MAX,
        |_, _| -> Result<()> { panic!("nothing is installed") },
    )
    .unwrap_err();
    assert!(matches!(failure, CopyFailure::Source(_)));
    assert_eq!(failure.into_error().code(), ErrorCode::SourceChanged);
    for target in &targets {
        assert!(entries(target.destination.parent().unwrap()).is_empty());
    }
}

#[test]
fn an_oversized_source_fails_as_too_large_before_any_name_is_published() {
    let roots = Roots::new(3);
    let bytes = body();
    let version = version_for(&bytes);
    let source = roots.source(&bytes);
    let targets = roots.targets();
    let failure = verified_fan_out_copy_at(
        &source,
        &targets,
        &version,
        2 * CHUNK as u64,
        |_, _| -> Result<()> { panic!("nothing is installed") },
    )
    .unwrap_err();
    assert!(matches!(failure, CopyFailure::Source(_)));
    assert_eq!(failure.into_error().code(), ErrorCode::FileTooLarge);
    for target in &targets {
        assert!(entries(target.destination.parent().unwrap()).is_empty());
    }
}

#[test]
fn an_unwritable_target_is_a_destination_failure_and_removes_every_temporary() {
    let roots = Roots::new(3);
    let bytes = body();
    let version = version_for(&bytes);
    let source = roots.source(&bytes);
    let targets = roots.targets();
    // The second disk's staging path is occupied by a regular file.
    fs::write(roots.roots[1].path().join("staging"), b"not a directory").unwrap();
    let failure = verified_fan_out_copy_at(
        &source,
        &targets,
        &version,
        u64::MAX,
        |_, _| -> Result<()> { panic!("nothing is installed") },
    )
    .unwrap_err();
    assert!(matches!(failure, CopyFailure::Destination(_)));
    for (index, target) in targets.iter().enumerate() {
        if index != 1 {
            assert!(
                entries(target.destination.parent().unwrap()).is_empty(),
                "target {index} removed its temporary and published no name"
            );
        }
    }
}

/// A chunk that counts how many chunks are alive at once.
struct Counted<'a> {
    index: usize,
    live: &'a AtomicUsize,
}
impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

fn wait_until(done: impl Fn() -> bool) {
    let deadline = Instant::now() + WATCHDOG;
    while !done() {
        assert!(Instant::now() < deadline, "watchdog expired");
        std::thread::yield_now();
    }
}

#[test]
fn a_slow_target_bounds_the_reader_and_the_live_chunks() {
    const CHUNKS: usize = 24;
    let live = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let read = AtomicUsize::new(0);
    let released = AtomicBool::new(false);
    let delivered: Vec<_> = (0..3).map(|_| Mutex::new(Vec::new())).collect();
    let (reader, results) = std::thread::scope(|scope| {
        let releaser = scope.spawn(|| {
            // The slow target holds chunk 0; the reader can fill its two
            // queue slots and then holds exactly one more chunk.
            wait_until(|| read.load(Ordering::SeqCst) == FAN_OUT_RING + 2);
            released.store(true, Ordering::SeqCst);
        });
        let outcome = fan_out(
            3,
            || {
                let index = read.load(Ordering::SeqCst);
                if index == CHUNKS {
                    return Ok(None);
                }
                // Checked at every read, before the new chunk exists.
                let alive = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(alive, Ordering::SeqCst);
                assert!(alive <= FAN_OUT_RING + 2, "{alive} chunks alive");
                if !released.load(Ordering::SeqCst) {
                    assert!(
                        index < FAN_OUT_RING + 2,
                        "reader ran ahead of a held target"
                    );
                }
                read.store(index + 1, Ordering::SeqCst);
                Ok(Some(Counted { index, live: &live }))
            },
            |target, mut stream| {
                while let Some(chunk) = stream.next() {
                    if target == 0 && chunk.index == 0 {
                        wait_until(|| released.load(Ordering::SeqCst));
                    }
                    delivered[target].lock().unwrap().push(chunk.index);
                    drop(chunk);
                }
                let complete = stream.source_complete();
                (complete, stream.agree(complete))
            },
        );
        releaser.join().unwrap();
        outcome
    });
    assert!(reader.is_none());
    assert_eq!(results, vec![(true, true); 3]);
    assert_eq!(peak.load(Ordering::SeqCst), FAN_OUT_RING + 2);
    assert_eq!(live.load(Ordering::SeqCst), 0);
    // One pass: every chunk was read once and delivered in order everywhere.
    assert_eq!(read.load(Ordering::SeqCst), CHUNKS);
    for target in delivered {
        assert_eq!(
            target.into_inner().unwrap(),
            (0..CHUNKS).collect::<Vec<_>>()
        );
    }
}

#[test]
fn a_failing_target_stops_the_reader_and_no_target_publishes() {
    let read = AtomicUsize::new(0);
    let (reader, results) = fan_out(
        3,
        || {
            let index = read.fetch_add(1, Ordering::SeqCst);
            Ok((index < 1_000).then_some(index))
        },
        |target, mut stream| {
            let mut seen = 0;
            while let Some(_chunk) = stream.next() {
                seen += 1;
                if target == 1 && seen == 2 {
                    // A write failure: this target stops consuming.
                    return Err("target 1 failed");
                }
            }
            let complete = stream.source_complete();
            Ok(stream.agree(complete))
        },
    );
    assert!(reader.is_none(), "a stopped reader is not a source failure");
    assert_eq!(results, vec![Ok(false), Err("target 1 failed"), Ok(false)]);
    // The reader stopped instead of reading the whole source.
    assert!(read.load(Ordering::SeqCst) < 1_000);
}

#[test]
fn a_source_failure_ends_every_stream_unpublished() {
    let read = AtomicUsize::new(0);
    let (reader, results) = fan_out(
        2,
        || {
            if read.fetch_add(1, Ordering::SeqCst) == 3 {
                return Err(CopyFailure::Source(Error::new(
                    ErrorCode::IoError,
                    "source read failed",
                )));
            }
            Ok(Some(()))
        },
        |_, mut stream| {
            let mut seen = 0;
            while stream.next().is_some() {
                seen += 1;
            }
            let complete = stream.source_complete();
            (seen, complete, stream.agree(true))
        },
    );
    assert!(matches!(reader, Some(CopyFailure::Source(_))));
    assert_eq!(results, vec![(3, false, false); 2]);
}

#[test]
fn one_unsealed_target_withholds_every_publication() {
    let (reader, results) = fan_out(
        3,
        {
            let mut left = 4;
            move || {
                left -= 1;
                Ok((left > 0).then_some(()))
            }
        },
        |target, mut stream| {
            while stream.next().is_some() {}
            // Target 2 failed its verification after the complete stream.
            stream.agree(target != 2)
        },
    );
    assert!(reader.is_none());
    assert_eq!(results, vec![false; 3]);
}

#[test]
fn a_target_panic_is_resumed_after_every_target_finished() {
    let finished = AtomicUsize::new(0);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fan_out(
            3,
            {
                let mut left = 8;
                move || {
                    left -= 1;
                    Ok((left > 0).then_some(()))
                }
            },
            |target, mut stream| {
                if target == 1 {
                    panic!("target panicked");
                }
                while stream.next().is_some() {}
                let complete = stream.source_complete();
                let published = stream.agree(complete);
                finished.fetch_add(1, Ordering::SeqCst);
                published
            },
        )
    }));
    let payload = caught.err().unwrap();
    assert_eq!(
        payload.downcast_ref::<&str>().copied(),
        Some("target panicked")
    );
    assert_eq!(finished.load(Ordering::SeqCst), 2);
}

#[test]
fn a_source_panic_closes_streams_and_joins_every_target_before_unwinding() {
    let finished = AtomicUsize::new(0);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fan_out::<(), _>(
            3,
            || panic!("source panicked"),
            |_, mut stream| {
                while stream.next().is_some() {}
                assert!(!stream.source_complete());
                assert!(!stream.agree(true));
                finished.fetch_add(1, Ordering::SeqCst);
            },
        )
    }));
    let payload = caught.unwrap_err();
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"source panicked"));
    assert_eq!(finished.load(Ordering::SeqCst), 3);
}

#[test]
fn unstarted_participants_settle_the_agreement_after_partial_launch() {
    // Exercise the ownership path used after Builder::spawn_scoped fails at
    // the first, middle or final target, without exhausting the host's threads.
    for started in 0..3 {
        let agreement = Agreement {
            targets: 3,
            state: Mutex::new(AgreementState {
                reader: None,
                arrived: 0,
                sealed: 0,
            }),
            changed: Condvar::new(),
        };
        let (senders, receivers): (Vec<_>, Vec<_>) = (0..3)
            .map(|_| sync_channel::<Arc<()>>(FAN_OUT_RING))
            .unzip();
        let mut streams: Vec<_> = receivers
            .into_iter()
            .map(|chunks| ChunkStream {
                chunks: Some(chunks),
                agreement: &agreement,
                reported: false,
            })
            .collect();
        let absent = streams.split_off(started);
        std::thread::scope(|scope| {
            let handles: Vec<_> = streams
                .into_iter()
                .map(|mut stream| {
                    scope.spawn(move || {
                        while stream.next().is_some() {}
                        assert!(!stream.agree(true));
                    })
                })
                .collect();
            drop(absent);
            Reader {
                senders,
                agreement: &agreement,
            }
            .finish(ReaderEnd::Failed);
            for handle in handles {
                handle.join().unwrap();
            }
        });
        let state = agreement.state();
        assert_eq!(state.arrived, 3);
        assert_eq!(state.sealed, started);
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn first_middle_and_final_spawn_errors_settle_actual_copy_workers_and_keep_the_io_source() {
    for target in 0..3 {
        let finished = AtomicUsize::new(0);
        SPAWN_FAULT.with(|fault| fault.set(Some(SpawnFault { target })));
        let (failure, results) = fan_out::<(), _>(
            3,
            || panic!("source read before every worker was created"),
            |_, mut stream| {
                while stream.next().is_some() {}
                assert!(!stream.source_complete());
                assert!(!stream.agree(false));
                finished.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(matches!(failure, Some(CopyFailure::Destination(_))));
        assert_eq!(finished.load(Ordering::SeqCst), target);
        assert_eq!(
            results.len(),
            target,
            "every started task returned before the error"
        );
        let roots = Roots::new(3);
        let bytes = body();
        let version = version_for(&bytes);
        let source = roots.source(&bytes);
        let targets = roots.targets();
        SPAWN_FAULT.with(|fault| fault.set(Some(SpawnFault { target })));
        let failure = verified_fan_out_copy_at(
            &source,
            &targets,
            &version,
            u64::MAX,
            |_, _| -> Result<()> { panic!("failed worker creation must not install") },
        )
        .unwrap_err();
        match failure {
            CopyFailure::Destination(Error::Contract(storage_contract::Error::Io {
                source,
                ..
            })) => {
                assert_eq!(source.raw_os_error(), Some(11));
            }
            error => panic!("spawn error source was replaced: {error:?}"),
        }
        for target in &targets {
            assert!(entries(target.destination.parent().unwrap()).is_empty());
        }
        // The fault is consumed once, every started worker has joined, and
        // the same destinations can be retried without collisions/late writes.
        let installed =
            verified_fan_out_copy_at(&source, &targets, &version, u64::MAX, |index, _| Ok(index))
                .unwrap();
        assert_eq!(installed, [0, 1, 2]);
        for target in &targets {
            assert_eq!(fs::read(&target.destination).unwrap(), bytes);
        }
    }
}

#[test]
fn dropping_a_target_before_or_after_source_proof_withholds_every_other_copy() {
    for moment in [ProofMoment::Before, ProofMoment::After] {
        for dropped in 0..3 {
            let roots = Roots::new(3);
            let bytes = body();
            let source = roots.source(&bytes);
            let version = version_for(&bytes);
            let targets = roots.targets();
            let mut reader = SourceReader::new(&source, &version, u64::MAX);
            let summary = OnceLock::new();
            let completed = AtomicUsize::new(0);
            let (failure, results) = fan_out(
                3,
                || reader.next(&summary),
                |index, mut stream| {
                    let result = if index == dropped {
                        if moment == ProofMoment::After {
                            while stream.next().is_some() {}
                            assert!(stream.source_complete());
                            assert!(summary.get().is_some());
                        }
                        drop(stream);
                        Ok(None)
                    } else {
                        copy_target(&targets[index], &summary, stream)
                    };
                    completed.fetch_add(1, Ordering::SeqCst);
                    result
                },
            );
            assert!(failure.is_none());
            assert!(results.into_iter().all(|result| result.unwrap().is_none()));
            assert_eq!(completed.load(Ordering::SeqCst), 3);
            assert_eq!(summary.get().is_some(), moment == ProofMoment::After);
            for target in &targets {
                assert!(entries(target.destination.parent().unwrap()).is_empty());
            }
            verified_fan_out_copy_at(&source, &targets, &version, u64::MAX, |_, _| Ok(())).unwrap();
            for target in &targets {
                assert_eq!(entries(target.destination.parent().unwrap()), ["body.bin"]);
                assert_eq!(fs::read(&target.destination).unwrap(), bytes);
            }
        }
    }
}

#[test]
fn conservative_admission_covers_shared_proof_and_independent_collision_reads() {
    // Metadata-only envelope decision; no large-file I/O is claimed here.
    for targets in 2..=MAX_FAN_OUT_TARGETS {
        for chunk_bytes in SUPPORTED_CHECKSUM_CHUNK_BYTES {
            let mut version = version_for(b"");
            version.checksum_chunk_bytes = chunk_bytes;
            let chunks = (MAX_FAN_OUT_CHECKSUM_BYTES / (4 * targets as u64)
                - CHECKSUM_HEADER_BYTES as u64
                - 1)
                / 32;
            version.logical_size = chunks * chunk_bytes as u64;
            assert!(fan_out_admitted(&version, targets));
            let digests = chunks * 32;
            let encoded = digests + CHECKSUM_HEADER_BYTES as u64;
            let producer_peak = digests + encoded;
            let install_peak = encoded + 2 * targets as u64 * (encoded + 1);
            assert!(producer_peak.max(install_peak) <= MAX_FAN_OUT_CHECKSUM_BYTES);
            version.logical_size += 1;
            assert!(!fan_out_admitted(&version, targets));
        }
    }
}

#[test]
fn fan_out_resource_rejection_precedes_source_reads_and_target_creation() {
    let (failure, results) = fan_out::<(), ()>(
        MAX_FAN_OUT_TARGETS + 1,
        || panic!("unadmitted source must not be read"),
        |_, _| panic!("unadmitted target must not start"),
    );
    assert_eq!(
        failure.unwrap().into_error().code(),
        ErrorCode::ResourceLimit
    );
    assert!(results.is_empty());

    let roots = Roots::new(3);
    let source = roots.source(b"small source");
    let targets = roots.targets();
    let mut version = version_for(b"small source");
    version.logical_size = 1_u64 << 40;
    assert!(!fan_out_admitted(&version, targets.len()));
    let failure = verified_fan_out_copy_at(
        &source,
        &targets,
        &version,
        u64::MAX,
        |_, _| -> Result<()> { panic!("unadmitted copy must not install") },
    )
    .unwrap_err();
    assert_eq!(failure.into_error().code(), ErrorCode::ResourceLimit);
    for root in &roots.roots {
        assert!(entries(root.path()).is_empty());
    }
}

#[test]
fn corrupt_checksum_files_cannot_expand_parallel_verification_budgets() {
    let roots = Roots::new(3);
    let version = version_for(b"small");
    for root in &roots.roots {
        // A sparse corrupt file larger than this tiny version's canonical
        // manifest, but below the old format-wide read allowance.
        File::create(root.path().join("checksums.bin"))
            .unwrap()
            .set_len(8 * 1024 * 1024)
            .unwrap();
    }
    std::thread::scope(|scope| {
        for root in &roots.roots {
            let version = &version;
            scope.spawn(move || {
                let mut read = 0;
                let failure = crate::disk::read_checksum_manifest_at(
                    root,
                    root.path(),
                    version,
                    &mut |bytes| {
                        read += bytes;
                        Ok(())
                    },
                )
                .unwrap_err();
                assert_eq!(failure.code(), ErrorCode::ResourceLimit);
                assert_eq!(
                    read, 0,
                    "oversized checksum bytes must not be allocated/read"
                );
                let failure = crate::disk::open_payload_at(root, root.path(), version).unwrap_err();
                assert_eq!(failure.code(), ErrorCode::ResourceLimit);
            });
        }
    });
    let root = &roots.roots[0];
    let path = root.path().join("checksums.bin");
    let canonical = ChecksumManifest {
        chunk_bytes: CHUNK,
        logical_size: 5,
        chunks: vec![*blake3::hash(b"small").as_bytes()],
    }
    .encode()
    .unwrap();
    fs::write(&path, &canonical).unwrap();
    let mut read = 0;
    let mut grew = false;
    let failure =
        crate::disk::read_checksum_manifest_at(root, root.path(), &version, &mut |bytes| {
            read += bytes;
            if bytes > 0 && !grew {
                File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_len(8 * 1024 * 1024)
                    .unwrap();
                grew = true;
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(failure.code(), ErrorCode::ResourceLimit);
    assert_eq!(
        read,
        canonical.len() + 1,
        "growth reads only the bounded overrun byte"
    );
    let mut maximum = version.clone();
    maximum.logical_size = 1_u64 << 40;
    assert_eq!(
        crate::disk::checksum_manifest_size(&maximum).unwrap(),
        crate::disk::MAX_CHECKSUM_MANIFEST
    );
    maximum.logical_size += 1;
    assert_eq!(
        crate::disk::checksum_manifest_size(&maximum)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimit
    );
    maximum.checksum_chunk_bytes = 0;
    assert_eq!(
        crate::disk::checksum_manifest_size(&maximum)
            .unwrap_err()
            .code(),
        ErrorCode::ChecksumFormatInvalid
    );
}

#[test]
fn source_truncation_and_growth_never_publish_copy_names() {
    let bytes = body();
    for changed in [
        &bytes[..bytes.len() - 1],
        &[bytes.as_slice(), b"extra"].concat(),
    ] {
        let roots = Roots::new(3);
        let source = roots.source(changed);
        let version = version_for(&bytes);
        let targets = roots.targets();
        let failure = verified_fan_out_copy_at(
            &source,
            &targets,
            &version,
            u64::MAX,
            |_, _| -> Result<()> { panic!("changed source must not install") },
        )
        .unwrap_err();
        assert!(matches!(failure, CopyFailure::Source(_)));
        assert_eq!(failure.into_error().code(), ErrorCode::SourceChanged);
        for target in &targets {
            assert!(entries(target.destination.parent().unwrap()).is_empty());
        }
    }
}

#[test]
fn a_wrong_checksum_commitment_never_becomes_shared_source_evidence() {
    let roots = Roots::new(3);
    let bytes = body();
    let source = roots.source(&bytes);
    let mut version = version_for(&bytes);
    version.checksum_manifest_hash = "0".repeat(64);
    let targets = roots.targets();
    let failure = verified_fan_out_copy_at(
        &source,
        &targets,
        &version,
        u64::MAX,
        |_, _| -> Result<()> { panic!("invalid source proof must not install") },
    )
    .unwrap_err();
    assert!(matches!(failure, CopyFailure::Source(_)));
    assert_eq!(failure.into_error().code(), ErrorCode::SourceChanged);
    for target in &targets {
        assert!(entries(target.destination.parent().unwrap()).is_empty());
    }
}

#[test]
fn a_real_source_read_error_preserves_its_io_cause_and_cleans_every_target() {
    let roots = Roots::new(3);
    let bytes = body();
    let version = version_for(&bytes);
    // A write-only descriptor produces EBADF at the actual read_at boundary.
    let mut source = File::create(roots.temp.path().join("write-only-source")).unwrap();
    source.write_all(&bytes).unwrap();
    let targets = roots.targets();
    let failure = verified_fan_out_copy_at(
        &source,
        &targets,
        &version,
        u64::MAX,
        |_, _| -> Result<()> { panic!("unreadable source must not install") },
    )
    .unwrap_err();
    match failure {
        CopyFailure::Source(Error::Contract(storage_contract::Error::Io { source, .. })) => {
            assert_eq!(source.raw_os_error(), Some(9));
        }
        error => panic!("source read cause was replaced: {error:?}"),
    }
    for target in &targets {
        assert!(entries(target.destination.parent().unwrap()).is_empty());
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn target_failure_or_panic_before_and_after_source_proof_cleans_and_joins() {
    for moment in [ProofMoment::Before, ProofMoment::After] {
        for action in [TargetAction::NoSpace, TargetAction::Panic] {
            for target in 0..3 {
                let roots = Roots::new(3);
                // Longer than the channel window: a first-chunk failure
                // necessarily happens before the producer can finish EOF.
                let bytes = body();
                let source = roots.source(&bytes);
                let version = version_for(&bytes);
                let targets = roots.targets();
                TARGET_FAULT.with(|fault| {
                    fault.set(Some(TargetFault {
                        target,
                        moment,
                        action,
                    }))
                });
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    verified_fan_out_copy_at(
                        &source,
                        &targets,
                        &version,
                        u64::MAX,
                        |_, _| -> Result<()> {
                            panic!("unsealed target must prevent installation")
                        },
                    )
                }));
                match action {
                    TargetAction::NoSpace => {
                        let failure = outcome.unwrap().unwrap_err();
                        assert!(matches!(failure, CopyFailure::Destination(_)));
                        let error = failure.into_error();
                        assert_eq!(error.code(), ErrorCode::NoSpace);
                        assert_eq!(error.message(), "target write exhausted space");
                    }
                    TargetAction::Panic => {
                        assert_eq!(
                            outcome.unwrap_err().downcast_ref::<&str>(),
                            Some(&"target proof panic")
                        );
                    }
                }
                for target in &targets {
                    assert!(entries(target.destination.parent().unwrap()).is_empty());
                }
                // Return/unwind joined every worker. Retrying the same names
                // succeeds without temporary collisions or background writes.
                let completed =
                    verified_fan_out_copy_at(&source, &targets, &version, u64::MAX, |index, _| {
                        Ok(index)
                    })
                    .unwrap();
                assert_eq!(completed, [0, 1, 2]);
                for target in &targets {
                    assert_eq!(entries(target.destination.parent().unwrap()), ["body.bin"]);
                    assert_eq!(fs::read(&target.destination).unwrap(), bytes);
                }
            }
        }
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn producer_panic_even_after_publishing_its_proof_cannot_complete_the_stream() {
    for moment in [ProofMoment::Before, ProofMoment::After] {
        let roots = Roots::new(3);
        let bytes = body();
        let source = roots.source(&bytes);
        let version = version_for(&bytes);
        let targets = roots.targets();
        SOURCE_PANIC.with(|fault| fault.set(Some(moment)));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            verified_fan_out_copy_at(
                &source,
                &targets,
                &version,
                u64::MAX,
                |_, _| -> Result<()> { panic!("failed producer must prevent installation") },
            )
        }));
        assert_eq!(
            outcome.unwrap_err().downcast_ref::<&str>(),
            Some(&"source proof panic")
        );
        assert!(
            SOURCE_PANIC.with(Cell::get).is_none(),
            "the selected proof-boundary fault was consumed"
        );
        for target in &targets {
            assert!(entries(target.destination.parent().unwrap()).is_empty());
        }
        verified_fan_out_copy_at(&source, &targets, &version, u64::MAX, |_, _| Ok(())).unwrap();
        for target in &targets {
            assert_eq!(entries(target.destination.parent().unwrap()), ["body.bin"]);
            assert_eq!(fs::read(&target.destination).unwrap(), bytes);
        }
    }
}

#[test]
fn install_panic_after_all_target_seals_joins_and_retains_recovery_files() {
    for panicked in 0..3 {
        let roots = Roots::new(3);
        let bytes = body();
        let source = roots.source(&bytes);
        let version = version_for(&bytes);
        let targets = roots.targets();
        let started = AtomicUsize::new(0);
        let finished = AtomicUsize::new(0);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            verified_fan_out_copy_at(&source, &targets, &version, u64::MAX, |index, _| {
                started.fetch_add(1, Ordering::SeqCst);
                if index == panicked {
                    panic!("installation panicked");
                }
                finished.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }));
        assert_eq!(
            outcome.unwrap_err().downcast_ref::<&str>(),
            Some(&"installation panicked")
        );
        assert_eq!(started.load(Ordering::SeqCst), 3);
        assert_eq!(finished.load(Ordering::SeqCst), 2);
        for target in &targets {
            assert_eq!(entries(target.destination.parent().unwrap()), ["body.bin"]);
            assert_eq!(fs::read(&target.destination).unwrap(), bytes);
        }
    }
}

#[test]
fn installation_errors_keep_target_order_after_all_targets_finish() {
    for failed in [vec![0], vec![1], vec![2], vec![0, 2], vec![1, 2]] {
        let roots = Roots::new(3);
        let bytes = body();
        let source = roots.source(&bytes);
        let version = version_for(&bytes);
        let targets = roots.targets();
        let finished = AtomicUsize::new(0);
        let failure =
            verified_fan_out_copy_at(&source, &targets, &version, u64::MAX, |index, _| {
                finished.fetch_add(1, Ordering::SeqCst);
                if failed.contains(&index) {
                    Err(Error::new(ErrorCode::NoSpace, format!("disk {index} full")))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert_eq!(finished.load(Ordering::SeqCst), 3);
        let failure = failure.into_error();
        assert_eq!(failure.code(), ErrorCode::NoSpace);
        assert_eq!(failure.message(), format!("disk {} full", failed[0]));
        // Installation failed after the all-sealed decision: independently
        // durable copy names remain recovery material, not a partial success.
        for target in &targets {
            assert_eq!(fs::read(&target.destination).unwrap(), bytes);
        }
    }
}
