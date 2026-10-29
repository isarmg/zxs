//! One source read fanned out to several independent payload copies.
//!
//! The calling thread is the single reader. Every chunk it reads is shared as
//! one `Arc` with K target threads through bounded channels of `FAN_OUT_RING`
//! chunks each, so the reader blocks on the slowest target (backpressure).
//! The reader verifies the source commitments once. Each target writes and
//! syncs its own temporary file: copies share no file, extent or link, only
//! immutable bytes and the source proof in memory. This is not a substitute
//! for subsequent verification that reads each target file from disk.
//! No target publishes its name until every target has sealed (written and
//! synced) a copy of the verified source; on any failure every target
//! removes its temporary file. All threads are joined before returning.
use super::{CopyFailure, DiskRoot, VerifiedCopy, VerifiedSourceSummary, fs_error};
use crate::*;
use platform_io::fs_safety::EntryName;
use std::fs::{File, Permissions};
use std::io::Write;
use std::os::unix::fs::{FileExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// Chunks queued per target. The reader runs at most `FAN_OUT_RING + 1`
/// chunks ahead of the chunk the slowest target is writing, so at most
/// `FAN_OUT_RING + 2` chunks are alive per copied version: at the 4 MiB
/// checksum chunk maximum that is 16 MiB per version, 64 MiB for the four
/// payload preparation workers.
pub(crate) const FAN_OUT_RING: usize = 2;
pub(crate) const MAX_FAN_OUT_TARGETS: usize = 8;
const MAX_FAN_OUT_CHECKSUM_BYTES: u64 = 16 * 1024 * 1024;

/// One-shot resource failure at the thread-creation boundary. Thread-local
/// ownership isolates concurrent fault-injection callers; no input parser or
/// normal-build behavior depends on this hook.
#[cfg(feature = "fault-injection")]
#[derive(Clone, Copy)]
struct SpawnFault {
    target: usize,
}

#[cfg(feature = "fault-injection")]
thread_local! {
    static SPAWN_FAULT: std::cell::Cell<Option<SpawnFault>> = const { std::cell::Cell::new(None) };
}

#[cfg(feature = "fault-injection")]
fn take_spawn_failure(target: usize) -> Option<std::io::Error> {
    SPAWN_FAULT.with(|fault| {
        if fault.get().is_some_and(|fault| fault.target == target) {
            fault.set(None);
            Some(std::io::Error::from_raw_os_error(11))
        } else {
            None
        }
    })
}

/// Admission for the optimized path, not a product file/route limit. The
/// caller keeps the serial copy path for larger routes/manifests. Count the
/// source digest vector and canonical encoding once, plus each immutable
/// writer's collision-read allocation (up to twice the encoded length + 1).
/// Keep the previous conservative 4*K*(M+1) admission: it covers both the
/// producer's D+M peak and installation's M+2*K*(M+1), without expanding the
/// parallel readback envelope or claiming a whole-engine memory limit.
pub(crate) fn fan_out_admitted(version: &FileVersion, targets: usize) -> bool {
    let chunks = version
        .logical_size
        .div_ceil(version.checksum_chunk_bytes as u64);
    targets <= MAX_FAN_OUT_TARGETS
        && (chunks * 32 + CHECKSUM_HEADER_BYTES as u64 + 1) * 4 * targets as u64
            <= MAX_FAN_OUT_CHECKSUM_BYTES
}

/// One destination of a fan-out copy: a held disk root and the payload path
/// below it. Its parent directory is created on demand.
pub(crate) struct FanOutTarget<'a> {
    pub(crate) root: &'a DiskRoot,
    pub(crate) destination: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReaderEnd {
    /// The source was read to its end.
    Complete,
    /// The source read failed.
    Failed,
    /// A target stopped consuming, so reading stopped early.
    Stopped,
}

struct AgreementState {
    reader: Option<ReaderEnd>,
    arrived: usize,
    sealed: usize,
}

/// The publish decision shared by every target of one fan-out.
struct Agreement {
    targets: usize,
    state: Mutex<AgreementState>,
    changed: Condvar,
}

impl Agreement {
    fn state(&self) -> std::sync::MutexGuard<'_, AgreementState> {
        self.state
            .lock()
            .expect("invariant: fan-out agreement poisoned")
    }
}

