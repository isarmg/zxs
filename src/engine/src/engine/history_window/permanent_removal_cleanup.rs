//! Read-only evidence of current-route cleanup, never node retirement authority.
use super::{HistoryCancellationCheckpoint, PermanentNodeRemovalAnalysis};
use crate::{ContentState, Engine, Error, ErrorCode, Result};
use storage_contract::history_window::{HistoryRoot, HistoryVersionId, HistoryWindowCancellation};

const MAX_REPLICAS: usize = 4096;
const MAX_READ_BYTES: usize = 64 * 1024 * 1024;
const MAX_CHECKPOINTS: usize = 131_072;

/// A published purge is not evidence that any particular copy has been removed.
#[derive(Debug)]
pub enum RemovalReplicaCleanupState {
    /// The current content state does not authorize body cleanup.
    NotRequested,
    /// No local authenticated tombstone was found. Absence alone proves nothing.
    MissingTombstone,
    /// A valid tombstone exists, but the body's current storage name still exists.
    PayloadPresent,
    /// Valid local publication/tombstone evidence and no body at either supported location.
    VerifiedAbsent,
    /// The concrete disk, I/O, layout or proof failure prevented this observation.
    Unverified(Error),
}

#[derive(Debug)]
pub struct RemovalReplicaCleanup {
    version_id: HistoryVersionId,
    slot_id: String,
    state: RemovalReplicaCleanupState,
}
impl RemovalReplicaCleanup {
    pub fn version_id(&self) -> &HistoryVersionId {
        &self.version_id
    }
    pub fn slot_id(&self) -> &str {
        &self.slot_id
    }
    pub fn state(&self) -> &RemovalReplicaCleanupState {
        &self.state
    }
}

/// Interval observation of the captured root's current placements only.
/// Not a durable receipt, an all-ever-copies erasure claim, or permission to remove
/// retained version/commit/source records. Cold callers must observe again.
#[derive(Debug)]
pub struct PermanentRemovalCleanupObservation {
    history_root: HistoryRoot,
    replicas: Vec<RemovalReplicaCleanup>,
}
impl PermanentRemovalCleanupObservation {
    pub fn history_root(&self) -> HistoryRoot {
        self.history_root
    }
    pub fn replicas(&self) -> &[RemovalReplicaCleanup] {
        &self.replicas
    }
    pub fn all_current_routes_absent(&self) -> bool {
        !self.replicas.is_empty()
            && self
                .replicas
                .iter()
                .all(|replica| matches!(replica.state, RemovalReplicaCleanupState::VerifiedAbsent))
    }
}

struct Budget<'a> {
    cancellation: &'a HistoryWindowCancellation,
    bytes: usize,
    checkpoints: usize,
    io_exhausted: bool,
}
impl Budget<'_> {
    fn observe(&mut self, bytes: usize) -> Result<()> {
        self.cancellation.checkpoint()?;
        if bytes > self.bytes || self.checkpoints == 0 {
            return Err(limit());
        }
        self.bytes -= bytes;
        self.checkpoints -= 1;
        Ok(())
    }

    fn read(&mut self, bytes: usize) -> Result<()> {
        match self.observe(bytes) {
            Err(error) if error.code() == ErrorCode::ResourceLimit => {
                self.io_exhausted = true;
                // A scan-wide budget is not corrupt disk metadata. Source
                // boundaries already preserve cancellation without revoking
                // their epoch; restore ResourceLimit outside those boundaries.
                Err(Error::new(
                    ErrorCode::OperationAborted,
                    "cleanup observation budget exhausted",
                ))
            }
            result => result,
        }
    }
}
fn limit() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "permanent removal cleanup observation exceeds its complete-result budget",
    )
}

pub(super) fn inspect(
    engine: &Engine,
    analysis: &PermanentNodeRemovalAnalysis,
    cancellation: &HistoryWindowCancellation,
) -> Result<PermanentRemovalCleanupObservation> {
    let versions = analysis.retirements().iter().map(|item| item.version_id().clone()).collect::<Vec<_>>();
    inspect_selected(engine, analysis.history_root(), &versions, cancellation)
}

