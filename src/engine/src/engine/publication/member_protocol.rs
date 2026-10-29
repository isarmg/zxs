//! Engine-side drivers for the per-member publication stages.
//!
//! Each stage runs once per member through the bounded runner in `stages`.
//! The engine owns the only mutable state the tasks need (the member
//! journals); it hands out disjoint exclusive borrows for the stage and takes
//! them back at the barrier. Deterministic failpoints that name a member index
//! run the stage serially in member order, which is one legal interleaving of
//! the parallel schedule.
use super::Engine;
use super::stages::{
    MAX_STAGE_WORKERS, MemberStage, PublicationStageReport, StageFailure, StageIo, StageKind,
    StageRun, StageTiming, TaskOutcome, run_member_stage, run_member_stage_with_limit,
};
#[cfg(feature = "fault-injection")]
use crate::Error;
use crate::disk::DiskHandle;
use crate::journal::{JournalWriter, Receipt, RecordType};
use crate::{DiskConfig, Result};
use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(feature = "fault-injection")]
use std::sync::Mutex;
use std::time::Instant;

pub(super) use crate::journal::AppendMode;

/// A fault evaluated inside one member's task of one stage. Typed and armed
/// through `Engine::inject_member_fault_once`; never parsed from input.
#[cfg(feature = "fault-injection")]
pub enum MemberFault {
    /// The member fails before performing any write of this stage.
    FailBeforeWrite,
    /// The member performs its complete write and then reports a failure, as
    /// a device that persisted the data but lost the acknowledgement would.
    FailAfterWrite,
    /// The member waits on the barrier twice before writing, so the harness
    /// can observe the stage mid-flight and then release it.
    Pause(Arc<std::sync::Barrier>),
    /// Gate participating tasks before an optional fault. Dropping the release
    /// sender cancels the gate, so a timed-out harness can still join workers.
    Gated {
        ready: std::sync::mpsc::Sender<()>,
        // Preserve Engine's shared-read contract while this receiver is armed;
        // the task consumes it exclusively before waiting.
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        fault: Option<Box<MemberFault>>,
    },
    /// The member's own journal append in this stage hits the given device
    /// fault (a torn frame or a failed sync), exactly as a journal fault armed
    /// on that writer would.
    Journal(crate::JournalFault),
}

#[cfg(feature = "fault-injection")]
pub(crate) struct ArmedMemberFault {
    pub(crate) stage: StageKind,
    pub(crate) disk_id: String,
    pub(crate) fault: MemberFault,
}

/// The ordered stage timings of one publication attempt, finished into the
/// retained report exactly once.
pub(crate) struct StageSequence {
    started: Instant,
    report: PublicationStageReport,
    /// Diagnostic summary of the stage that failed, if any.
    failure: Option<String>,
}

impl StageSequence {
    pub(crate) fn new(candidate_hash: &str, members: usize) -> Self {
        Self::started_at(candidate_hash, members, Instant::now())
    }

    pub(super) fn started_at(candidate_hash: &str, members: usize, started: Instant) -> Self {
        Self {
            started,
            report: PublicationStageReport::new(candidate_hash, members),
            failure: None,
        }
    }

