//! Durable inputs for one joint move, never a journal Candidate or decision.
//! File scopes are materialized; unresolved system/disk authority stays explicit.
use super::Engine;
use crate::{disk::*, *};
use data_serialization::Serialize;
use std::collections::{BTreeMap, BTreeSet};

mod materials;
mod native;
mod scopes;
pub use scopes::FileHistoryMoveScopedPreparation;
pub use native::FileHistoryMoveNativePreparation;
mod store;
use materials::Materials;

const DIRECTORY: &str = "file-history-move-decision-inputs";
const MANIFEST: &str = "inputs.json";
const LOGICAL: &str = "joint-metadata.json";
const STATE: &str = "unaccepted-state.json";
const SOURCE_DATABASE: &str = "source-file.sqlite";
const TARGET_DATABASE: &str = "target-file.sqlite";
const MANIFEST_BYTES: usize = 2 * 1024 * 1024;
const STATE_BYTES: usize = 12 * 1024 * 1024;

/// Still-required authority, not a boolean completion bit that a stored
/// preparation can turn into an ACCEPT. Every current report has all five.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(rename_all = "kebab-case")]
pub enum FileHistoryMoveDecisionObligation {
    UnifiedPermanentReceipt,
    SystemHistoricalDatabase,
    DiskHistoricalDatabases,
    JointJournalDecisionAndRecovery,
    TwoEndedReadFences,
}
const OUTSTANDING: [FileHistoryMoveDecisionObligation; 5] = [
    FileHistoryMoveDecisionObligation::UnifiedPermanentReceipt,
    FileHistoryMoveDecisionObligation::SystemHistoricalDatabase,
    FileHistoryMoveDecisionObligation::DiskHistoricalDatabases,
    FileHistoryMoveDecisionObligation::JointJournalDecisionAndRecovery,
    FileHistoryMoveDecisionObligation::TwoEndedReadFences,
];

/// Exact input-manifest identity. Neither its possession nor its persisted
/// self digest authorizes publication or proves those bytes still exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileHistoryMoveDecisionDigest(blake3::Hash);
impl std::fmt::Display for FileHistoryMoveDecisionDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0.to_hex().as_str())
    }
}

/// A non-publishing observation of the same original operation's materials.
/// Not serializable and deliberately has no conversion to OperationResult.
#[derive(Debug)]
pub struct FileHistoryMoveDecisionInputs {
    operation: OperationId,
    digest: FileHistoryMoveDecisionDigest,
    metadata_members: usize,
    participant_disks: usize,
    source_database_hash: String,
    target_database_hash: String,
    proposed_state_hash: String,
}
impl FileHistoryMoveDecisionInputs {
    pub fn operation_id(&self) -> OperationId { self.operation }
    pub fn digest(&self) -> &FileHistoryMoveDecisionDigest { &self.digest }
    pub fn metadata_members(&self) -> usize { self.metadata_members }
    pub fn participant_disks(&self) -> usize { self.participant_disks }
    pub fn source_database_records_hash(&self) -> &str { &self.source_database_hash }
    pub fn target_database_records_hash(&self) -> &str { &self.target_database_hash }
    pub fn proposed_state_hash(&self) -> &str { &self.proposed_state_hash }
    pub fn outstanding(&self) -> &[FileHistoryMoveDecisionObligation] { &OUTSTANDING }
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::PreparationStale, message)
}
fn limit() -> Error {
    Error::new(ErrorCode::ResourceLimit, "joint move input size overflow")
}
fn checkpoint(cancellation: &HistoryWindowCancellation) -> Result<()> {
    if cancellation.is_cancelled() {
        return Err(Error::new(ErrorCode::OperationAborted, "joint move input preparation cancelled"));
    }
    Ok(())
}
fn hash(domain: &'static [u8], bytes: &[u8]) -> blake3::Hash {
    let mut hash = blake3::Hasher::new();
    hash.update(domain);
    hash.update(&[0]);
    hash.update(bytes);
    hash.finalize()
}

impl Engine {
    /// Persist one original manifest with the complete proposal, both FILE
    /// databases, target graph/audit and exact three-scope obligations. Source,
    /// target and metadata participants are all required, not a quorum subset.
    /// Full pool State stays on system/metadata roots only, never payload-only
    /// disks. This prepares inputs only: no slot reservation or journal write.
    pub fn prepare_file_history_move_decision_inputs(
        &mut self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveDecisionInputs> {
        let targets = self.verify_file_history_move_target(projection, recovery, cancellation)?;
        let materials = Materials::capture(self, projection, recovery, &targets)?;
        checkpoint(cancellation)?;
        let prepare = || -> Result<()> {
            store::Store::system(self, &materials, store::Mode::Prepare)?
                .install(&materials, cancellation)?;
            for participant in &materials.record.disks {
                checkpoint(cancellation)?;
                store::Store::disk(self, &materials, participant, store::Mode::Prepare)?
                    .install(&materials, cancellation)?;
            }
            self.verify_move_decision_inputs(projection, recovery, &materials, cancellation)
        };
        prepare().map_err(|error| error.context(
            ErrorCode::RecoveryRequired,
            "joint move inputs may be partially retained; preserve them and resume the original operation, no ACCEPT or move success exists",
        ))?;
        Ok(materials.report())
    }

    /// Independently reconstruct from the current original capture and compare
    /// all retained replicas. Cold callers first resume that original envelope;
    /// missing inputs are errors here and are never silently filled or adopted.
    pub fn verify_file_history_move_decision_inputs(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        reviewed: &FileHistoryMoveDecisionDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveDecisionInputs> {
        let targets = self.verify_file_history_move_target(projection, recovery, cancellation)?;
        let materials = Materials::capture(self, projection, recovery, &targets)?;
        if materials.digest != *reviewed {
            return Err(invalid("move decision inputs differ from the reviewed original"));
        }
        self.verify_move_decision_inputs(projection, recovery, &materials, cancellation)?;
        Ok(materials.report())
    }

    /// Cold, read-only observation when the caller has resumed the original
    /// recovery envelope but has no process-local decision-input digest yet.
    /// Every manifest must equal the independently reconstructed original;
    /// this does not deserialize a stored "complete" flag or fill missing files.
    pub fn read_file_history_move_decision_inputs(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveDecisionInputs> {
        let targets = self.verify_file_history_move_target(projection, recovery, cancellation)?;
        let materials = Materials::capture(self, projection, recovery, &targets)?;
        self.verify_move_decision_inputs(projection, recovery, &materials, cancellation)?;
        Ok(materials.report())
    }

    fn verify_move_decision_inputs(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        materials: &Materials,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        checkpoint(cancellation)?;
        store::Store::system(self, materials, store::Mode::Read)?
            .verify(materials, cancellation)?;
        for participant in &materials.record.disks {
            checkpoint(cancellation)?;
            store::Store::disk(self, materials, participant, store::Mode::Read)?
                .verify(materials, cancellation)?;
        }
        // A manifest or valid SQLite self hash is never a body proof. This
        // rereads every target FULL and original inode marker, then every
        // recovery replica and the original State/config/namespace observation.
        let targets = self.verify_file_history_move_target(projection, recovery, cancellation)?;
        if targets.copy_identities() != &materials.record.target_copy_identities {
            return Err(invalid("original move FULL copy identities changed during input preparation"));
        }
        Ok(())
    }
}