/// One target's view of the shared chunk sequence.
///
/// Every stream reports to the agreement exactly once: through `agree`, or,
/// if the target returns or unwinds first, as unsealed when dropped. Either
/// way its channel is closed first so a blocked reader stops.
pub(crate) struct ChunkStream<'s, C> {
    chunks: Option<Receiver<Arc<C>>>,
    agreement: &'s Agreement,
    reported: bool,
}

impl<C> ChunkStream<'_, C> {
    /// The next chunk in source order; `None` once the reader has ended.
    pub(crate) fn next(&mut self) -> Option<Arc<C>> {
        self.chunks.as_ref().and_then(|chunks| chunks.recv().ok())
    }

    /// Whether the stream ended because the whole source was read. Only
    /// meaningful after `next` returned `None`.
    pub(crate) fn source_complete(&self) -> bool {
        self.agreement.state().reader == Some(ReaderEnd::Complete)
    }

    /// Report whether this target sealed its copy, then wait for every other
    /// target. Returns true only if every target sealed a copy of the
    /// completely read source; only then may any target publish its name.
    pub(crate) fn agree(mut self, sealed: bool) -> bool {
        self.report(sealed);
        let state = self
            .agreement
            .changed
            .wait_while(self.agreement.state(), |state| {
                state.arrived < self.agreement.targets
            })
            .expect("invariant: fan-out agreement poisoned");
        state.reader == Some(ReaderEnd::Complete) && state.sealed == self.agreement.targets
    }

    fn report(&mut self, sealed: bool) {
        self.chunks = None;
        self.reported = true;
        // No user work runs under this lock; poisoning is impossible. Avoid
        // introducing a second panic when this participant unwinds.
        let mut state = self
            .agreement
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        state.arrived += 1;
        state.sealed += usize::from(sealed);
        drop(state);
        self.agreement.changed.notify_all();
    }
}

/// The reader owns stream termination even when `read` unwinds. Dropping it
/// closes every sender before the scope joins targets waiting for EOF.
struct Reader<'a, C> {
    senders: Vec<std::sync::mpsc::SyncSender<Arc<C>>>,
    agreement: &'a Agreement,
}

impl<C> Reader<'_, C> {
    fn finish(&mut self, end: ReaderEnd) {
        self.agreement.state().reader = Some(end);
        self.senders.clear();
        self.agreement.changed.notify_all();
    }
}

impl<C> Drop for Reader<'_, C> {
    fn drop(&mut self) {
        let mut state = self
            .agreement
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if state.reader.is_none() {
            state.reader = Some(ReaderEnd::Failed);
        }
        drop(state);
        self.senders.clear();
        self.agreement.changed.notify_all();
    }
}

impl<C> Drop for ChunkStream<'_, C> {
    fn drop(&mut self) {
        if !self.reported {
            self.report(false);
        }
    }
}

