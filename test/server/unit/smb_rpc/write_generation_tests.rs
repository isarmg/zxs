//! The VFS write sequence (`smb_protocol::client::WriteSequence`) driven
//! against a real core-backed session: PrepareWrite admits every mutation,
//! Dirty opens each save generation, Flush/Close still publish every byte.
use super::*;
use smb_protocol::client::{Mutation, WriteSequence};
use std::time::{Duration, Instant};
use storage_contract::OperationId;

type VfsRequest = smb_protocol::protocol::Request<OperationId>;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sent {
    Prepare,
    Dirty,
    Flush,
}

fn call(session: &mut Session, sent: &mut Vec<Sent>, request: VfsRequest) -> io::Result<Value> {
    let (kind, request) = match request {
        VfsRequest::PrepareWrite { handle, end, bytes } => {
            (Sent::Prepare, Request::PrepareWrite { handle, end, bytes })
        }
        VfsRequest::Dirty { handle } => (Sent::Dirty, Request::Dirty { handle }),
        VfsRequest::Flush { handle } => (Sent::Flush, Request::Flush { handle }),
        other => panic!("write sequencing sent {other:?}"),
    };
    sent.push(kind);
    session
        .dispatch(request)
        .map(|(value, _)| value)
        .map_err(|error| io::Error::other(error.error))
}

fn error_code(error: &io::Error) -> storage_contract::ErrorCode {
    error
        .get_ref()
        .unwrap()
        .downcast_ref::<ApiError>()
        .unwrap()
        .code
}

struct Vfs {
    handle: u64,
    file: File,
    sequence: WriteSequence,
    sent: Vec<Sent>,
}

impl Vfs {
    fn open(session: &mut Session, path: &str, create: bool, write_through: bool) -> Self {
        let (result, descriptor) = session
            .open(path.into(), true, create, false, create, write_through)
            .unwrap();
        let opened: OpenResult = json_codec::from_value(result).unwrap();
        Self {
            handle: opened.handle,
            file: File::from(descriptor.unwrap()),
            sequence: WriteSequence::new(write_through),
            sent: Vec::new(),
        }
    }

    fn write(&mut self, session: &mut Session, offset: u64, bytes: &[u8]) -> io::Result<()> {
        let Self {
            handle,
            file,
            sequence,
            sent,
        } = self;
        sequence.mutate(
            *handle,
            offset + bytes.len() as u64,
            bytes.len() as u64,
            |request| call(session, sent, request),
            || {
                file.write_all_at(bytes, offset)?;
                Ok(((), Mutation::Changed))
            },
        )
    }

    fn flush(&mut self, session: &mut Session) -> io::Result<Value> {
        let Self {
            handle,
            sequence,
            sent,
            ..
        } = self;
        sequence.flush(*handle, |request| call(session, sent, request))
    }

    fn dirty_count(&self) -> usize {
        self.sent
            .iter()
            .filter(|sent| **sent == Sent::Dirty)
            .count()
    }
}

fn published(session: &Session, path: &str) -> Vec<u8> {
    let entry = session
        .entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == path)
        .unwrap();
    let content = session
        .shared
        .backend
        .content(&session.identity, &entry.version_id)
        .unwrap();
    let mut bytes = vec![0; content.size as usize];
    content.file.read_exact_at(&mut bytes, 0).unwrap();
    bytes
}

fn write_handle(session: &Session, handle: u64) -> &Writer {
    let Handle::Write(writer) = &session.handles[&handle] else {
        panic!("expected a writer")
    };
    writer
}

fn workspace_id(session: &Session, handle: u64) -> String {
    write_handle(session, handle).workspace.workspace_id.clone()
}

#[test]
fn one_dirty_per_generation_still_publishes_every_change_at_flush_and_close() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let mut vfs = Vfs::open(&mut session, "report.txt", false, false);
    for (offset, byte) in [(0, b"n"), (1, b"e"), (2, b"w")] {
        vfs.write(&mut session, offset, byte).unwrap();
    }
    assert_eq!(
        vfs.sent,
        [Sent::Prepare, Sent::Dirty, Sent::Prepare, Sent::Prepare]
    );
    let saved = vfs.flush(&mut session).unwrap();
    assert_eq!(saved["commit_state"], "PUBLISHED");
    assert_eq!(published(&session, "report.txt"), b"new");
    // The published generation is closed: the next change opens a new one.
    vfs.write(&mut session, 3, b"!").unwrap();
    vfs.write(&mut session, 4, b"?").unwrap();
    assert_eq!(vfs.dirty_count(), 2);
    session.close(vfs.handle, false).unwrap();
    assert_eq!(published(&session, "report.txt"), b"new!?");
}