    pub(super) fn offset(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    pub(super) fn candidate_hash(&self) -> &str {
        &self.report.candidate_hash
    }

    pub(super) fn record(&mut self, timing: StageTiming) {
        self.report.stages.push(timing);
    }

    /// Record a stage that ran on the calling thread as one unit of work.
    pub(super) fn record_serial(
        &mut self,
        stage: StageKind,
        began: Instant,
        members_completed: usize,
        io: StageIo,
    ) {
        self.record(StageTiming {
            stage,
            started_at: began.duration_since(self.started),
            elapsed: began.elapsed(),
            parallel: false,
            workers: 1,
            members_completed,
            io,
        });
    }

    pub(super) fn prepend(&mut self, mut earlier: Vec<StageTiming>) {
        earlier.append(&mut self.report.stages);
        self.report.stages = earlier;
    }

    /// The failed stage's member summary, for the in-doubt message.
    pub(super) fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    fn fail(&mut self, failure: StageFailure) -> crate::Error {
        let (primary, summary) = failure.into_parts();
        self.failure = Some(summary);
        primary
    }

    /// Emit the one structured event for this publication and hand the report
    /// to the engine. Numbers are what this process did on these devices.
    pub(crate) fn finish(mut self, outcome: &'static str) -> Arc<PublicationStageReport> {
        self.report.outcome = outcome;
        let total = self.report.total_io();
        let stages = self
            .report
            .stages
            .iter()
            .map(|stage| {
                format!(
                    "{}:{}/{}:{}:{}us",
                    stage.stage.label(),
                    stage.members_completed,
                    stage.workers,
                    if stage.parallel { "parallel" } else { "serial" },
                    stage.elapsed.as_micros()
                )
            })
            .collect::<Vec<_>>()
            .join(";");
        tracing::info!(
            target: "storage_engine::publication",
            candidate_hash = %self.report.candidate_hash,
            members = self.report.members,
            outcome,
            elapsed_us = self.started.elapsed().as_micros() as u64,
            objects_completed = total.objects_completed,
            records_completed = total.records_completed,
            proofs_completed = total.proofs_completed,
            logical_bytes_completed = total.logical_bytes_completed,
            observed_bytes_read = total.observed_bytes_read,
            payload_source_bytes_completed = total.payload_source_bytes_completed,
            stages = %stages,
            "publication stages"
        );
        Arc::new(self.report)
    }
}

/// The member fault taken for one stage before the stage borrows the engine.
#[cfg(feature = "fault-injection")]
pub(super) type ArmedStageFault = Mutex<Vec<ArmedMemberFault>>;
#[cfg(not(feature = "fault-injection"))]
pub(super) struct ArmedStageFault {}

/// Which payload disks a proof stage covers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ProofDisks {
    /// Every required slot; an unavailable disk fails the stage.
    Required,
    /// Historical replay: a disk absent from this admission is skipped.
    Available,
}

/// One payload disk's share of a proof stage: the (version, slot) targets it
/// owns, in their original candidate order.
pub(super) struct ProofTarget<'a> {
    pub(super) disk: &'a DiskConfig,
    pub(super) handle: &'a Arc<DiskHandle>,
    /// (version evidence index, slot id) pairs in candidate order.
    pub(super) items: Vec<(usize, &'a str)>,
    /// Large checksum manifests retain the serial verification budget.
    parallel_verification: bool,
}

/// Group every required (version, slot) pair by its disk, keeping first-seen
/// disk order and the original order of pairs within a disk.
pub(super) fn group_proof_targets<'a>(
    engine: &'a Engine,
    candidate: &'a crate::journal::Candidate,
    disks: ProofDisks,
) -> Result<Vec<ProofTarget<'a>>> {
    let mut targets: Vec<ProofTarget<'a>> = Vec::new();
    for (index, evidence) in candidate.versions.iter().enumerate() {
        for slot in &evidence.placement.route_slot_ids {
            let disk = engine.disk(slot)?;
            if let Some(target) = targets
                .iter_mut()
                .find(|target| target.disk.disk_id == disk.disk_id)
            {
                target.items.push((index, slot));
                target.parallel_verification &=
                    crate::disk::fan_out_admitted(&evidence.version, MAX_STAGE_WORKERS);
                continue;
            }
            let handle = match disks {
                ProofDisks::Required => engine.disk_resources.get(disk)?,
                ProofDisks::Available => match engine.disk_resources.available(disk) {
                    Some(handle) => handle,
                    None => continue,
                },
            };
            targets.push(ProofTarget {
                disk,
                handle,
                items: vec![(index, slot)],
                parallel_verification: crate::disk::fan_out_admitted(
                    &evidence.version,
                    MAX_STAGE_WORKERS,
                ),
            });
        }
    }
    Ok(targets)
}

/// The grouped items of the target owning `task`'s disk.
pub(super) fn target_items<'t>(
    targets: &'t [ProofTarget<'_>],
    task: &MemberStage<'_>,
) -> &'t [(usize, &'t str)] {
    &targets
        .iter()
        .find(|target| target.disk.disk_id == task.disk.disk_id)
        .expect("invariant: proof task belongs to a grouped target")
        .items
}

/// Append one record through the member's own journal and account for it.
pub(super) fn append_record(
    task: &mut MemberStage<'_>,
    mode: AppendMode,
    kind: RecordType,
    sequence: u64,
    candidate: &str,
    object: &str,
    io: &mut StageIo,
) -> Result<Receipt> {
    let disk = task.disk;
    let journal = task.journal();
    let receipt = journal.append_mode(disk, kind, sequence, candidate, object, mode)?;
    let bytes = journal.last_frame_bytes();
    if bytes > 0 {
        io.records_completed += 1;
        io.logical_bytes_completed += bytes;
    }
    Ok(receipt)
}