/// Read once through `read` and hand every chunk, in order, to each of
/// `targets` scoped threads running `target`. Returns the reader's own
/// failure, if it failed, and each target's result in target order.
///
/// A target that stops consuming makes the reader stop; a reader failure or
/// stop ends every stream with `source_complete() == false`. All threads are
/// joined before this returns; a target panic is resumed after the join.
pub(crate) fn fan_out<C: Send + Sync, T: Send>(
    targets: usize,
    mut read: impl FnMut() -> std::result::Result<Option<C>, CopyFailure>,
    target: impl Fn(usize, ChunkStream<'_, C>) -> T + Sync,
) -> (Option<CopyFailure>, Vec<T>) {
    if targets > MAX_FAN_OUT_TARGETS {
        return (
            Some(CopyFailure::Destination(Error::new(
                ErrorCode::ResourceLimit,
                "fan-out target count exceeds worker envelope",
            ))),
            Vec::new(),
        );
    }
    let agreement = Agreement {
        targets,
        state: Mutex::new(AgreementState {
            reader: None,
            arrived: 0,
            sealed: 0,
        }),
        changed: Condvar::new(),
    };
    let (senders, receivers): (Vec<_>, Vec<_>) =
        (0..targets).map(|_| sync_channel(FAN_OUT_RING)).unzip();
    // Every participant owns a guard before the first spawn. If spawn fails,
    // dropping its closure and the remaining iterator settles all absent
    // participants, so already started targets cannot wait for phantom peers.
    let streams: Vec<_> = receivers
        .into_iter()
        .map(|chunks| ChunkStream {
            chunks: Some(chunks),
            agreement: &agreement,
            reported: false,
        })
        .collect();
    std::thread::scope(|scope| {
        let mut reader = Reader {
            senders,
            agreement: &agreement,
        };
        let mut handles = Vec::with_capacity(targets);
        let mut spawn_failure = None;
        for (index, stream) in streams.into_iter().enumerate() {
            let target = &target;
            let worker = move || target(index, stream);
            #[cfg(feature = "fault-injection")]
            let spawned = match take_spawn_failure(index) {
                Some(error) => {
                    drop(worker);
                    Err(error)
                }
                None => std::thread::Builder::new().spawn_scoped(scope, worker),
            };
            #[cfg(not(feature = "fault-injection"))]
            let spawned = std::thread::Builder::new().spawn_scoped(scope, worker);
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    spawn_failure = Some(CopyFailure::Destination(error.into()));
                    break;
                }
            }
        }
        let (end, failure) = if spawn_failure.is_some() {
            (ReaderEnd::Failed, spawn_failure)
        } else {
            loop {
                match read() {
                    Err(failure) => break (ReaderEnd::Failed, Some(failure)),
                    Ok(None) => break (ReaderEnd::Complete, None),
                    Ok(Some(chunk)) => {
                        let chunk = Arc::new(chunk);
                        // Blocks on the slowest target: this is the backpressure.
                        if reader
                            .senders
                            .iter()
                            .any(|sender| sender.send(Arc::clone(&chunk)).is_err())
                        {
                            break (ReaderEnd::Stopped, None);
                        }
                    }
                }
            }
        };
        // Publish the reader's end before closing the streams that observe it.
        reader.finish(end);
        let mut results = Vec::with_capacity(targets);
        let mut panic = None;
        for handle in handles {
            match handle.join() {
                Ok(result) => results.push(result),
                Err(payload) => {
                    panic.get_or_insert(payload);
                }
            }
        }
        if let Some(payload) = panic {
            std::panic::resume_unwind(payload);
        }
        (failure, results)
    })
}