#[test]
fn prepare_write_rejects_revocation_before_the_descriptor_changes() {
    let (_temp, shared, backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let mut vfs = Vfs::open(&mut session, "report.txt", false, false);
    backend.revoked.store(true, Ordering::SeqCst);
    let error = vfs.write(&mut session, 0, b"new").unwrap_err();
    assert_eq!(error_code(&error), storage_contract::ErrorCode::Forbidden);
    assert_eq!(vfs.sent, [Sent::Prepare]);
    let mut bytes = [0; 3];
    vfs.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(
        &bytes, b"old",
        "the rejected change never reached the descriptor"
    );
}

#[test]
fn prepare_write_rejects_revocation_after_same_generation_dirty_was_skipped() {
    let (temp, shared, _backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let mut vfs = Vfs::open(&mut session, "report.txt", false, false);
    vfs.write(&mut session, 0, b"n").unwrap();
    vfs.write(&mut session, 1, b"e").unwrap();
    assert_eq!(vfs.sent, [Sent::Prepare, Sent::Dirty, Sent::Prepare]);
    // SMB mappings have no HTTP session; revoke by replacing the sole principal.
    crate::lock(&temp.auth).replace_authority(
        AuthStore::initialize(
            &temp.path().join("replacement-auth"),
            "bob",
            "long-test-password",
        )
        .unwrap(),
    );
    let error = vfs.write(&mut session, 2, b"w").unwrap_err();
    assert_eq!(
        error_code(&error),
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(vfs.sent.last(), Some(&Sent::Prepare));
    assert_eq!(vfs.dirty_count(), 1);
    let mut bytes = [0; 3];
    vfs.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"ned");
}

#[test]
fn prepare_write_rejects_expiry_after_same_generation_dirty_was_skipped() {
    let (temp, shared, backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let mut vfs = Vfs::open(&mut session, "report.txt", false, false);
    vfs.write(&mut session, 0, b"n").unwrap();
    vfs.write(&mut session, 1, b"e").unwrap();
    assert_eq!(vfs.sent, [Sent::Prepare, Sent::Dirty, Sent::Prepare]);
    let workspace = workspace_id(&session, vfs.handle);
    backend.inner.test_with_engine_locked(|engine| {
        let mut expired = engine.workspace(&workspace).unwrap();
        // Reopen valid, expired workspace mirrors through the real recovery
        // boundary, as in engine_backend::authorization_tests; no wall-clock sleep.
        // A spare engine lets the original release its filesystem ownership.
        let spare = Engine::initialize(
            &temp.path().join("expiry-spare-state"),
            Config::development([temp.path().join("expiry-spare-disk")]),
        )
        .unwrap();
        drop(std::mem::replace(engine, spare));
        expired.created_at = 0;
        for parent in [
            temp.path().join("state/workspaces"),
            temp.path().join("p/staging/workspaces"),
        ] {
            fs::write(
                parent.join(&workspace).join("workspace.json"),
                json_codec::to_vec(&expired).unwrap(),
            )
            .unwrap();
        }
        *engine = Engine::open(&temp.path().join("state")).unwrap();
    });
    let error = vfs.write(&mut session, 2, b"w").unwrap_err();
    assert_eq!(
        error_code(&error),
        storage_contract::ErrorCode::LeaseExpired
    );
    assert_eq!(vfs.sent.last(), Some(&Sent::Prepare));
    assert_eq!(vfs.dirty_count(), 1);
    let mut bytes = [0; 3];
    vfs.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"ned");
    assert_eq!(workspace_id(&session, vfs.handle), workspace);
}

#[test]
fn prepare_write_rejects_a_closed_workspace_before_the_descriptor_changes() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let mut vfs = Vfs::open(&mut session, "report.txt", false, false);
    vfs.write(&mut session, 0, b"n").unwrap();
    let workspace = workspace_id(&session, vfs.handle);
    session
        .shared
        .backend
        .abort_workspace(&session.identity, &workspace)
        .unwrap();
    let error = vfs.write(&mut session, 1, b"e").unwrap_err();
    assert_eq!(
        error_code(&error),
        storage_contract::ErrorCode::WorkspaceClosed
    );
    assert_eq!(vfs.sent.last(), Some(&Sent::Prepare));
    assert_eq!(vfs.file.metadata().unwrap().len(), 3);
    let mut bytes = [0; 3];
    vfs.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"nld");
}

