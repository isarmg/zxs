//! Deterministic ordinary Candidate and exact native dependencies for the one
//! selected move. Immutable objects are not journal PREPARE/ACCEPT or visibility.
use super::*;
use crate::business::{NativeMoveCandidate, NativeMoveDraft};
use crate::journal::{
    Candidate, CandidateData, PreparedMetadataObject, PreparedSnapshot, VersionEvidence,
};

mod objects;

pub(super) const MANIFEST: &str = "candidate-inputs.json";
const CANDIDATE: &str = "candidate.json";
const SYSTEM: &str = "candidate-system.sqlite";
const MAX_OBJECTS: usize = 32_768;
const MAX_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct Object {
    kind: &'static str,
    hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    bytes: u64,
}
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct Manifest<'a> {
    format: &'static str,
    operation_id: OperationId,
    choice_digest: &'a str,
    native_digest: &'a str,
    candidate_hash: &'a str,
    snapshot_hash: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    source_file_records_hash: &'a str,
    target_file_records_hash: &'a str,
    system_database_digest: String,
    objects: &'a [Object],
}

/// An observation of ordinary immutable objects on every original metadata
/// member. No result/success conversion and no authority to append a journal.
#[derive(Debug)]
pub struct FileHistoryMoveCandidatePreparation {
    operation: OperationId,
    choice_digest: String,
    candidate_hash: String,
    snapshot_hash: String,
    sequence: u64,
    metadata_members: usize,
    objects: usize,
    bytes_per_member: u64,
}
impl FileHistoryMoveCandidatePreparation {
    pub fn operation_id(&self) -> OperationId {
        self.operation
    }
    pub fn choice_digest(&self) -> &str {
        &self.choice_digest
    }
    pub fn candidate_hash(&self) -> &str {
        &self.candidate_hash
    }
    pub fn snapshot_hash(&self) -> &str {
        &self.snapshot_hash
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn metadata_members(&self) -> usize {
        self.metadata_members
    }
    pub fn objects(&self) -> usize {
        self.objects
    }
    pub fn bytes_per_member(&self) -> u64 {
        self.bytes_per_member
    }
    pub fn outstanding(&self) -> &[FileHistoryMoveDecisionObligation] {
        &OUTSTANDING
    }
}

pub(super) struct FrozenCandidate {
    outcome: NativeMoveCandidate,
    snapshot: PreparedSnapshot,
    histories: Vec<PreparedMetadataObject>,
    candidate: Candidate,
    candidate_object: PreparedMetadataObject,
    system: Vec<u8>,
    manifest: Vec<u8>,
    objects: Vec<Object>,
    ordinary_bytes: u64,
    choice_digest: String,
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "move candidate exceeds 32768 ordinary objects / 128 MiB of dependencies and candidate SYSTEM bytes",
    )
}
impl FrozenCandidate {
    fn capture(
        engine: &Engine,
        projection: &FileHistoryMoveProjection,
        native: &NativeMoveDraft,
        choice: &FileHistoryMoveJointChoice,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<Self> {
        checkpoint(cancellation)?;
        let outcome = native.candidate_draft()?;
        Self::require_same_files(engine, projection, native, &outcome)?;
        let snapshot = crate::journal::prepare_snapshot(outcome.state(), MAX_METADATA)?;
        let (history_root, histories) = crate::history_package::prepare_history_manifests_bounded(
            outcome.state(),
            MAX_METADATA,
            MAX_BYTES as usize,
            MAX_OBJECTS,
        )?;
        let result = outcome.result();
        // No fresh random transaction identity on retry or after restart. The
        // chosen operation and all original material hashes determine one slot.
        let transaction = object_hash("file-history-move-candidate-transaction", &choice.digest())?;
        let candidate = Candidate::new(
            CandidateData {
                pool_id: engine.config.pool_id.clone(),
                sequence: result.sequence,
                transaction_id: format!("tx_{}", &transaction[..32]),
                operation_id: result.operation_id,
                request_digest: result.request_digest.clone(),
                previous_snapshot: engine.snapshot.clone(),
                snapshot_hash: snapshot.hash.clone(),
                versions: projection
                    .version_mapping()
                    .values()
                    .map(|id| VersionEvidence {
                        version: outcome.state().versions[id].clone(),
                        placement: outcome.state().placements[id].clone(),
                    })
                    .collect(),
                metadata_disk_ids: engine
                    .members()
                    .iter()
                    .map(|disk| disk.disk_id.clone())
                    .collect(),
                history_roots: history_root.reference().clone(),
            },
            None,
        );
        engine.verify_candidate_membership(&candidate)?;
        crate::metadata_budget::preflight_publication_prepared(
            &engine.planning_context(),
            &candidate,
            result,
            outcome.state(),
            None,
            MAX_METADATA,
            Some((&snapshot, &history_root)),
        )?;
        let candidate_object = PreparedMetadataObject::new("candidate", &candidate, MAX_METADATA)?;
        let system = crate::engine::index::preparation::encode_move_candidate(&outcome, &|| {
            checkpoint(cancellation)
        })?;
        let mut frozen = Self {
            outcome,
            snapshot,
            histories,
            candidate,
            candidate_object,
            system,
            manifest: Vec::new(),
            objects: Vec::new(),
            ordinary_bytes: 0,
            choice_digest: choice.digest().into(),
        };
        frozen.describe_objects(cancellation)?;
        frozen.manifest = encode_metadata(
            &Manifest {
                format: "file-history-move-unaccepted-candidate-inputs",
                operation_id: projection.operation_id(),
                choice_digest: choice.digest(),
                native_digest: native.digest(),
                candidate_hash: &frozen.candidate_object.hash,
                snapshot_hash: &frozen.snapshot.hash,
                sequence: frozen.candidate.sequence,
                source_file_records_hash: native.source_records_hash(),
                target_file_records_hash: native.target_records_hash(),
                system_database_digest: blake3::hash(&frozen.system).to_hex().to_string(),
                objects: &frozen.objects,
            },
            MANIFEST_BYTES,
        )?;
        Ok(frozen)
    }

    fn require_same_files(
        engine: &Engine,
        projection: &FileHistoryMoveProjection,
        native: &NativeMoveDraft,
        outcome: &NativeMoveCandidate,
    ) -> Result<()> {
        let source = crate::container_storage::file_database::capture_copy(
            outcome.state(),
            projection.capture().source_file_id().as_str(),
            &projection.proposed_source_deletion().history_tree_id,
        )?;
        let target_history = &outcome.state().repositories
            [&projection.capture().namespace().target_repository_id]
            .history_tree_id;
        let target = crate::container_storage::file_database::capture_copy(
            outcome.state(),
            projection.target_file_id().as_str(),
            target_history,
        )?;
        if source.records_hash != native.source_records_hash()
            || target.records_hash != native.target_records_hash()
        {
            return Err(invalid(
                "prospective move outcome changed the original complete FILE closure",
            ));
        }
        // Compare complete local inventories, including recursive backup dirs,
        // rather than assuming receipt status cannot affect a scope projection.
        for disk in engine
            .registered_disks()
            .filter(|disk| disk.registration_mode == RegistrationMode::Enabled)
        {
            crate::engine::disk_catalog::require_same_move_current(
                native,
                outcome,
                disk.identity(),
            )?;
        }
        Ok(())
    }

    pub(super) fn hash(&self) -> &str {
        &self.candidate_object.hash
    }

    pub(super) fn artifacts(&self, metadata: bool) -> Vec<store::Artifact<'_>> {
        let mut artifacts = vec![store::Artifact::Exact {
            name: MANIFEST,
            bytes: &self.manifest,
        }];
        if metadata {
            artifacts.push(store::Artifact::Exact {
                name: CANDIDATE,
                bytes: self.candidate_object.bytes(),
            });
            artifacts.push(store::Artifact::Scoped {
                name: SYSTEM,
                bytes: &self.system,
            });
        }
        artifacts
    }

