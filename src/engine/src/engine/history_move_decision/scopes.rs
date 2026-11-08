//! Complete source SYSTEM images and original disk history, plus separately
//! labelled unaccepted current projections. None invents a move PUBLICATION.
use super::*;
use crate::business::NativeMoveDraft;
use crate::engine::disk_catalog::PreparedDiskScopes;
use std::sync::atomic::AtomicBool;

mod projection;
pub(super) use projection::Scopes;

pub(super) const SCOPE_MANIFEST: &str = "scoped-databases.json";
pub(super) const MAX_DATABASE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SET_BYTES: usize = 128 * 1024 * 1024;
const MAX_DATABASES: usize = 2048 + 1 + 32 * 3;

/// Complete retained source history and unaccepted proposed SQL, not an
/// accepted slot, ordinary alias installation or successful operation result.
#[derive(Debug)]
pub struct FileHistoryMoveScopedPreparation {
    operation_id: OperationId,
    native_digest: String,
    scoped_digest: String,
    source_sequence: u64,
    system_publications: usize,
    participant_disks: usize,
    databases: usize,
    database_bytes: u64,
}
impl FileHistoryMoveScopedPreparation {
    pub fn operation_id(&self) -> OperationId { self.operation_id }
    pub fn native_digest(&self) -> &str { &self.native_digest }
    pub fn scoped_digest(&self) -> &str { &self.scoped_digest }
    pub fn source_sequence(&self) -> u64 { self.source_sequence }
    pub fn system_publications(&self) -> usize { self.system_publications }
    pub fn participant_disks(&self) -> usize { self.participant_disks }
    pub fn databases(&self) -> usize { self.databases }
    pub fn database_bytes(&self) -> u64 { self.database_bytes }
    pub fn outstanding(&self) -> &[FileHistoryMoveDecisionObligation] { &OUTSTANDING }
}

fn budget() -> Error {
    Error::new(ErrorCode::ResourceLimit,
        "move scoped preparation exceeds 2048 source publications / 32 disks / 128 MiB")
}

impl Engine {
    /// Prepare complete source publication-index images and each participant's
    /// original current/history, then the same native draft's proposed current
    /// indexes. Requires every original recovery/FULL/FILE/native material.
    /// It never installs normal database aliases, opens a slot or hides a file.
    pub fn prepare_file_history_move_scoped_databases(
        &mut self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveScopedPreparation> {
        let (materials, native, scopes) = self.move_scoped_inputs(projection, recovery, cancellation)?;
        // Reject configured capacity before writing the first scoped file. The
        // physical reserve and identities are checked again by each held store.
        for participant in &materials.record.disks {
            store::Store::require_scoped_capacity(self, &materials, participant, &native, &scopes)?;
        }
        let prepare = || -> Result<()> {
            checkpoint(cancellation)?;
            store::Store::system(self, &materials, store::Mode::Read)?
                .install_scopes(&scopes, cancellation)?;
            for participant in &materials.record.disks {
                checkpoint(cancellation)?;
                store::Store::disk(self, &materials, participant, store::Mode::Read)?
                    .install_scopes(&scopes, cancellation)?;
            }
            self.verify_move_scopes(projection, recovery, &materials, &native, &scopes, cancellation)
        };
        prepare().map_err(|error| error.context(ErrorCode::RecoveryRequired,
            "move scoped databases may be partially retained; resume only the original operation, no move decision or success exists"))?;
        Ok(scopes.report())
    }

    /// Independently reconstruct the whole source prefix and native draft,
    /// then read every required scoped replica. Missing data is never repaired.
    pub fn read_file_history_move_scoped_databases(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveScopedPreparation> {
        let (materials, native, scopes) = self.move_scoped_inputs(projection, recovery, cancellation)?;
        self.verify_move_scopes(projection, recovery, &materials, &native, &scopes, cancellation)?;
        Ok(scopes.report())
    }

    fn move_scoped_inputs(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<(Materials, NativeMoveDraft, Scopes)> {
        let (materials, native) = self.native_move_inputs(projection, recovery, cancellation)?;
        self.verify_native_move_inputs(projection, recovery, &materials, &native, cancellation)?;
        let scopes = Scopes::capture(self, &materials, &native, cancellation)?;
        self.recheck_file_history_move_projection(projection, cancellation)?;
        Ok((materials, native, scopes))
    }

    fn verify_move_scopes(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        materials: &Materials,
        native: &NativeMoveDraft,
        scopes: &Scopes,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        store::Store::system(self, materials, store::Mode::Read)?
            .verify_scopes(scopes, cancellation)?;
        for participant in &materials.record.disks {
            checkpoint(cancellation)?;
            store::Store::disk(self, materials, participant, store::Mode::Read)?
                .verify_scopes(scopes, cancellation)?;
        }
        // A SQL image and its manifest are not a body proof or a lease. Finish
        // on all original native inputs, FILE/FULL identities and source capture.
        self.verify_native_move_inputs(projection, recovery, materials, native, cancellation)
    }
}