#[test]
fn lost_core_publication_reply_requires_reconciliation_before_the_next_mutation() {
    for write_through in [false, true] {
        let (_temp, shared, backend, branch) = fixture();
        let mut session = session(shared, &branch);
        let mut vfs = Vfs::open(&mut session, "report.txt", false, write_through);
        let error = if write_through {
            backend.fail.store(true, Ordering::SeqCst);
            vfs.write(&mut session, 0, b"new").unwrap_err()
        } else {
            vfs.write(&mut session, 0, b"new").unwrap();
            backend.fail.store(true, Ordering::SeqCst);
            vfs.flush(&mut session).unwrap_err()
        };
        assert_eq!(
            error_code(&error),
            storage_contract::ErrorCode::InjectedFailure
        );
        let original = write_handle(&session, vfs.handle).operation_id;
        let error = vfs.write(&mut session, 0, b"bad").unwrap_err();
        assert_eq!(
            error_code(&error),
            storage_contract::ErrorCode::WorkspaceClosed
        );
        let mut bytes = [0; 3];
        vfs.file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"new");
        let uploads = backend.uploads.load(Ordering::SeqCst);
        let saved = vfs.flush(&mut session).unwrap();
        assert_eq!(saved["operation_id"], original.to_string());
        assert_eq!(backend.uploads.load(Ordering::SeqCst), uploads);
        vfs.write(&mut session, 0, b"end").unwrap();
        assert_ne!(write_handle(&session, vfs.handle).operation_id, original);
        session.close(vfs.handle, false).unwrap();
        assert_eq!(backend.uploads.load(Ordering::SeqCst), uploads + 1);
        assert_eq!(published(&session, "report.txt"), b"end");
        let entry = session
            .entries()
            .unwrap()
            .into_iter()
            .find(|entry| entry.path == "report.txt")
            .unwrap();
        assert_eq!(
            backend
                .file_history(&session.identity, &entry.file_id)
                .unwrap()
                .len(),
            3
        );
    }
}

#[test]
fn failed_successor_record_is_reannounced_without_another_workspace() {
    let (_temp, shared, backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let mut vfs = Vfs::open(&mut session, "report.txt", false, false);
    vfs.write(&mut session, 0, b"one").unwrap();
    vfs.flush(&mut session).unwrap();
    let record_path = write_handle(&session, vfs.handle)
        .source
        .with_extension("work.json");
    *backend.obstruct_work_record.lock().unwrap() = Some(record_path.clone());
    let error = vfs.write(&mut session, 0, b"two").unwrap_err();
    let api = error.get_ref().unwrap().downcast_ref::<ApiError>().unwrap();
    assert!(std::error::Error::source(api).is_some());
    let writer = write_handle(&session, vfs.handle);
    assert!(writer.dirty && !writer.committed);
    let (workspace, operation) = (writer.workspace.workspace_id.clone(), writer.operation_id);
    fs::remove_dir(&record_path).unwrap();
    vfs.write(&mut session, 0, b"end").unwrap();
    assert_eq!(vfs.dirty_count(), 3, "failed Dirty must not cache success");
    assert_eq!(workspace_id(&session, vfs.handle), workspace);
    let record: Value = json_codec::from_slice(&fs::read(record_path).unwrap()).unwrap();
    assert_eq!(record["operation_id"], operation.to_string());
    session.close(vfs.handle, false).unwrap();
    assert_eq!(published(&session, "report.txt"), b"end");
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 2);
}

#[test]
fn write_through_reports_and_saves_every_change() {
    let (_temp, shared, backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let mut vfs = Vfs::open(&mut session, "report.txt", false, true);
    vfs.write(&mut session, 0, b"n").unwrap();
    vfs.write(&mut session, 1, b"e").unwrap();
    assert_eq!(
        vfs.sent,
        [Sent::Prepare, Sent::Dirty, Sent::Prepare, Sent::Dirty]
    );
    assert_eq!(published(&session, "report.txt"), b"ned");
    let entry = session
        .entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == "report.txt")
        .unwrap();
    let history = backend
        .file_history(&session.identity, &entry.file_id)
        .unwrap();
    assert_eq!(
        history.len(),
        3,
        "the initial version plus one save per change"
    );
    session.close(vfs.handle, false).unwrap();
}