#[cfg(feature = "fault-injection")]
fn injected(stage: StageKind, disk: &DiskConfig) -> Error {
    Error::new(
        crate::ErrorCode::InjectedFailure,
        format!(
            "member {} interrupted in stage {}",
            disk.slot_id,
            stage.label()
        ),
    )
}

#[cfg(feature = "fault-injection")]
fn with_member_fault<T>(
    stage: StageKind,
    armed: &ArmedStageFault,
    task: &mut MemberStage<'_>,
    work: &(impl Fn(&mut MemberStage<'_>) -> TaskOutcome<T> + Sync),
) -> TaskOutcome<T> {
    let fault = {
        let mut armed = armed
            .lock()
            .expect("invariant: armed member fault lock poisoned");
        armed
            .iter()
            .position(|armed| armed.disk_id == task.disk.disk_id)
            .map(|index| armed.remove(index).fault)
    };
    let mut fault = fault;
    loop {
        match fault {
            Some(MemberFault::Gated {
                ready,
                release,
                fault: inner,
            }) => {
                if ready.send(()).is_err()
                    || release
                        .into_inner()
                        .expect("invariant: armed gate receiver has no shared consumer")
                        .recv()
                        .is_err()
                {
                    return TaskOutcome::failed(injected(stage, task.disk), StageIo::default());
                }
                fault = inner.map(|fault| *fault);
            }
            fault => return apply_member_fault(stage, task, work, fault),
        }
    }
}

#[cfg(feature = "fault-injection")]
fn apply_member_fault<T>(
    stage: StageKind,
    task: &mut MemberStage<'_>,
    work: &(impl Fn(&mut MemberStage<'_>) -> TaskOutcome<T> + Sync),
    fault: Option<MemberFault>,
) -> TaskOutcome<T> {
    match fault {
        None => work(task),
        Some(MemberFault::FailBeforeWrite) => {
            TaskOutcome::failed(injected(stage, task.disk), StageIo::default())
        }
        Some(MemberFault::FailAfterWrite) => {
            let mut outcome = work(task);
            if outcome.value.is_ok() {
                outcome.value = Err(injected(stage, task.disk));
            }
            outcome
        }
        Some(MemberFault::Pause(barrier)) => {
            barrier.wait();
            barrier.wait();
            work(task)
        }
        Some(MemberFault::Journal(fault)) => {
            task.journal().fault = Some(fault);
            work(task)
        }
        Some(MemberFault::Gated { .. }) => unreachable!("invariant: member gates were consumed"),
    }
}

impl Engine {
    /// Run one per-member stage across the metadata members.
    ///
    /// `failpoint_prefix` names the deterministic per-member failpoint family
    /// of this stage (`after_accept_` and so on). If a fault is armed for that
    /// family the stage runs serially in member order and consults the
    /// failpoint after each member exactly as the original loop did; otherwise
    /// members run in parallel through the bounded runner. Journal fences are
    /// applied once the stage has returned. A failure returns the error of the
    /// lowest failed member index unchanged; the member summary is kept in
    /// `sequence` for the caller's in-doubt message.
    pub(super) fn run_member_stage<T: Send>(
        &mut self,
        sequence: &mut StageSequence,
        stage: StageKind,
        members: &[DiskConfig],
        failpoint_prefix: Option<&str>,
        work: impl Fn(&mut MemberStage<'_>) -> TaskOutcome<T> + Sync,
    ) -> Result<Vec<T>> {
        let armed = self.take_member_fault(stage);
        #[cfg(feature = "fault-injection")]
        let work = |task: &mut MemberStage<'_>| with_member_fault(stage, &armed, task, &work);
        #[cfg(not(feature = "fault-injection"))]
        let _ = armed;
        #[cfg(feature = "fault-injection")]
        if let Some(prefix) = failpoint_prefix.filter(|prefix| {
            self.fault
                .as_ref()
                .is_some_and(|fault| fault.point.starts_with(prefix))
        }) {
            return self.run_member_stage_serially(sequence, stage, members, prefix, &work);
        }
        #[cfg(not(feature = "fault-injection"))]
        let _ = failpoint_prefix;
        let result = {
            let mut writers: BTreeMap<&str, &mut JournalWriter> = self
                .journals
                .iter_mut()
                .map(|(disk_id, writer)| (disk_id.as_str(), writer))
                .collect();
            let mut tasks = Vec::with_capacity(members.len());
            for disk in members {
                tasks.push(MemberStage {
                    disk,
                    handle: self.disk_resources.get(disk)?,
                    journal: Some(
                        writers
                            .remove(disk.disk_id.as_str())
                            .expect("invariant: writable metadata member owns a journal"),
                    ),
                });
            }
            // A worker panic still joins every task. Preserve its payload,
            // but fence the owner before unwinding past the engine boundary.
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_member_stage(stage, sequence.offset(), tasks, work)
            }))
        };
        self.fence_failed_journals(members);
        match result {
            Ok(result) => finish_stage(sequence, result),
            Err(payload) => {
                self.blocked = true;
                self.metadata_integrity.invalidate();
                std::panic::resume_unwind(payload)
            }
        }
    }

    #[cfg(feature = "fault-injection")]
    fn run_member_stage_serially<T>(
        &mut self,
        sequence: &mut StageSequence,
        stage: StageKind,
        members: &[DiskConfig],
        prefix: &str,
        work: &(impl Fn(&mut MemberStage<'_>) -> TaskOutcome<T> + Sync),
    ) -> Result<Vec<T>> {
        let began = Instant::now();
        let mut values = Vec::with_capacity(members.len());
        let mut io = StageIo::default();
        let mut completed = Vec::new();
        for (index, disk) in members.iter().enumerate() {
            let outcome = {
                let handle = self.disk_resources.get(disk)?;
                let journal = self
                    .journals
                    .get_mut(&disk.disk_id)
                    .expect("invariant: writable metadata member owns a journal");
                let mut task = MemberStage {
                    disk,
                    handle,
                    journal: Some(journal),
                };
                work(&mut task)
            };
            io.add(outcome.io);
            self.fence_failed_journals(std::slice::from_ref(disk));
            match outcome.value {
                Ok(value) => {
                    values.push(value);
                    completed.push(disk.disk_id.clone());
                }
                Err(error) => {
                    sequence.record_serial(stage, began, completed.len(), io);
                    let code = error.code();
                    return Err(sequence.fail(StageFailure {
                        stage,
                        primary: error,
                        completed,
                        failed: vec![(disk.disk_id.clone(), code)],
                    }));
                }
            }
            if let Err(error) = self.failpoint(&format!("{prefix}{}", index + 1)) {
                sequence.record_serial(stage, began, completed.len(), io);
                return Err(error);
            }
        }
        sequence.record_serial(stage, began, completed.len(), io);
        Ok(values)
    }

    /// Take the member fault armed for `stage`, if any, so it is evaluated by
    /// exactly one task of this stage and never leaks into a later stage.
    pub(super) fn take_member_fault(&mut self, stage: StageKind) -> ArmedStageFault {
        #[cfg(feature = "fault-injection")]
        {
            let (matching, remaining) = self
                .member_faults
                .drain(..)
                .partition(|armed| armed.stage == stage);
            self.member_faults = remaining;
            Mutex::new(matching)
        }
        #[cfg(not(feature = "fault-injection"))]
        {
            let _ = stage;
            ArmedStageFault {}
        }
    }

    /// Run one per-disk stage across payload disks that carry proofs. These
    /// stages write no journal records, so no fence applies and the engine is
    /// only borrowed immutably.
    pub(super) fn run_disk_stage<T: Send>(
        &self,
        sequence: &mut StageSequence,
        stage: StageKind,
        armed: &ArmedStageFault,
        targets: &[ProofTarget<'_>],
        work: impl Fn(&mut MemberStage<'_>) -> TaskOutcome<T> + Sync,
    ) -> Result<Vec<T>> {
        #[cfg(feature = "fault-injection")]
        let work = |task: &mut MemberStage<'_>| with_member_fault(stage, armed, task, &work);
        #[cfg(not(feature = "fault-injection"))]
        let _ = armed;
        let tasks = targets
            .iter()
            .map(|target| MemberStage {
                disk: target.disk,
                handle: target.handle,
                journal: None,
            })
            .collect();
        let worker_limit = if targets.iter().all(|target| target.parallel_verification) {
            MAX_STAGE_WORKERS
        } else {
            1
        };
        let result =
            run_member_stage_with_limit(stage, sequence.offset(), tasks, worker_limit, work);
        finish_stage(sequence, result)
    }
}

fn finish_stage<T>(
    sequence: &mut StageSequence,
    result: std::result::Result<StageRun<T>, Box<(StageFailure, StageTiming)>>,
) -> Result<Vec<T>> {
    match result {
        Ok(run) => {
            sequence.record(run.timing);
            Ok(run.values)
        }
        Err(failed) => {
            let (failure, timing) = *failed;
            sequence.record(timing);
            Err(sequence.fail(failure))
        }
    }
}
