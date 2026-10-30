//! Read-only verification of the retained publication closure and required FULL copies.
//!
//! Offline callers can keep exclusive access; managed online callers capture an
//! admitted baseline and acquire short per-version leases. Neither is a disk snapshot.
use super::Engine;
use crate::{disk::*, journal::*, *};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

mod metadata;
mod schedule;
pub use schedule::IntegrityScrubSchedule;
pub(super) use schedule::{ScheduleState, ScheduledScrub, ScrubCoveragePermit};

const MAX_RETAINED_ISSUES: usize = 128;
const MAX_BATCH_PAYLOAD_FILES: usize = 64;
const MAX_BATCH_LAYOUT_BYTES: usize = 64 * 1024;
const MAX_BATCH_LAYOUT_CHECKPOINTS: usize = 4096;

/// A one-way cancellation signal owned by the caller's managed scrub job.
#[derive(Debug, Default)]
pub struct IntegrityScrubCancellation(AtomicBool);

impl IntegrityScrubCancellation {
    pub fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn checkpoint(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(Error::new(
                ErrorCode::OperationAborted,
                "integrity scrub cancelled",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum IntegrityScrubCompletion {
    Running,
    Complete,
    Superseded,
    Cancelled,
    Failed(Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrityScrubArea {
    Identity,
    Metadata,
    Payload,
}

#[derive(Debug)]
pub enum IntegrityScrubTarget {
    Disk { slot_id: String },
    Journal { slot_id: String },
    Version { slot_id: String, version_id: String },
}

#[derive(Debug)]
pub struct IntegrityScrubIssue {
    pub area: IntegrityScrubArea,
    pub target: IntegrityScrubTarget,
    pub source: Error,
}

/// Interval observations of one admitted State, not a claim about later disk health.
/// Counts retained before interruption are partial. `Complete` means the specified
/// scan finished; only `is_healthy()` additionally requires every check to succeed.
#[derive(Debug)]
pub struct IntegrityScrubReport {
    pub sequence: u64,
    pub snapshot_hash: Option<String>,
    pub started_at: SystemTime,
    pub finished_at: SystemTime,
    pub completion: IntegrityScrubCompletion,
    pub metadata_members_checked: usize,
    pub metadata_publications_verified: usize,
    /// Exact domain/hash checks completed; a later schema or edge check can still fail.
    pub metadata_objects_verified: usize,
    pub journal_frames_verified: usize,
    /// Pending decisions are not published State and have no payload requirement here.
    pub unpublished_journal_records: usize,
    pub identity_failures: usize,
    pub metadata_failures: usize,
    pub payload_copies_required: usize,
    pub payload_copies_verified: usize,
    pub payload_copies_unavailable: usize,
    /// Identity or publication metadata failed before payload verification was possible.
    pub payload_copies_unverified: usize,
    pub tombstones_verified: usize,
    pub issues: Vec<IntegrityScrubIssue>,
    pub issues_omitted: usize,
}

impl IntegrityScrubReport {
    pub fn is_healthy(&self) -> bool {
        matches!(self.completion, IntegrityScrubCompletion::Complete)
            && self.identity_failures == 0
            && self.metadata_failures == 0
            && self.payload_copies_unavailable == 0
            && self.payload_copies_unverified == 0
            && self.payload_copies_verified == self.payload_copies_required
    }

    fn issue(&mut self, area: IntegrityScrubArea, target: IntegrityScrubTarget, source: Error) {
        match area {
            IntegrityScrubArea::Identity => self.identity_failures += 1,
            IntegrityScrubArea::Metadata => self.metadata_failures += 1,
            IntegrityScrubArea::Payload => self.payload_copies_unavailable += 1,
        }
        if self.issues.len() < MAX_RETAINED_ISSUES {
            self.issues.push(IntegrityScrubIssue {
                area,
                target,
                source,
            });
        } else {
            self.issues_omitted += 1;
        }
    }
}

/// Captured admitted metadata roots and payload obligations, not a disk snapshot.
/// The caller must keep its owning Engine alive until `finish_integrity_scrub`.
/// Existing immutable metadata has no runtime GC; the journal frontier is fixed.
pub struct IntegrityScrubPlan {
    config: ValidatedConfig,
    disk_resources: std::sync::Arc<DiskResources>,
    state: std::sync::Arc<State>,
    snapshot: Option<String>,
    tails: std::collections::BTreeMap<String, Option<String>>,
    owner: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, usize>>>,
    verify_eof: bool,
    metadata_integrity: std::sync::Arc<super::provenance::MetadataIntegrity>,
    metadata_epoch: super::provenance::MetadataEpoch,
    schedule: Option<ScheduledScrub>,
}

/// One version lease plus held payload handles. No byte verification occurs on admission.
pub struct IntegrityScrubBatch {
    baseline: std::sync::Arc<State>,
    config: ValidatedConfig,
    _lease: super::ReadLease,
    payloads: Vec<(String, Result<std::fs::File>)>,
    layout_bytes_read: usize,
}

pub enum IntegrityScrubBatchAdmission {
    Ready(IntegrityScrubBatch),
    Superseded,
}

/// A completed scan still needs its live Engine owner to accept the observation.
pub struct IntegrityScrubOutcome {
    baseline: std::sync::Arc<State>,
    metadata_epoch: super::provenance::MetadataEpoch,
    schedule: Option<ScheduledScrub>,
    started: std::time::Instant,
    owner: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, usize>>>,
    config: ValidatedConfig,
    report: IntegrityScrubReport,
    tails: std::collections::BTreeMap<String, Option<String>>,
    extra_tail: std::collections::BTreeSet<String>,
}

impl Engine {
    /// Capture only admitted in-memory roots/frontiers. Reject already detached preparation.
    /// No filesystem access; State and configuration are shared, not deep-copied.
    pub fn prepare_integrity_scrub(&self) -> Result<IntegrityScrubPlan> {
        self.require_no_commit_preparation()?;
        Ok(IntegrityScrubPlan {
            config: self.config.clone(),
            disk_resources: std::sync::Arc::clone(&self.disk_resources),
            state: std::sync::Arc::clone(&self.state),
            snapshot: self.snapshot.clone(),
            tails: self
                .journals
                .iter()
                .map(|(id, writer)| {
                    (
                        id.clone(),
                        writer.records().last().map(|(_, hash)| hash.clone()),
                    )
                })
                .collect(),
            owner: std::sync::Arc::clone(&self.reads),
            verify_eof: false,
            metadata_integrity: std::sync::Arc::clone(&self.metadata_integrity),
            metadata_epoch: self.metadata_integrity.capture(),
            schedule: None,
        })
    }

    /// Hold the Engine lock only for current binding comparison, one read lease,
    /// and body path resolution/open. Renames are serialized by that same lock;
    /// large proofs, manifests and payload hashing run outside it. A live lease
    /// keeps purge/split from deleting these bodies until this batch is dropped.
    pub fn admit_integrity_scrub_batch(
        &self,
        plan: &IntegrityScrubPlan,
        version_id: &str,
    ) -> Result<IntegrityScrubBatchAdmission> {
        self.require_no_commit_preparation()?;
        if !std::sync::Arc::ptr_eq(&self.reads, &plan.owner)
            || !self.config.same_admission(&plan.config)
        {
            return Ok(IntegrityScrubBatchAdmission::Superseded);
        }
        let version = plan.state.versions.get(version_id).ok_or_else(|| {
            Error::new(
                ErrorCode::VersionNotFound,
                "version is outside the scrub baseline",
            )
        })?;
        let placement = &plan.state.placements[version_id];
        if placement.route_slot_ids.len() > MAX_BATCH_PAYLOAD_FILES
            && matches!(
                plan.state.content_states.get(version_id),
                None | Some(ContentState::Available)
            )
        {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "scrub per-version file-descriptor budget exhausted",
            ));
        }
        if self.state.versions.get(version_id) != Some(version)
            || self.state.placements.get(version_id) != Some(placement)
            || self.state.content_states.get(version_id)
                != plan.state.content_states.get(version_id)
        {
            return Ok(IntegrityScrubBatchAdmission::Superseded);
        }
        *self
            .reads
            .lock()
            .expect("invariant: read lease lock poisoned")
            .entry(version_id.to_owned())
            .or_default() += 1;
        let lease = super::ReadLease {
            version: version_id.to_owned(),
            reads: std::sync::Arc::clone(&self.reads),
        };
        let mut payloads = Vec::new();
        let mut layout_bytes_read = 0;
        let mut layout_checkpoints = 0;
        // Path discovery needs authenticated namespace/owner markers. Meter
        // and bound those small reads here, but never invoke a sleeping caller
        // while its Engine lock is held. The run loop charges actual bytes later.
        let mut observe_layout = |bytes| {
            if bytes == 0 {
                if layout_bytes_read >= MAX_BATCH_LAYOUT_BYTES
                    || layout_checkpoints == MAX_BATCH_LAYOUT_CHECKPOINTS
                {
                    return Err(Error::new(
                        ErrorCode::ResourceLimit,
                        "scrub locked path-discovery budget exhausted",
                    ));
                }
                layout_checkpoints += 1;
            } else {
                layout_bytes_read += bytes;
            }
            Ok(())
        };
        if matches!(
            plan.state.content_states.get(version_id),
            None | Some(ContentState::Available)
        ) {
            for slot in &placement.route_slot_ids {
                let disk = self.disk(slot)?;
                let directory = version_dir(disk, &version.history_tree_id, version_id);
                payloads.push((
                    slot.clone(),
                    plan.disk_resources.get(disk).and_then(|handle| {
                        open_payload_file_observed_at(
                            &handle.root,
                            &directory,
                            version,
                            &mut observe_layout,
                        )
                    }),
                ));
            }
        }
        Ok(IntegrityScrubBatchAdmission::Ready(IntegrityScrubBatch {
            baseline: std::sync::Arc::clone(&plan.state),
            config: plan.config.clone(),
            _lease: lease,
            payloads,
            layout_bytes_read,
        }))
    }

    /// Accept interval observations only on the same still-live Engine/configuration.
    /// Unrelated publications do not invalidate this explicitly named baseline.
    pub fn finish_integrity_scrub(
        &self,
        mut outcome: IntegrityScrubOutcome,
    ) -> IntegrityScrubReport {
        if matches!(outcome.report.completion, IntegrityScrubCompletion::Running) {
            if !std::sync::Arc::ptr_eq(&self.reads, &outcome.owner)
                || !self.config.same_admission(&outcome.config)
            {
                outcome.report.completion = IntegrityScrubCompletion::Superseded;
            } else {
                outcome.report.completion = IntegrityScrubCompletion::Complete;
                for disk in self.config.metadata_members() {
                    let Some(captured) = outcome.tails.get(&disk.disk_id) else {
                        // No admitted anchor at capture is already a member
                        // identity/metadata issue, not a new runtime transition.
                        continue;
                    };
                    let Some(writer) = self.journals.get(&disk.disk_id) else {
                        self.metadata_integrity.invalidate();
                        outcome.report.completion = IntegrityScrubCompletion::Failed(Error::new(
                            ErrorCode::RecoveryRequired,
                            "scrub completion has no live admitted journal writer",
                        ));
                        break;
                    };
                    if writer.needs_recovery() {
                        self.metadata_integrity.invalidate();
                        outcome.report.completion = IntegrityScrubCompletion::Failed(Error::new(
                            ErrorCode::RecoveryRequired,
                            "journal writer became uncertain during integrity scrub",
                        ));
                        break;
                    }
                    if outcome.extra_tail.contains(&disk.disk_id) {
                        let captured = captured.as_deref();
                        let current = writer.records().last().map(|(_, hash)| hash.as_str());
                        if current == captured {
                            self.metadata_integrity.invalidate();
                            outcome.report.issue(IntegrityScrubArea::Metadata,
                                IntegrityScrubTarget::Journal { slot_id: disk.slot_id.clone() },
                                Error::new(ErrorCode::LogCorrupt, "observed bytes beyond the admitted frontier without an Engine journal advance"));
                        }
                    }
                }
            }
        }
        if outcome.report.is_healthy() && !outcome.metadata_epoch.matches(&self.metadata_integrity)
        {
            outcome.report.completion = IntegrityScrubCompletion::Superseded;
        }
        outcome.report.finished_at = SystemTime::now();
        self.finish_scrub_schedule(&outcome);
        outcome.report
    }

    /// Explicit exclusive/offline consumer. The managed online consumer instead
    /// captures a plan and only locks for individual batch admission and finish.
    pub fn scrub(
        &mut self,
        cancellation: &IntegrityScrubCancellation,
    ) -> Result<IntegrityScrubReport> {
        let mut plan = self.prepare_integrity_scrub()?;
        // An exclusive caller expects no authorized append, so also reject an
        // externally appended self-consistent tail beyond the admitted frontier.
        plan.verify_eof = true;
        let outcome = plan.run(
            cancellation,
            |plan, id| self.admit_integrity_scrub_batch(plan, id),
            |_| {},
            |_| {},
        );
        Ok(self.finish_integrity_scrub(outcome))
    }
}

impl IntegrityScrubPlan {
    pub fn sequence(&self) -> u64 {
        self.state.sequence
    }
    pub fn snapshot_hash(&self) -> Option<&str> {
        self.snapshot.as_deref()
    }

    /// Baseline required body bytes only, not a misleading exact total I/O estimate.
    /// Compute outside the Engine lock. Metadata/proofs/manifests add actual bytes.
    pub fn estimated_payload_bytes(&self) -> Result<u64> {
        self.state
            .versions
            .iter()
            .try_fold(0u64, |total, (id, version)| {
                if !matches!(
                    self.state.content_states.get(id),
                    None | Some(ContentState::Available)
                ) {
                    return Ok(total);
                }
                let copies = self.state.placements[id].route_slot_ids.len() as u64;
                version
                    .logical_size
                    .checked_mul(copies)
                    .and_then(|bytes| total.checked_add(bytes))
                    .ok_or_else(|| {
                        Error::new(ErrorCode::ResourceLimit, "scrub byte estimate exceeds u64")
                    })
            })
    }

    /// Run outside the Engine lock; acquire exactly one version batch at a time.
    /// `on_read` receives actual successful storage-file read bytes and may wait
    /// for rate allowance. Ordinary reads are <=64 KiB; one locked path-discovery
    /// batch defers its actual marker-byte charge (<128 KiB) until after unlock. It must wake on cancellation;
    /// the core checks the token before reads and immediately after the callback.
    /// Directory-enumeration syscalls are not byte metered; stored seals are.
    /// `on_progress` publishes partial counters promptly after an observed issue;
    /// Running is never healthy. Neither callback can interrupt a kernel I/O call.
    pub fn run(
        self,
        cancellation: &IntegrityScrubCancellation,
        mut acquire: impl FnMut(&IntegrityScrubPlan, &str) -> Result<IntegrityScrubBatchAdmission>,
        mut on_read: impl FnMut(usize),
        mut on_progress: impl FnMut(&IntegrityScrubReport),
    ) -> IntegrityScrubOutcome {
        let started = std::time::Instant::now();
        let started_at = SystemTime::now();
        let mut report = IntegrityScrubReport {
            sequence: self.state.sequence,
            snapshot_hash: self.snapshot.clone(),
            started_at,
            finished_at: started_at,
            completion: IntegrityScrubCompletion::Running,
            metadata_members_checked: 0,
            metadata_publications_verified: 0,
            metadata_objects_verified: 0,
            journal_frames_verified: 0,
            unpublished_journal_records: 0,
            identity_failures: 0,
            metadata_failures: 0,
            payload_copies_required: 0,
            payload_copies_verified: 0,
            payload_copies_unavailable: 0,
            payload_copies_unverified: 0,
            tombstones_verified: 0,
            issues: Vec::new(),
            issues_omitted: 0,
        };
        let mut observe = |bytes| {
            if bytes != 0 {
                on_read(bytes);
            }
            cancellation.checkpoint()
        };
        let mut extra_tail = std::collections::BTreeSet::new();
        {
            let mut terminal_observed = false;
            let mut progress = |report: &IntegrityScrubReport| {
                if !terminal_observed
                    && matches!(
                        report.completion,
                        IntegrityScrubCompletion::Cancelled
                            | IntegrityScrubCompletion::Failed(_)
                            | IntegrityScrubCompletion::Superseded
                    )
                {
                    // This is before terminal progress and any later limiter or
                    // Engine-lock wait. A stopped scan cannot authorize an
                    // already preparing writer while finish waits for its lock.
                    if let Some(run) = &self.schedule {
                        run.revoke(&self.metadata_epoch);
                    }
                    terminal_observed = true;
                }
                on_progress(report);
            };
            progress(&report);
            if let Err(error) = self.scan(
                &mut acquire,
                &mut observe,
                &mut progress,
                &mut report,
                &mut extra_tail,
            ) {
                report.completion = if error.code() == ErrorCode::OperationAborted {
                    IntegrityScrubCompletion::Cancelled
                } else {
                    IntegrityScrubCompletion::Failed(error)
                };
            }
            report.finished_at = SystemTime::now();
            progress(&report);
        }
        IntegrityScrubOutcome {
            baseline: self.state,
            metadata_epoch: self.metadata_epoch,
            schedule: self.schedule,
            started,
            owner: self.owner,
            config: self.config,
            report,
            tails: self.tails,
            extra_tail,
        }
    }

    fn scan(
        &self,
        acquire: &mut impl FnMut(&IntegrityScrubPlan, &str) -> Result<IntegrityScrubBatchAdmission>,
        observe: &mut impl FnMut(usize) -> Result<()>,
        progress: &mut impl FnMut(&IntegrityScrubReport),
        report: &mut IntegrityScrubReport,
        extra_tail: &mut std::collections::BTreeSet<String>,
    ) -> Result<()> {
        let mut identities = std::collections::BTreeMap::new();
        for disk in &self.config.disks {
            observe(0)?;
            match self.verify_disk_seal(disk, observe) {
                Ok(handle) => {
                    identities.insert(disk.slot_id.as_str(), handle);
                }
                Err(error) => {
                    let error = resumable_failure(error)?;
                    if disk.metadata_member {
                        self.metadata_integrity.fault(&error);
                        report.metadata_failures += 1;
                    }
                    report.issue(IntegrityScrubArea::Identity, disk_target(disk), error);
                    progress(report);
                }
            }
        }
        let mut acknowledged_unavailable = None;
        for disk in self.config.metadata_members() {
            if !identities.contains_key(disk.slot_id.as_str()) {
                continue;
            }
            report.metadata_members_checked += 1;
            match metadata::verify_member(self, disk, observe, progress, report) {
                Ok((evidence, extra)) => {
                    if extra {
                        extra_tail.insert(disk.disk_id.clone());
                    }
                    if acknowledged_unavailable.is_none() {
                        acknowledged_unavailable = Some(evidence);
                    }
                }
                Err(error) => {
                    let error = resumable_failure(error)?;
                    self.metadata_integrity.fault(&error);
                    report.issue(
                        IntegrityScrubArea::Metadata,
                        IntegrityScrubTarget::Journal {
                            slot_id: disk.slot_id.clone(),
                        },
                        error,
                    );
                }
            }
            progress(report);
        }
        for (id, version) in self.state.versions.iter() {
            observe(0)?;
            let mut batch = match acquire(self, id)? {
                IntegrityScrubBatchAdmission::Ready(batch) => batch,
                IntegrityScrubBatchAdmission::Superseded => {
                    report.completion = IntegrityScrubCompletion::Superseded;
                    return Ok(());
                }
            };
            // `acquire` is an external callback. A typed batch is proof for one
            // exact baseline/version/owner, not permission to substitute another
            // equal-content replica or an unrelated pool's read lease.
            if batch._lease.version != *id
                || !std::sync::Arc::ptr_eq(&batch._lease.reads, &self.owner)
                || !std::sync::Arc::ptr_eq(&batch.baseline, &self.state)
                || !batch.config.same_admission(&self.config)
            {
                return Err(Error::new(
                    ErrorCode::PlanInvalid,
                    "scrub callback returned a batch for a different baseline, owner or version",
                ));
            }
            let state = self.state.content_states.get(id).copied();
            let mut opened = std::collections::BTreeMap::new();
            let mut unavailable = std::collections::BTreeSet::new();
            let mut admission_failure = None;
            if matches!(state, None | Some(ContentState::Available)) {
                report.payload_copies_required += self.state.placements[id].route_slot_ids.len();
                for slot in &self.state.placements[id].route_slot_ids {
                    if !identities.contains_key(slot.as_str()) {
                        report.payload_copies_unverified += 1;
                    }
                }
            }
            // An open/length/layout failure has already been observed. Publish
            // it before the limiter can sleep or a large local proof is read.
            for (slot, result) in std::mem::take(&mut batch.payloads) {
                match result {
                    Ok(file) => {
                        opened.insert(slot, file);
                    }
                    Err(error) if error.code() == ErrorCode::ResourceLimit => {
                        if admission_failure.is_none() {
                            admission_failure = Some(error);
                        }
                    }
                    Err(error) => {
                        if identities.contains_key(slot.as_str()) {
                            report.issue(
                                IntegrityScrubArea::Payload,
                                IntegrityScrubTarget::Version {
                                    slot_id: slot.clone(),
                                    version_id: id.clone(),
                                },
                                error,
                            );
                            unavailable.insert(slot);
                        }
                    }
                }
            }
            if let Some(error) = admission_failure {
                report.completion = IntegrityScrubCompletion::Failed(error);
                progress(report);
                // Keep this already observed terminal resource failure primary
                // if cancellation arrives while paying its actual read charge.
                match observe(batch.layout_bytes_read) {
                    Ok(()) => (),
                    Err(error) if error.code() == ErrorCode::OperationAborted => (),
                    Err(error) => return Err(error),
                }
                return Ok(());
            }
            progress(report);
            // Actual marker reads were completed during admission; never sleep
            // under its lock. Pay that bounded burst before any further I/O.
            observe(batch.layout_bytes_read)?;

            for slot in &self.state.placements[id].route_slot_ids {
                observe(0)?;
                let disk = self
                    .config
                    .disks
                    .iter()
                    .find(|disk| disk.slot_id == *slot)
                    .expect("invariant: admitted placement references a configured disk");
                let Some(handle) = identities.get(slot.as_str()) else {
                    continue;
                };
                let metadata = super::provenance::MetadataReadContext::for_disk(
                    &self.config,
                    &handle.seal,
                    disk.metadata_member
                        .then_some(self.metadata_integrity.as_ref()),
                );
                let directory = version_dir(disk, &version.history_tree_id, id);
                let target = || IntegrityScrubTarget::Version {
                    slot_id: slot.clone(),
                    version_id: id.clone(),
                };
                match state {
                    None | Some(ContentState::Available) => {
                        let allows_missing_proof = acknowledged_unavailable
                            .as_ref()
                            .is_some_and(|proof| proof.contains(id, slot));
                        match verify_version_metadata_observed(
                            &handle.root,
                            &directory,
                            version,
                            allows_missing_proof,
                            observe,
                            Some(&metadata),
                        ) {
                            Ok(LocalReplicaMetadata::Published) => (),
                            Ok(LocalReplicaMetadata::AcknowledgedUnavailable) => {
                                if !unavailable.contains(slot) {
                                    report.issue(IntegrityScrubArea::Payload, target(), Error::new(ErrorCode::ContentUnavailable, "degraded publication acknowledged this unavailable route; local publication proof is absent"));
                                }
                                progress(report);
                                continue;
                            }
                            Err(error) => {
                                let error = resumable_failure(error)?;
                                if !unavailable.contains(slot) {
                                    report.payload_copies_unverified += 1;
                                }
                                report.issue(IntegrityScrubArea::Metadata, target(), error);
                                progress(report);
                                continue;
                            }
                        }
                        if unavailable.contains(slot) {
                            continue;
                        }
                        let file = opened.remove(slot).expect("invariant: successful batch admission holds each available required route");
                        let verified =
                            read_checksum_manifest_at(&handle.root, &directory, version, observe)
                                .and_then(|manifest| {
                                    verify_open_payload_observed(file, manifest, version, observe)
                                });
                        match verified {
                            Ok(_) => report.payload_copies_verified += 1,
                            Err(error) => {
                                let error = resumable_failure(error)?;
                                report.issue(IntegrityScrubArea::Payload, target(), error);
                            }
                        }
                    }
                    Some(state @ (ContentState::Purged | ContentState::Migrated)) => {
                        match verify_retained_tombstone_observed(
                            &handle.root,
                            &directory,
                            version,
                            state,
                            observe,
                            Some(&metadata),
                        ) {
                            Ok(true) => report.tombstones_verified += 1,
                            Ok(false) => (),
                            Err(error) => {
                                let error = resumable_failure(error)?;
                                report.issue(IntegrityScrubArea::Metadata, target(), error);
                            }
                        }
                    }
                }
                progress(report);
            }
            // Drop every held fd and the version lease before admitting the next batch.
            drop(opened);
            drop(batch);
        }
        for disk in &self.config.disks {
            let Some(_) = identities.get(disk.slot_id.as_str()) else {
                continue;
            };
            observe(0)?;
            match self.verify_disk_seal(disk, observe) {
                Ok(_) => (),
                Err(error) => {
                    let error = resumable_failure(error)?;
                    if disk.metadata_member {
                        self.metadata_integrity.fault(&error);
                        report.metadata_failures += 1;
                    }
                    report.issue(IntegrityScrubArea::Identity, disk_target(disk), error);
                    progress(report);
                }
            }
        }
        observe(0)
    }

    fn verify_disk_seal(
        &self,
        disk: &DiskConfig,
        observe: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<&DiskHandle> {
        let handle = self.disk_resources.get(disk)?;
        let stored = decode_seal(&handle.root.read_bytes_observed(
            &disk.root.join("disk.json"),
            MAX_METADATA,
            observe,
        )?)?;
        validate_stored_seal(&stored, &handle.seal)?;
        Ok(handle)
    }
}

fn disk_target(disk: &DiskConfig) -> IntegrityScrubTarget {
    IntegrityScrubTarget::Disk {
        slot_id: disk.slot_id.clone(),
    }
}

// These two causes mean the scan did not finish, not that an object was unhealthy.
fn resumable_failure(error: Error) -> Result<Error> {
    match error.code() {
        ErrorCode::OperationAborted | ErrorCode::ResourceLimit => Err(error),
        _ => Ok(error),
    }
}

enum LocalReplicaMetadata {
    Published,
    AcknowledgedUnavailable,
}

fn verify_version_metadata_observed(
    root: &DiskRoot,
    directory: &std::path::Path,
    version: &FileVersion,
    allows_missing_proof: bool,
    observe: &mut impl FnMut(usize) -> Result<()>,
    metadata: Option<&super::provenance::MetadataReadContext<'_>>,
) -> Result<LocalReplicaMetadata> {
    let stored: FileVersion = root.read_json_observed(&directory.join("version.json"), observe)?;
    if stored != *version {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "stored version differs from admitted version",
        ));
    }
    if allows_missing_proof
        && root
            .open_optional_regular(&directory.join("publication-proof.json"))?
            .is_none()
    {
        return Ok(LocalReplicaMetadata::AcknowledgedUnavailable);
    }
    verify_local_publication_at(root, directory, version, observe, metadata)?;
    Ok(LocalReplicaMetadata::Published)
}

fn verify_retained_tombstone_observed(
    root: &DiskRoot,
    directory: &std::path::Path,
    version: &FileVersion,
    state: ContentState,
    observe: &mut impl FnMut(usize) -> Result<()>,
    metadata: Option<&super::provenance::MetadataReadContext<'_>>,
) -> Result<bool> {
    // The safe component-by-component directory opener rejects symlink
    // ancestors before absence can be classified as a legitimately removed tree.
    match root.directory(directory) {
        Ok(_) => (),
        Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error),
    }
    let stored: FileVersion = root.read_json_observed(&directory.join("version.json"), observe)?;
    if stored != *version {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "retained version differs from admitted version",
        ));
    }
    let tombstone: ContentTombstone =
        root.read_json_observed(&directory.join("purged.json"), observe)?;
    if tombstone.state != state {
        return Err(Error::new(
            ErrorCode::TombstoneInvalid,
            "local tombstone differs from current content state",
        ));
    }
    verify_tombstone_content_at(root, directory, version, &tombstone, observe, metadata)?;
    Ok(true)
}

#[cfg(test)]
fn verify_retained_tombstone(
    directory: &std::path::Path,
    version: &FileVersion,
    state: ContentState,
) -> Result<bool> {
    let root = DiskRoot::open(directory.ancestors().nth(4).unwrap())?;
    verify_retained_tombstone_observed(&root, directory, version, state, &mut |_| Ok(()), None)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/integrity_scrub_tests.rs"]
mod tests;
