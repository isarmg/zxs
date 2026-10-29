//! Bounded cross-member stage execution for one publication.
//!
//! A stage is one per-member unit of the publication protocol (object store
//! plus journal append, a seal write, a proof write). Members run in parallel
//! across disks; the stage returns only after every started task has finished,
//! so the next stage starts behind a barrier exactly as the serial loop did.
//! Same-disk order is preserved because each disk owns exactly one task per
//! stage and stages are strictly sequential.
use crate::disk::DiskHandle;
use crate::journal::JournalWriter;
use crate::{DiskConfig, Error, ErrorCode, Result};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Upper bound on scoped worker threads per stage. Fixed members per pool are
/// small; the bound protects pathological configurations, not throughput.
pub(super) const MAX_STAGE_WORKERS: usize = 8;

/// The publication stages, in protocol order. Payload stages run before the
/// decision; the rest form the member protocol from PREPARE to the proofs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StageKind {
    PayloadCopy,
    SnapshotInstall,
    ManifestInstall,
    Prepare,
    Accept,
    Finalize,
    Names,
    CommitProofs,
    Publication,
    Seals,
    PublicationProofs,
}

impl StageKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::PayloadCopy => "payload-copy",
            Self::SnapshotInstall => "snapshot-install",
            Self::ManifestInstall => "manifest-install",
            Self::Prepare => "prepare",
            Self::Accept => "accept",
            Self::Finalize => "finalize",
            Self::Names => "names",
            Self::CommitProofs => "commit-proofs",
            Self::Publication => "publication",
            Self::Seals => "seals",
            Self::PublicationProofs => "publication-proofs",
        }
    }
}

/// One member's resources for one stage. The journal borrow is exclusive and
/// disjoint across members, so member tasks never share a writer.
pub(super) struct MemberStage<'a> {
    pub(super) disk: &'a DiskConfig,
    pub(super) handle: &'a Arc<DiskHandle>,
    pub(super) journal: Option<&'a mut JournalWriter>,
}

impl MemberStage<'_> {
    pub(super) fn journal(&mut self) -> &mut JournalWriter {
        self.journal
            .as_deref_mut()
            .expect("invariant: journal stage task owns its member journal")
    }
}

/// Logical completed work, not syscall I/O. Completed immutable installs
/// include compare-and-sync of identical objects; bytes from failed/partial
/// writes are excluded. Read counts cover only explicitly observed reads, not
/// every filesystem read in validation, directory work or durable writers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageIo {
    /// Successful immutable-object install calls, including existing objects.
    pub objects_completed: u32,
    /// New frames whose append and sync succeeded; replayed frames are zero.
    pub records_completed: u32,
    /// Successful proof install calls, including existing identical proofs.
    pub proofs_completed: u32,
    /// Object/proof/frame bytes for successful calls and completed payload copies.
    /// Payload-side metadata and failed calls are not included.
    pub logical_bytes_completed: u64,
    pub observed_bytes_read: u64,
    /// Source lengths for completely successful payload-copy groups; does not
    /// include the subsequent full verification passes or failed groups.
    pub payload_source_bytes_completed: u64,
}

impl StageIo {
    pub(super) fn add(&mut self, other: StageIo) {
        self.objects_completed += other.objects_completed;
        self.records_completed += other.records_completed;
        self.proofs_completed += other.proofs_completed;
        self.logical_bytes_completed += other.logical_bytes_completed;
        self.observed_bytes_read += other.observed_bytes_read;
        self.payload_source_bytes_completed += other.payload_source_bytes_completed;
    }
}

/// Timing and accounting of one executed stage.
#[derive(Clone, Debug)]
pub struct StageTiming {
    pub stage: StageKind,
    /// Offset from the start of this publication's stage sequence.
    pub started_at: Duration,
    pub elapsed: Duration,
    /// Parallel outer-stage tasks. Nested per-version fan-out is described by
    /// its own fixed envelope, not added to this outer worker count.
    pub parallel: bool,
    /// Started outer workers (or one caller-thread worker on the serial path).
    pub workers: usize,
    pub members_completed: usize,
    pub io: StageIo,
}