#[test]
fn a_generation_grows_its_budget_record_once_per_grant() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let mut vfs = Vfs::open(&mut session, "fresh.bin", true, false);
    let source = match &session.handles[&vfs.handle] {
        Handle::Write(writer) => writer.source.clone(),
        _ => panic!("expected a writer"),
    };
    let (creates, grows) = budget::persist::record_writes();
    let replacements = recovery_record::persist::replacements();
    for index in 0..64u64 {
        vfs.write(&mut session, index * 64, &[index as u8; 64])
            .unwrap();
    }
    assert_eq!(
        vfs.sent.len(),
        65,
        "64 admissions and one generation notice"
    );
    assert_eq!(budget::persist::record_writes(), (creates, grows + 1));
    let owner: Value =
        json_codec::from_slice(&fs::read(source.with_extension("budget.json")).unwrap()).unwrap();
    assert_eq!(owner["admitted_end"], "4096");
    // A created file is dirty from Open; announcing it rewrites nothing.
    assert_eq!(recovery_record::persist::replacements(), replacements);
    session.close(vfs.handle, false).unwrap();
    let expected = (0..64u8).flat_map(|byte| [byte; 64]).collect::<Vec<_>>();
    assert_eq!(published(&session, "fresh.bin"), expected);
}

/// Repeatable workload measurements. They print counts and timings instead of
/// asserting machine-dependent values; see docs/validation/smb-write-admission.md.
/// `ZXS_SMB_MEASURE_LEGACY=1` drives the pre-ADR-0010 VFS sequence (PrepareWrite
/// and Dirty for every change) for comparison against the same server.
mod measurement {
    use super::*;

    fn setting(name: &str, default: u64) -> u64 {
        std::env::var(name).map_or(default, |value| value.parse().unwrap())
    }

    /// One getppid syscall: a phase boundary for `strace` without a test hook.
    fn phase() {
        std::hint::black_box(std::os::unix::process::parent_id());
    }

    fn measured_session(
        fixture: &Fixture,
        base: &Arc<Shared>,
        branch: &str,
        limit: u64,
    ) -> Session {
        let spool = fixture.path().join("measure-spool");
        fs::create_dir(&spool).unwrap();
        fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(spool.join("empty-directory")).unwrap();
        let shared = Arc::new(Shared {
            budget: budget::Budget::recover(
                &spool,
                crate::SpoolLimits {
                    bytes_per_account: 1 << 40,
                    bytes_total: 1 << 40,
                    recovery_reserve_bytes: 1 << 30,
                },
            )
            .unwrap(),
            max_file_bytes: limit,
            authenticated_sessions: tokio::sync::Semaphore::new(MAX_AUTHENTICATED_SESSIONS),
            backend: base.backend.clone(),
            username: "alice".into(),
            auth: base.auth.clone(),
            spool,
            file_writers: Mutex::new(BTreeSet::new()),
            namespace_operations: Mutex::new(BTreeSet::new()),
        });
        Session {
            identity: identity(&shared),
            shared,
            branch: branch.into(),
            handles: BTreeMap::new(),
            next_handle: 1,
            writer_locks: BTreeSet::new(),
            max_file_bytes: limit,
        }
    }

    fn write_range(session: &mut Session, vfs: &mut Vfs, legacy: bool, total: u64, chunk: u64) {
        let buffer = vec![0xa5; chunk as usize];
        let mut offset = 0;
        while offset < total {
            let length = chunk.min(total - offset) as usize;
            if legacy {
                call(
                    session,
                    &mut vfs.sent,
                    VfsRequest::PrepareWrite {
                        handle: vfs.handle,
                        end: offset + length as u64,
                        bytes: length as u64,
                    },
                )
                .unwrap();
                vfs.file.write_all_at(&buffer[..length], offset).unwrap();
                call(
                    session,
                    &mut vfs.sent,
                    VfsRequest::Dirty { handle: vfs.handle },
                )
                .unwrap();
            } else {
                vfs.write(session, offset, &buffer[..length]).unwrap();
            }
            offset += length as u64;
        }
    }

    fn percentile(sorted: &[Duration], percent: usize) -> f64 {
        let rank = (sorted.len() * percent).div_ceil(100).max(1) - 1;
        sorted[rank].as_secs_f64() * 1000.0
    }

