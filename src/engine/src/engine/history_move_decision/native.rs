//! One native receipt template under the same retained decision-input manifest.
//! The Engine still cannot publish it without the missing authority closure.
use super::*;
use crate::business::NativeMoveDraft;

/// Verified durable template observation, not a published operation result.
#[derive(Debug)]
pub struct FileHistoryMoveNativePreparation {
    operation_id: OperationId,
    native_digest: String,
    request_digest: String,
    decision_inputs_digest: FileHistoryMoveDecisionDigest,
    source_records_hash: String,
    target_records_hash: String,
}
impl FileHistoryMoveNativePreparation {
    pub fn operation_id(&self) -> OperationId { self.operation_id }
    pub fn native_digest(&self) -> &str { &self.native_digest }
    pub fn request_digest(&self) -> &str { &self.request_digest }
    pub fn decision_inputs_digest(&self) -> &FileHistoryMoveDecisionDigest { &self.decision_inputs_digest }
    pub fn source_database_records_hash(&self) -> &str { &self.source_records_hash }
    pub fn target_database_records_hash(&self) -> &str { &self.target_records_hash }
    pub fn outstanding(&self) -> &[FileHistoryMoveDecisionObligation] { &OUTSTANDING }
}
fn report(materials: &Materials, native: &NativeMoveDraft) -> FileHistoryMoveNativePreparation {
    FileHistoryMoveNativePreparation {
        operation_id: native.operation_id(), native_digest: native.digest().into(),
        request_digest: native.request_digest().into(), decision_inputs_digest: materials.digest,
        source_records_hash: native.source_records_hash().into(),
        target_records_hash: native.target_records_hash().into(),
    }
}

impl Engine {
    /// Persist the ONE prospective native move operation, including both audit
    /// owners and its single event/principal, after exact input/FULL verification.
    /// This never creates separate copy/delete receipts and cannot emit success.
    pub fn prepare_file_history_move_native_operation(
        &mut self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveNativePreparation> {
        let (materials, native) = self.native_move_inputs(projection, recovery, cancellation)?;
        let prepare = || -> Result<()> {
            checkpoint(cancellation)?;
            store::Store::system(self, &materials, store::Mode::Read)?
                .install_native(&native, cancellation)?;
            for participant in &materials.record.disks {
                checkpoint(cancellation)?;
                store::Store::require_native_capacity(self, &materials, participant, &native)?;
                store::Store::disk(self, &materials, participant, store::Mode::Read)?
                    .install_native(&native, cancellation)?;
            }
            self.verify_native_move_inputs(projection, recovery, &materials, &native, cancellation)
        };
        prepare().map_err(|error| error.context(
            ErrorCode::RecoveryRequired,
            "unaccepted native move draft may be partially retained; resume the original operation, no move result or ACCEPT has been published",
        ))?;
        Ok(report(&materials, &native))
    }

    /// Cold read-only replay: original source and retained inputs independently
    /// reconstruct the native operation. Missing or changed files are not fixed.
    pub fn read_file_history_move_native_operation(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveNativePreparation> {
        let (materials, native) = self.native_move_inputs(projection, recovery, cancellation)?;
        self.verify_native_move_inputs(projection, recovery, &materials, &native, cancellation)?;
        Ok(report(&materials, &native))
    }

    fn native_move_inputs(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<(Materials, NativeMoveDraft)> {
        let targets = self.verify_file_history_move_target(projection, recovery, cancellation)?;
        let materials = Materials::capture(self, projection, recovery, &targets)?;
        self.verify_move_decision_inputs(projection, recovery, &materials, cancellation)?;
        let native = projection.native_draft(self, recovery, &materials.digest)?;
        // Ordinary receipt-derived FILE projections must reproduce BOTH exact
        // staged databases, including source D and only old D/undo on target.
        if native.source_records_hash() != materials.source.records_hash()
            || native.target_records_hash() != materials.target.records_hash()
        {
            return Err(Error::new(ErrorCode::PublicationInvalid,
                "native move receipt does not reproduce the exact two-file material closure"));
        }
        self.recheck_file_history_move_projection(projection, cancellation)?;
        Ok((materials, native))
    }

    fn verify_native_move_inputs(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        materials: &Materials,
        native: &NativeMoveDraft,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        checkpoint(cancellation)?;
        store::Store::system(self, materials, store::Mode::Read)?.verify_native(native, cancellation)?;
        for participant in &materials.record.disks {
            checkpoint(cancellation)?;
            store::Store::disk(self, materials, participant, store::Mode::Read)?
                .verify_native(native, cancellation)?;
        }
        self.verify_move_decision_inputs(projection, recovery, materials, cancellation)
    }
}