/// Everything one publication did, stage by stage, in this process on these
/// devices. It is a record of work, never a throughput claim.
#[derive(Clone, Debug)]
pub struct PublicationStageReport {
    pub candidate_hash: String,
    pub members: usize,
    /// How the publication attempt ended (`published`, `in-doubt`, ...).
    pub outcome: &'static str,
    pub stages: Vec<StageTiming>,
}

impl PublicationStageReport {
    pub(super) fn new(candidate_hash: &str, members: usize) -> Self {
        Self {
            candidate_hash: candidate_hash.to_owned(),
            members,
            outcome: "running",
            stages: Vec::new(),
        }
    }

    pub(super) fn total_io(&self) -> StageIo {
        let mut total = StageIo::default();
        for stage in &self.stages {
            total.add(stage.io);
        }
        total
    }
}

/// Per-task outcome. Failed tasks retain earlier successfully completed calls;
/// the failed call's partial I/O is deliberately not reported as completed.
pub(super) struct TaskOutcome<T> {
    pub(super) value: Result<T>,
    pub(super) io: StageIo,
}

#[cfg(feature = "fault-injection")]
impl<T> TaskOutcome<T> {
    pub(super) fn failed(error: Error, io: StageIo) -> Self {
        Self {
            value: Err(error),
            io,
        }
    }
}

/// A stage that did not complete on every member.
#[derive(Debug)]
pub(super) struct StageFailure {
    pub(super) stage: StageKind,
    /// The error of the lowest failed member index, returned unchanged.
    pub(super) primary: Error,
    pub(super) completed: Vec<String>,
    /// Every failed member in member order; the first entry owns `primary`.
    pub(super) failed: Vec<(String, ErrorCode)>,
}

impl StageFailure {
    /// Log this failure once, at the stage boundary, with structured fields,
    /// and split it into the unchanged primary error and a short summary. The
    /// summary names configuration disk ids and error codes only, never paths
    /// or secrets.
    pub(super) fn into_parts(self) -> (Error, String) {
        let completed = self.completed.join(",");
        let failed = self
            .failed
            .iter()
            .map(|(disk, code)| format!("{disk}={code}"))
            .collect::<Vec<_>>()
            .join(",");
        tracing::warn!(
            target: "storage_engine::publication",
            stage = self.stage.label(),
            completed = %completed,
            failed = %failed,
            "publication stage did not complete on every member"
        );
        let summary = format!(
            "stage {}; completed [{completed}]; failed [{failed}]",
            self.stage.label()
        );
        (self.primary, summary)
    }
}

/// Outcome of a stage: the member results in member order and the timing.
pub(super) struct StageRun<T> {
    pub(super) values: Vec<T>,
    pub(super) timing: StageTiming,
}

struct Queue {
    next: usize,
    closed: bool,
}

struct Claimed<'a>(&'a Mutex<Queue>);
impl Drop for Claimed<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.lock().unwrap_or_else(|p| p.into_inner()).closed = true;
        }
    }
}

