//! Read-only verification of the retained publication closure and required FULL copies.
//!
//! This first implementation deliberately keeps exclusive Engine access throughout
//! the scan. It is not a disk snapshot, a write permit, or a recovery operation.
use super::Engine;
use crate::{disk::*, journal::*, *};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

mod metadata;

const MAX_RETAINED_ISSUES: usize = 128;

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
    Complete,
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

impl Engine {
    /// Verify retained metadata on every configured metadata member and every
    /// required AVAILABLE payload route, without repairing or changing State.
    ///
    /// The exclusive borrow must remain protected by the service's Engine mutex.
    /// Existing detached commit preparation is rejected before any scan I/O.
    /// No future publication, purge, configuration change or recovery may overlap
    /// through this Engine. External changes remain possible: identities are read
    /// before each member and after the full scan, and all successful byte checks are interval
    /// observations. Cancellation is checked between frames, objects and chunks;
    /// it cannot interrupt an in-progress kernel filesystem call.
    ///
    /// # Errors
    /// Returns `CommitPreparationBusy` when an already admitted detached writer
    /// may still install directories. Runtime failure/interruption is in the report.
    pub fn scrub(
        &mut self,
        cancellation: &IntegrityScrubCancellation,
    ) -> Result<IntegrityScrubReport> {
        self.require_no_commit_preparation()?;
        let started_at = SystemTime::now();
        let mut report = IntegrityScrubReport {
            sequence: self.state.sequence,
            snapshot_hash: self.snapshot.clone(),
            started_at,
            finished_at: started_at,
            completion: IntegrityScrubCompletion::Complete,
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
        if let Err(error) = self.scrub_disks(cancellation, &mut report) {
            report.completion = if error.code() == ErrorCode::OperationAborted {
                IntegrityScrubCompletion::Cancelled
            } else {
                IntegrityScrubCompletion::Failed(error)
            };
        }
        report.finished_at = SystemTime::now();
        Ok(report)
    }

    fn scrub_disks(
        &self,
        cancellation: &IntegrityScrubCancellation,
        report: &mut IntegrityScrubReport,
    ) -> Result<()> {
        let mut identities = Vec::with_capacity(self.config.disks.len());
        for disk in &self.config.disks {
            cancellation.checkpoint()?;
            let identity = match validate_seal(&self.config, disk) {
                Ok(identity) => identity,
                Err(error) => {
                    if disk.metadata_member {
                        report.metadata_failures += 1;
                    }
                    report.issue(IntegrityScrubArea::Identity, disk_target(disk), error);
                    self.count_unverified_copies(disk, report);
                    continue;
                }
            };
            identities.push((disk, identity));
        }
        let mut acknowledged_unavailable = None;
        for (disk, _) in &identities {
            cancellation.checkpoint()?;
            if !disk.metadata_member {
                continue;
            }
            report.metadata_members_checked += 1;
            match metadata::verify_member(self, disk, cancellation, report) {
                Ok(evidence) => {
                    if acknowledged_unavailable.is_none() {
                        acknowledged_unavailable = Some(evidence);
                    }
                }
                Err(error) => {
                    let error = resumable_failure(error)?;
                    report.issue(
                        IntegrityScrubArea::Metadata,
                        IntegrityScrubTarget::Journal {
                            slot_id: disk.slot_id.clone(),
                        },
                        error,
                    );
                }
            }
        }
        for (disk, _) in &identities {
            self.scrub_payloads(
                disk,
                cancellation,
                acknowledged_unavailable.as_ref(),
                report,
            )?;
        }
        // Recheck earlier members after the last member's I/O as well. A disk
        // replaced while another disk is scanned must not yield a healthy run.
        for (disk, identity) in identities {
            cancellation.checkpoint()?;
            match validate_seal(&self.config, disk)
                .and_then(|after| stable_identity(&identity, &after))
            {
                Ok(()) => (),
                Err(error) => {
                    if disk.metadata_member {
                        report.metadata_failures += 1;
                    }
                    report.issue(IntegrityScrubArea::Identity, disk_target(disk), error);
                }
            }
        }
        cancellation.checkpoint()
    }

    fn count_unverified_copies(&self, disk: &DiskConfig, report: &mut IntegrityScrubReport) {
        for (id, placement) in &self.state.placements {
            if placement.route_slot_ids.contains(&disk.slot_id)
                && matches!(
                    self.state.content_states.get(id),
                    None | Some(ContentState::Available)
                )
            {
                report.payload_copies_required += 1;
                report.payload_copies_unverified += 1;
            }
        }
    }

    fn scrub_payloads(
        &self,
        disk: &DiskConfig,
        cancellation: &IntegrityScrubCancellation,
        acknowledged_unavailable: Option<&metadata::AcknowledgedUnavailable>,
        report: &mut IntegrityScrubReport,
    ) -> Result<()> {
        for (id, version) in self.state.versions.iter() {
            cancellation.checkpoint()?;
            let placement = &self.state.placements[id];
            if !placement.route_slot_ids.contains(&disk.slot_id) {
                continue;
            }
            let directory = version_dir(disk, &version.history_tree_id, id);
            let target = || IntegrityScrubTarget::Version {
                slot_id: disk.slot_id.clone(),
                version_id: id.clone(),
            };
            // Admitted State deliberately permits sparse AVAILABLE entries.
            match self.state.content_states.get(id).copied() {
                None | Some(ContentState::Available) => {
                    report.payload_copies_required += 1;
                    let allows_missing_proof = acknowledged_unavailable
                        .is_some_and(|evidence| evidence.contains(id, &disk.slot_id));
                    match verify_version_metadata(&directory, version, allows_missing_proof) {
                        Ok(LocalReplicaMetadata::Published) => (),
                        Ok(LocalReplicaMetadata::AcknowledgedUnavailable) => {
                            report.issue(IntegrityScrubArea::Payload, target(), Error::new(ErrorCode::ContentUnavailable,
                                "degraded publication acknowledged this unavailable route; local publication proof is absent"));
                            continue;
                        }
                        Err(error) => {
                            report.payload_copies_unverified += 1;
                            report.issue(IntegrityScrubArea::Metadata, target(), error);
                            continue;
                        }
                    }
                    match verify_payload_held_with(&directory, version, || {
                        cancellation.checkpoint()
                    }) {
                        Ok(_) => report.payload_copies_verified += 1,
                        Err(error) => {
                            let error = resumable_failure(error)?;
                            report.issue(IntegrityScrubArea::Payload, target(), error);
                        }
                    }
                }
                Some(state @ (ContentState::Purged | ContentState::Migrated)) => {
                    match verify_retained_tombstone(&directory, version, state) {
                        Ok(true) => report.tombstones_verified += 1,
                        Ok(false) => (),
                        Err(error) => report.issue(IntegrityScrubArea::Metadata, target(), error),
                    }
                }
            }
        }
        Ok(())
    }
}

fn disk_target(disk: &DiskConfig) -> IntegrityScrubTarget {
    IntegrityScrubTarget::Disk {
        slot_id: disk.slot_id.clone(),
    }
}

fn stable_identity(before: &DiskSeal, after: &DiskSeal) -> Result<()> {
    if before.device != after.device
        || before.inode != after.inode
        || before.physical_device != after.physical_device
        || before.filesystem_uuid != after.filesystem_uuid
        || before.binding_generation != after.binding_generation
    {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "disk changed during integrity scrub",
        ));
    }
    Ok(())
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

