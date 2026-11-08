//! Current administrator admission and exact file-bound retry, under the engine owner.
use super::*;
use storage_contract::{OperationId, history_window::{HistoryFileId, HistoryWindowCancellation}, node_retirement::{Cleanup, Outcome, Preview, Replica, ReplicaState, Selection}};

fn identity(engine: &AdmissionEngine, caller: &Identity) -> Result<Identity, ApiError> {
    let current = current_protected_identity(engine, caller)?;
    // SMB soft delete does not authorize irreversible history/body retirement.
    if current.smb_binding.is_some() { return Err(ApiError::forbidden()); }
    Ok(current)
}
fn selected(request: &Selection) -> Result<storage_engine::PermanentNodeRemovalSelection, ApiError> {
    storage_engine::PermanentNodeRemovalSelection::new(request.version_ids().to_vec()).map_err(error)
}
fn read_request(request: &Selection) -> storage_contract::shared_history::SharedHistoryRequest {
    storage_contract::shared_history::SharedHistoryRequest {
        file_id: request.file_id().clone(), history_root: request.history_root(),
    }
}
pub(super) fn preview(backend: &EngineBackend, caller: &Identity, request: Selection, cancellation: HistoryWindowCancellation) -> Result<Preview, ApiError> {
    let read = {
        let engine = backend.engine.lock().expect("invariant: service state lock poisoned");
        identity(&engine, caller)?;
        engine.capture_shared_history(read_request(&request), cancellation).map_err(error)?
    };
    let analysis = read.permanent_removal_analysis(&selected(&request)?).map_err(error)?;
    let engine = backend.engine.lock().expect("invariant: service state lock poisoned");
    identity(&engine, caller)?;
    engine.recheck_shared_history(&read).map_err(error)?;
    let coupled_node_ids = analysis.retirements().iter().flat_map(|retirement| retirement.event_node_ids().iter().cloned()).collect();
    let result = Preview {
        file_id: analysis.origin_file_id().clone(), history_tree_id: analysis.history_tree_id().clone(), history_root: analysis.history_root(),
        version_ids: request.version_ids().to_vec(), coupled_node_ids,
        file_components: analysis.file_components().to_vec(), retained_commit_entries: u64::try_from(analysis.retained_commit_entries()).map_err(ApiError::invalid_source)?,
        affected_branch_heads: analysis.affected_branch_heads().to_vec(), frozen_files: analysis.frozen_files().to_vec(),
    };
    result.validate(&request).map_err(ApiError::invalid_source)?;
    Ok(result)
}
pub(super) fn submit(backend: &EngineBackend, caller: &Identity, request: Selection, operation: OperationId) -> Result<Outcome, ApiError> {
    let mut engine = backend.engine.lock().expect("invariant: service state lock poisoned");
    let current = identity(&engine, caller)?;
    let selection = selected(&request)?;
    let result = if engine.state().operations.contains_key(&operation) {
        // Existing receipt is checked before a capture of the now-stale root.
        engine.retry_file_node_retirement_at_root(operation, &current.principal, request.file_id(), request.history_root(), &selection)
    } else {
        let read = engine.capture_shared_history(read_request(&request), HistoryWindowCancellation::default()).map_err(error)?;
        engine.retire_file_nodes(&read, &selection, &current.principal, operation)
    }.map_err(error)?;
    outcome(request.file_id().clone(), result)
}
pub(super) fn resume(backend: &EngineBackend, caller: &Identity, file: HistoryFileId, operation: OperationId) -> Result<Outcome, ApiError> {
    let mut engine = backend.engine.lock().expect("invariant: service state lock poisoned");
    let current = identity(&engine, caller)?;
    let result = engine.resume_file_node_retirement_for_file(&file, operation, &current.principal).map_err(error)?;
    outcome(file, result)
}
fn outcome(file_id: HistoryFileId, result: storage_engine::PermanentNodeRetirementOutcome) -> Result<Outcome, ApiError> {
    let cleanup = match result.cleanup() {
        Err(error) => Cleanup::Unverified { error_code: error.code().as_str().into() },
        Ok(observation) => Cleanup::Observed {
            history_root: observation.history_root(),
            replicas: observation.replicas().iter().map(|replica| Replica {
                version_id: replica.version_id().clone(), slot_id: replica.slot_id().into(),
                status: match replica.state() {
                    storage_engine::RemovalReplicaCleanupState::NotRequested => ReplicaState::NotRequested,
                    storage_engine::RemovalReplicaCleanupState::MissingTombstone => ReplicaState::MissingTombstone,
                    storage_engine::RemovalReplicaCleanupState::PayloadPresent => ReplicaState::PayloadPresent,
                    storage_engine::RemovalReplicaCleanupState::VerifiedAbsent => ReplicaState::VerifiedAbsent,
                    storage_engine::RemovalReplicaCleanupState::Unverified(error) => ReplicaState::Unverified { error_code: error.code().as_str().into() },
                },
            }).collect(),
        },
    };
    Ok(Outcome { file_id, operation: crate::operation_wire::operation_wire(result.publication().clone().into())?, cleanup })
}