/// Read and verify `source` once, write its immutable chunks to every target,
/// then run `install` with the shared source proof for each independently
/// published copy on that target's own thread (same disk, sequential).
///
/// Errors: the first target failure by target index wins, then a source
/// failure; `CopyFailure::{Source, Destination}` classification is kept.
/// After a failure no copy name is published unless every target had already
/// agreed to publish (a later rename or install failure on one disk does not
/// undo the others, exactly as a failed later level did not undo an earlier
/// one).
pub(crate) fn verified_fan_out_copy_at<'v, R: Send>(
    source: &File,
    targets: &[FanOutTarget<'_>],
    version: &'v FileVersion,
    max_bytes: u64,
    install: impl Fn(usize, VerifiedCopy<'v>) -> Result<R> + Sync,
) -> std::result::Result<Vec<R>, CopyFailure> {
    if !fan_out_admitted(version, targets.len()) {
        return Err(CopyFailure::Destination(Error::new(
            ErrorCode::ResourceLimit,
            "fan-out checksum or worker envelope exceeded",
        )));
    }
    let mut source = SourceReader::new(source, version, max_bytes);
    let summary = OnceLock::new();
    #[cfg(all(test, feature = "fault-injection"))]
    let target_fault = tests::take_target_fault();
    let (reader, results) = fan_out(
        targets.len(),
        || source.next(&summary),
        |index, stream| {
            #[cfg(all(test, feature = "fault-injection"))]
            tests::arm_worker_fault(index, target_fault);
            match copy_target(&targets[index], &summary, stream)? {
                Some(copy) => install(index, copy)
                    .map(Some)
                    .map_err(CopyFailure::Destination),
                None => Ok(None),
            }
        },
    );
    let mut installed = Vec::with_capacity(targets.len());
    for result in results {
        installed.extend(result?);
    }
    if let Some(failure) = reader {
        return Err(failure);
    }
    assert_eq!(
        installed.len(),
        targets.len(),
        "invariant: a completely read source without target failures publishes every copy"
    );
    Ok(installed)
}

/// Single-owner verification state for one admitted source. Only successful
/// EOF constructs source evidence; stopping early leaves no proof behind.
struct SourceReader<'s, 'v> {
    source: &'s File,
    version: &'v FileVersion,
    max_bytes: u64,
    size: u64,
    full: blake3::Hasher,
    chunks: Vec<[u8; 32]>,
}

impl<'s, 'v> SourceReader<'s, 'v> {
    fn new(source: &'s File, version: &'v FileVersion, max_bytes: u64) -> Self {
        Self {
            source,
            version,
            max_bytes,
            size: 0,
            full: blake3::Hasher::new(),
            chunks: Vec::with_capacity(
                version
                    .logical_size
                    .div_ceil(version.checksum_chunk_bytes as u64) as usize,
            ),
        }
    }

    fn next(
        &mut self,
        summary: &OnceLock<Arc<VerifiedSourceSummary<'v>>>,
    ) -> std::result::Result<Option<Vec<u8>>, CopyFailure> {
        let version = self.version;
        if let Some(chunk) = read_chunk(
            self.source,
            version.checksum_chunk_bytes as usize,
            &mut self.size,
            self.max_bytes,
            version.logical_size,
        )? {
            self.full.update(&chunk);
            self.chunks.push(*blake3::hash(&chunk).as_bytes());
            #[cfg(all(test, feature = "fault-injection"))]
            tests::source_checkpoint(tests::ProofMoment::Before);
            return Ok(Some(chunk));
        }
        let hash = ContentHash::from_digest(self.full.finalize());
        let changed = || {
            CopyFailure::Source(Error::new(
                ErrorCode::SourceChanged,
                "copied content differs from the published manifest",
            ))
        };
        if self.size != version.logical_size || hash != version.content_hash {
            return Err(changed());
        }
        let checksum_bytes = ChecksumManifest {
            chunk_bytes: version.checksum_chunk_bytes,
            logical_size: self.size,
            chunks: std::mem::take(&mut self.chunks),
        }
        .encode()?;
        let checksum_hash = blake3::hash(&checksum_bytes);
        if checksum_hash.to_hex().as_str() != version.checksum_manifest_hash {
            return Err(changed());
        }
        summary
            .set(Arc::new(VerifiedSourceSummary {
                version,
                checksum_bytes,
            }))
            .expect("invariant: source proof published once at EOF");
        #[cfg(all(test, feature = "fault-injection"))]
        tests::source_checkpoint(tests::ProofMoment::After);
        // ReaderEnd::Complete is published only after this proof.
        Ok(None)
    }
}

fn read_chunk(
    source: &File,
    chunk_bytes: usize,
    size: &mut u64,
    max_bytes: u64,
    expected_size: u64,
) -> std::result::Result<Option<Vec<u8>>, CopyFailure> {
    let mut chunk = vec![0u8; chunk_bytes];
    let mut used = 0;
    while used < chunk.len() {
        let read = source
            .read_at(&mut chunk[used..], *size + used as u64)
            .map_err(|error| CopyFailure::Source(error.into()))?;
        if read == 0 {
            break;
        }
        used += read;
    }
    if used == 0 {
        return Ok(None);
    }
    *size = size.checked_add(used as u64).ok_or_else(|| {
        CopyFailure::Source(Error::new(
            storage_contract::ErrorCode::FileTooLarge,
            "file length overflow",
        ))
    })?;
    if *size > max_bytes {
        return Err(CopyFailure::Source(Error::new(
            storage_contract::ErrorCode::FileTooLarge,
            "file exceeds configured maximum",
        )));
    }
    if *size > expected_size {
        return Err(CopyFailure::Source(Error::new(
            ErrorCode::SourceChanged,
            "source grew beyond the published length",
        )));
    }
    chunk.truncate(used);
    Ok(Some(chunk))
}

/// One target: an exclusive temporary file in the destination directory,
/// written from ordered immutable chunks bound to the producer's source proof,
/// made read-only and synced; renamed without replacement and the directory
/// synced only once every target agreed. `None` means another participant
/// failed and this target removed its temporary file.
fn copy_target<'v>(
    target: &FanOutTarget<'_>,
    summary: &OnceLock<Arc<VerifiedSourceSummary<'v>>>,
    mut stream: ChunkStream<'_, Vec<u8>>,
) -> std::result::Result<Option<VerifiedCopy<'v>>, CopyFailure> {
    let parent = target
        .destination
        .parent()
        .expect("invariant: payload destination has a parent");
    target.root.private_dir(parent)?;
    let directory = target.root.directory(parent)?;
    let name = EntryName::new(
        target
            .destination
            .file_name()
            .expect("invariant: payload destination has a name"),
    )
    .map_err(fs_error)?;
    let temporary = format!(".zxs-full-{}", new_id("req"));
    let file = File::from(
        rustix::fs::openat(
            &directory,
            temporary.as_str(),
            rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::EXCL
                | rustix::fs::OFlags::WRONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_raw_mode(0o600),
        )
        .map_err(std::io::Error::from)?,
    );
    let _unwind_cleanup = UnwindTemporary {
        directory: &directory,
        name: &temporary,
    };
    let sealed = write_and_seal(&mut stream, &file, summary);
    let publish = stream.agree(matches!(sealed, Ok(Some(_))));
    let outcome = match sealed {
        Ok(Some(summary)) if publish => (|| {
            rustix::fs::renameat_with(
                &directory,
                temporary.as_str(),
                &directory,
                name.as_os_str(),
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(std::io::Error::from)?;
            directory.sync_all()?;
            Ok(Some(summary))
        })(),
        Ok(_) => Ok(None),
        Err(failure) => Err(failure),
    };
    match outcome {
        Ok(Some(summary)) => Ok(Some(VerifiedCopy { file, summary })),
        Ok(None) => match remove_temporary(&directory, &temporary) {
            Ok(()) => Ok(None),
            Err(cleanup) => Err(CopyFailure::Destination(
                std::io::Error::from(cleanup).into(),
            )),
        },
        Err(operation) => match remove_temporary(&directory, &temporary) {
            Ok(()) => Err(operation),
            Err(cleanup) => Err(CopyFailure::Destination(Error::Cleanup {
                operation: Box::new(operation.into_error()),
                cleanup: cleanup.into(),
            })),
        },
    }
}