pub(super) fn inspect_selected(
    engine: &Engine,
    history_root: HistoryRoot,
    versions: &[HistoryVersionId],
    cancellation: &HistoryWindowCancellation,
) -> Result<PermanentRemovalCleanupObservation> {
    let mut budget = Budget {
        cancellation,
        bytes: MAX_READ_BYTES,
        checkpoints: MAX_CHECKPOINTS,
        io_exhausted: false,
    };
    let mut replicas = Vec::new();
    for id in versions {
        budget.observe(0)?;
        // The just-rechecked complete component proves these State identities.
        let version = &engine.state.versions[id.as_str()];
        let placement = &engine.state.placements[id.as_str()];
        if placement.route_slot_ids.len() > MAX_REPLICAS - replicas.len() {
            return Err(limit());
        }
        for slot in &placement.route_slot_ids {
            budget.observe(0)?;
            let observed = match engine.state.content_states.get(id.as_str()).copied() {
                Some(state @ (ContentState::Purged | ContentState::Migrated | ContentState::Retired)) => {
                    inspect_replica(engine, version, slot, state, &mut budget)
                }
                None | Some(ContentState::Available) => {
                    Ok(RemovalReplicaCleanupState::NotRequested)
                }
            };
            let state = match observed {
                Err(_) if budget.io_exhausted => return Err(limit()),
                Ok(state) => state,
                Err(error)
                    if matches!(
                        error.code(),
                        ErrorCode::OperationAborted | ErrorCode::ResourceLimit
                    ) =>
                {
                    return Err(error);
                }
                Err(error) => RemovalReplicaCleanupState::Unverified(error),
            };
            replicas.push(RemovalReplicaCleanup {
                version_id: id.clone(),
                slot_id: slot.clone(),
                state,
            });
        }
    }
    budget.observe(0)?;
    Ok(PermanentRemovalCleanupObservation {
        history_root,
        replicas,
    })
}

fn inspect_replica(
    engine: &Engine,
    version: &crate::FileVersion,
    slot: &str,
    state: ContentState,
    budget: &mut Budget<'_>,
) -> Result<RemovalReplicaCleanupState> {
    let disk = engine.disk(slot)?;
    super::super::provenance::validate_owned_seal_observed(
        &engine.disk_resources,
        disk,
        &engine.metadata_integrity,
        &mut |bytes| budget.read(bytes),
    )?;
    let handle = engine.disk_resources.get(disk)?;
    let root = &handle.root;
    let directory = crate::disk::version_dir(disk, &version.history_tree_id, &version.version_id);
    if root
        .open_optional_read_regular(&directory.join("purged.json"))?
        .is_none()
    {
        return Ok(RemovalReplicaCleanupState::MissingTombstone);
    }
    let observe = &mut |bytes| budget.read(bytes);
    let stored: crate::FileVersion =
        root.read_json_observed(&directory.join("version.json"), observe)?;
    if stored != *version {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "cleanup version differs from the captured retained version",
        ));
    }
    let tombstone: crate::journal::ContentTombstone =
        crate::journal::decode_proof(&root.read_bytes_observed(
            &directory.join("purged.json"),
            crate::disk::MAX_METADATA,
            observe,
        )?)?;
    if tombstone.state != state || tombstone.candidate.sequence > engine.state.sequence {
        return Err(Error::new(
            ErrorCode::TombstoneInvalid,
            "cleanup tombstone differs from the captured content state or is from its future",
        ));
    }
    let metadata = super::super::provenance::MetadataReadContext::for_disk(
        &engine.config,
        &handle.seal,
        disk.metadata_member
            .then_some(engine.metadata_integrity.as_ref()),
    );
    crate::journal::verify_tombstone_content_at(
        root,
        &directory,
        version,
        &tombstone,
        observe,
        Some(&metadata),
    )?;
    Ok(
        match crate::container_storage::content_location_at(root, &directory, version, observe)? {
            crate::container_storage::ContentLocation::Present(_) => {
                RemovalReplicaCleanupState::PayloadPresent
            }
            crate::container_storage::ContentLocation::Absent(_) => {
                RemovalReplicaCleanupState::VerifiedAbsent
            }
        },
    )
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/removal_cleanup_budget_tests.rs"]
mod tests;