    fn report(&self) -> FileHistoryMoveCandidatePreparation {
        FileHistoryMoveCandidatePreparation {
            operation: self.candidate.operation_id,
            choice_digest: self.choice_digest.clone(),
            candidate_hash: self.candidate_object.hash.clone(),
            snapshot_hash: self.snapshot.hash.clone(),
            sequence: self.candidate.sequence,
            metadata_members: self.candidate.metadata_disk_ids.len(),
            objects: self.objects.len(),
            bytes_per_member: self.ordinary_bytes,
        }
    }
}

impl Engine {
    /// Freeze one ordinary candidate/snapshot/history object set on every
    /// original metadata member. The full selected inputs must remain present.
    /// No journal frame, published alias or operation receipt is produced.
    pub fn prepare_file_history_move_candidate(
        &mut self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveCandidatePreparation> {
        let (materials, native, scopes, choice) =
            self.selected_move_inputs(projection, recovery, cancellation)?;
        let frozen = FrozenCandidate::capture(self, projection, &native, &choice, cancellation)?;
        frozen.preflight(self, &materials, &native, &scopes, cancellation)?;
        let prepare = || -> Result<()> {
            // Every exact input manifest is synchronized before any ordinary
            // candidate object can exist. Partial installs remain unjournaled.
            store::Store::system(self, &materials, store::Mode::Read)?
                .install_candidate(&frozen, cancellation)?;
            for participant in &materials.record.disks {
                checkpoint(cancellation)?;
                store::Store::disk(self, &materials, participant, store::Mode::Read)?
                    .install_candidate(&frozen, cancellation)?;
            }
            frozen.install_objects(self, cancellation)?;
            self.verify_move_candidate(&materials, &frozen, cancellation)?;
            self.read_file_history_move_joint_choice(projection, recovery, cancellation)?;
            Ok(())
        };
        prepare().map_err(|error| error.context(ErrorCode::RecoveryRequired,
            "original native candidate material may be partially installed; preserve the choice fence and resume the same candidate, no move has published"))?;
        Ok(frozen.report())
    }

    /// Reconstruct independently from the frozen original source, then verify
    /// every participant and each original member's complete ordinary objects.
    /// Missing dependencies fail without fallback to another member or repair.
    pub fn read_file_history_move_candidate(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveCandidatePreparation> {
        let (materials, native, _, choice) =
            self.selected_move_inputs(projection, recovery, cancellation)?;
        let frozen = FrozenCandidate::capture(self, projection, &native, &choice, cancellation)?;
        self.verify_move_candidate(&materials, &frozen, cancellation)?;
        self.read_file_history_move_joint_choice(projection, recovery, cancellation)?;
        Ok(frozen.report())
    }

    fn verify_move_candidate(
        &self,
        materials: &Materials,
        frozen: &FrozenCandidate,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        store::Store::system(self, materials, store::Mode::Read)?
            .verify_candidate(frozen, cancellation)?;
        for participant in &materials.record.disks {
            checkpoint(cancellation)?;
            store::Store::disk(self, materials, participant, store::Mode::Read)?
                .verify_candidate(frozen, cancellation)?;
        }
        frozen.verify_objects(self, cancellation)?;
        self.require_move_choice_baseline()
    }
}