/// Run `work` once per member task with at most `MAX_STAGE_WORKERS` scoped
/// threads, claiming tasks in member order. A single task runs on the caller.
///
/// Guarantees: every task that started runs to its own completion (no
/// mid-write abort); after the first observed failure no further task is
/// claimed; all threads are joined before this returns; a worker panic is
/// resumed on the caller after the join; results are returned in member order;
/// the reported primary error is the one of the lowest failed member index.
pub(super) fn run_member_stage<T: Send>(
    stage: StageKind,
    started_at: Duration,
    tasks: Vec<MemberStage<'_>>,
    work: impl Fn(&mut MemberStage<'_>) -> TaskOutcome<T> + Sync,
) -> std::result::Result<StageRun<T>, (StageFailure, StageTiming)> {
    run_member_stage_with_limit(stage, started_at, tasks, MAX_STAGE_WORKERS, work)
}

/// Payload verification retains the format's large checksum-manifest budget.
/// Those stages can reduce concurrency without imposing a smaller file limit.
pub(super) fn run_member_stage_with_limit<T: Send>(
    stage: StageKind,
    started_at: Duration,
    tasks: Vec<MemberStage<'_>>,
    worker_limit: usize,
    work: impl Fn(&mut MemberStage<'_>) -> TaskOutcome<T> + Sync,
) -> std::result::Result<StageRun<T>, (StageFailure, StageTiming)> {
    let members = tasks.len();
    let requested_workers = members.min(worker_limit);
    let mut workers = requested_workers;
    let mut startup_error = None;
    let began = Instant::now();
    let disk_ids: Vec<String> = tasks.iter().map(|task| task.disk.disk_id.clone()).collect();
    let slots: Vec<Mutex<Option<MemberStage<'_>>>> = tasks
        .into_iter()
        .map(|task| Mutex::new(Some(task)))
        .collect();
    let outcomes: Vec<Mutex<Option<TaskOutcome<T>>>> =
        (0..members).map(|_| Mutex::new(None)).collect();
    let queue = Mutex::new(Queue {
        next: 0,
        closed: false,
    });
    let worker = || {
        loop {
            let index = {
                let mut queue = queue.lock().expect("invariant: stage queue poisoned");
                if queue.closed || queue.next == members {
                    return;
                }
                let index = queue.next;
                queue.next += 1;
                index
            };
            // Close admission during unwinding, after the claim lock was released.
            let _claimed = Claimed(&queue);
            let mut task = slots[index]
                .lock()
                .expect("invariant: stage task slot poisoned")
                .take()
                .expect("invariant: stage task is claimed once");
            let outcome = work(&mut task);
            let failed = outcome.value.is_err();
            *outcomes[index]
                .lock()
                .expect("invariant: stage outcome slot poisoned") = Some(outcome);
            if failed {
                queue
                    .lock()
                    .expect("invariant: stage queue poisoned")
                    .closed = true;
                return;
            }
        }
    };
    if requested_workers > 1 {
        let panic = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(requested_workers);
            for _ in 0..requested_workers {
                match std::thread::Builder::new().spawn_scoped(scope, worker) {
                    Ok(handle) => handles.push(handle),
                    Err(error) => {
                        queue
                            .lock()
                            .expect("invariant: stage queue poisoned")
                            .closed = true;
                        startup_error = Some(Error::from(error));
                        break;
                    }
                }
            }
            workers = handles.len();
            let mut panic = None;
            for handle in handles {
                if let Err(payload) = handle.join() {
                    panic.get_or_insert(payload);
                }
            }
            panic
        });
        if let Some(payload) = panic {
            std::panic::resume_unwind(payload);
        }
    } else {
        worker();
    }
    let elapsed = began.elapsed();
    let mut values = Vec::with_capacity(members);
    let mut completed = Vec::new();
    let mut failed = Vec::new();
    let mut primary = None;
    let mut io = StageIo::default();
    for (slot, disk_id) in outcomes.into_iter().zip(disk_ids) {
        // A slot left empty was never claimed: an earlier member failed first.
        let Some(outcome) = slot
            .into_inner()
            .expect("invariant: stage outcome slot poisoned")
        else {
            continue;
        };
        io.add(outcome.io);
        match outcome.value {
            Ok(value) => {
                completed.push(disk_id);
                values.push(value);
            }
            Err(error) => {
                failed.push((disk_id, error.code()));
                primary.get_or_insert(error);
            }
        }
    }
    let timing = StageTiming {
        stage,
        started_at,
        elapsed,
        parallel: workers > 1,
        workers,
        members_completed: completed.len(),
        io,
    };
    match primary.or(startup_error) {
        None => {
            assert_eq!(
                values.len(),
                members,
                "invariant: a stage without failures ran every member task"
            );
            Ok(StageRun { values, timing })
        }
        Some(primary) => Err((
            StageFailure {
                stage,
                primary,
                completed,
                failed,
            },
            timing,
        )),
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/publication/stages_tests.rs"]
mod tests;
