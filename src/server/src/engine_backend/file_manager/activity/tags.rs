//! Container changes retain the original path and never claim a content node.
use crate::backend_activity::ActivityDetailSnapshot;
use crate::engine_backend::{
    error,
    file_manager::activity::{ActivityRows, PendingDetail, PendingEvent, Timestamp, record_event},
    repository,
};
use crate::ApiError;
use storage_contract::OperationResult;
use storage_engine::Engine;

pub(super) fn record_operation<'a>(
    engine: &'a Engine,
    events: &mut ActivityRows<'a>,
    operation: &'a OperationResult,
) -> Result<bool, ApiError> {
    let Some(outcome) = engine
        .container_tags_outcome(operation.operation_id)
        .map_err(error)?
    else {
        return Ok(false);
    };
    // A no-op still has a permanent successful receipt, but is not a change.
    if !outcome.changed {
        return Ok(true);
    }
    let repository = repository(engine, &outcome.request.context.repository_id.to_string())?;
    let path = if repository.file_manager_root {
        outcome.request.context.path.to_string()
    } else {
        format!("{}/{}", repository.name, outcome.request.context.path)
    };
    record_event(
        events,
        operation.sequence,
        operation.operation_id.to_string(),
        PendingEvent {
            timestamp: Timestamp::Receipt(operation.result.get("timestamp")),
            slot_id: repository.primary_slot_id.clone(),
            path,
            action: storage_contract::file_container_tags::KIND.into(),
            principal: engine.operation_owner(operation.operation_id).map_err(error)?,
            file_id: outcome.request.file_id.to_string(),
            message: String::new(),
            detail: PendingDetail::Captured(ActivityDetailSnapshot::FileOperation {
                version_id: None,
                node_id: None,
                branch_id: None,
            }),
        },
    );
    Ok(true)
}