    #[test]
    #[ignore = "measurement workload; run explicitly with --ignored --exact --nocapture"]
    fn measure_sequential_write_admission() {
        let chunk = setting("ZXS_SMB_MEASURE_CHUNK", 65536);
        let total = setting("ZXS_SMB_MEASURE_BYTES", 1 << 30);
        let legacy = std::env::var_os("ZXS_SMB_MEASURE_LEGACY").is_some();
        let overwrite = std::env::var_os("ZXS_SMB_MEASURE_OVERWRITE").is_some();
        let (fixture, base, _backend, branch) = fixture();
        let mut session = measured_session(&fixture, &base, &branch, total);
        let mut vfs = Vfs::open(&mut session, "measure.bin", true, false);
        if overwrite {
            // Unmeasured: publish a full-length file, then overwrite it in place.
            write_range(&mut session, &mut vfs, false, total, 1 << 20);
            vfs.flush(&mut session).unwrap();
            vfs.sent.clear();
        }
        let (creates, grows) = budget::persist::record_writes();
        let replacements = recovery_record::persist::replacements();
        phase();
        let started = Instant::now();
        write_range(&mut session, &mut vfs, legacy, total, chunk);
        let written = started.elapsed();
        phase();
        let (creates_after, grows_after) = budget::persist::record_writes();
        let replacements_after = recovery_record::persist::replacements();
        let prepare = vfs
            .sent
            .iter()
            .filter(|sent| **sent == Sent::Prepare)
            .count();
        let dirty = vfs.dirty_count();
        let started = Instant::now();
        vfs.flush(&mut session).unwrap();
        let flushed = started.elapsed();
        phase();
        let started = Instant::now();
        session.close(vfs.handle, false).unwrap();
        let closed = started.elapsed();
        phase();
        println!(
            "{}",
            json!({
                "workload": if overwrite { "overwrite" } else { "append" },
                "sequence": if legacy { "per-write-dirty" } else { "generation-dirty" },
                "bytes": total.to_string(),
                "chunk": chunk.to_string(),
                "prepare_write": prepare,
                "dirty": dirty,
                "budget_record_creates": creates_after - creates,
                "budget_record_grows": grows_after - grows,
                "recovery_record_replacements": replacements_after - replacements,
                "write_phase_ms": written.as_secs_f64() * 1000.0,
                "flush_ms": flushed.as_secs_f64() * 1000.0,
                "close_ms": closed.as_secs_f64() * 1000.0,
            })
        );
        // Keep the directories for path-based classification of a strace log.
        if std::env::var_os("ZXS_SMB_MEASURE_KEEP").is_some() {
            println!("kept {}", fixture.path().display());
            std::mem::forget(fixture);
        }
    }

    #[test]
    #[ignore = "measurement workload; run explicitly with --ignored --exact --nocapture"]
    fn measure_flush_and_close_latency() {
        let chunk = setting("ZXS_SMB_MEASURE_CHUNK", 65536);
        let size = setting("ZXS_SMB_MEASURE_BYTES", 8 << 20);
        let iterations = setting("ZXS_SMB_MEASURE_ITERATIONS", 40);
        let legacy = std::env::var_os("ZXS_SMB_MEASURE_LEGACY").is_some();
        let (fixture, base, _backend, branch) = fixture();
        let mut session = measured_session(&fixture, &base, &branch, size + chunk);
        let (mut flushes, mut closes) = (Vec::new(), Vec::new());
        for iteration in 0..iterations {
            let mut vfs = Vfs::open(
                &mut session,
                &format!("latency-{iteration}.bin"),
                true,
                false,
            );
            write_range(&mut session, &mut vfs, legacy, size, chunk);
            let started = Instant::now();
            vfs.flush(&mut session).unwrap();
            flushes.push(started.elapsed());
            // One more change after the save, published by the final Close.
            if legacy {
                call(
                    &mut session,
                    &mut vfs.sent,
                    VfsRequest::PrepareWrite {
                        handle: vfs.handle,
                        end: size + chunk,
                        bytes: chunk,
                    },
                )
                .unwrap();
                vfs.file
                    .write_all_at(&vec![0x5a; chunk as usize], size)
                    .unwrap();
                call(
                    &mut session,
                    &mut vfs.sent,
                    VfsRequest::Dirty { handle: vfs.handle },
                )
                .unwrap();
            } else {
                vfs.write(&mut session, size, &vec![0x5a; chunk as usize])
                    .unwrap();
            }
            let started = Instant::now();
            session.close(vfs.handle, false).unwrap();
            closes.push(started.elapsed());
        }
        flushes.sort();
        closes.sort();
        let summary = |samples: &[Duration]| {
            json!({
                "p50_ms": percentile(samples, 50),
                "p95_ms": percentile(samples, 95),
                "p99_ms": percentile(samples, 99),
                "max_ms": samples.last().unwrap().as_secs_f64() * 1000.0,
            })
        };
        println!(
            "{}",
            json!({
                "sequence": if legacy { "per-write-dirty" } else { "generation-dirty" },
                "iterations": iterations,
                "bytes": size.to_string(),
                "chunk": chunk.to_string(),
                "flush": summary(&flushes),
                "close": summary(&closes),
            })
        );
    }
}