fn verify_version_metadata(
    directory: &std::path::Path,
    version: &FileVersion,
    allows_missing_proof: bool,
) -> Result<LocalReplicaMetadata> {
    let stored: FileVersion = read_json(&directory.join("version.json"))?;
    if stored != *version {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "stored version differs from admitted version",
        ));
    }
    if allows_missing_proof
        && open_optional_regular(&directory.join("publication-proof.json"))?.is_none()
    {
        return Ok(LocalReplicaMetadata::AcknowledgedUnavailable);
    }
    verify_local_publication(directory, version)?;
    Ok(LocalReplicaMetadata::Published)
}

fn verify_retained_tombstone(
    directory: &std::path::Path,
    version: &FileVersion,
    state: ContentState,
) -> Result<bool> {
    // The safe component-by-component directory opener rejects symlink
    // ancestors before absence can be classified as a legitimately removed tree.
    match existing_private_dir(directory) {
        Ok(_) => (),
        Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error),
    }
    let stored: FileVersion = read_json(&directory.join("version.json"))?;
    if stored != *version {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "retained version differs from admitted version",
        ));
    }
    let tombstone: ContentTombstone = read_json(&directory.join("purged.json"))?;
    if tombstone.state != state {
        return Err(Error::new(
            ErrorCode::TombstoneInvalid,
            "local tombstone differs from current content state",
        ));
    }
    verify_tombstone_content(directory, version, &tombstone)?;
    Ok(true)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/integrity_scrub_tests.rs"]
mod tests;