/// Normal failures report cleanup errors below. Unwind cleanup is best-effort
/// and must not replace the original panic or panic again during destruction.
struct UnwindTemporary<'a> {
    directory: &'a File,
    name: &'a str,
}

impl Drop for UnwindTemporary<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let _ = remove_temporary(self.directory, self.name);
        }
    }
}

fn remove_temporary(
    directory: &File,
    temporary: &str,
) -> std::result::Result<(), rustix::io::Errno> {
    match rustix::fs::unlinkat(directory, temporary, rustix::fs::AtFlags::empty()) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
        Err(cleanup) => Err(cleanup),
    }
}

/// FIFO delivery and write_all bind this file to the producer's immutable
/// chunks. The shared proof validates input, while this target's own chmod
/// and fsync establish its durability. No target-file readback occurs here.
fn write_and_seal<'v>(
    stream: &mut ChunkStream<'_, Vec<u8>>,
    file: &File,
    summary: &OnceLock<Arc<VerifiedSourceSummary<'v>>>,
) -> std::result::Result<Option<Arc<VerifiedSourceSummary<'v>>>, CopyFailure> {
    let mut writer = file;
    let mut size = 0u64;
    while let Some(chunk) = stream.next() {
        #[cfg(all(test, feature = "fault-injection"))]
        tests::target_checkpoint(tests::ProofMoment::Before)?;
        writer.write_all(&chunk)?;
        size += chunk.len() as u64;
    }
    if !stream.source_complete() {
        return Ok(None);
    }
    let summary = summary
        .get()
        .expect("invariant: completed reader verified its source");
    assert_eq!(
        size, summary.version.logical_size,
        "invariant: every source byte was written in order"
    );
    #[cfg(all(test, feature = "fault-injection"))]
    tests::target_checkpoint(tests::ProofMoment::After)?;
    file.set_permissions(Permissions::from_mode(0o400))?;
    // Persist the complete bytes and final mode together before publishing
    // the name. The rename still needs its own parent-directory barrier.
    file.sync_all()?;
    Ok(Some(Arc::clone(summary)))
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk/fan_out_tests.rs"]
mod tests;
