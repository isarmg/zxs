//! Operation ownership and domain validity must precede health I/O.
use super::repo_for_history;
use crate::{ApiError, auth::Identity};
use storage_engine::{Engine, OperationRecord};

pub(super) fn authorized_record(
    engine: &Engine,
    identity: &Identity,
    operation_id: storage_contract::OperationId,
) -> Result<OperationRecord, ApiError> {
    find_authorized_record(engine, identity, operation_id)?.ok_or_else(|| {
        ApiError::new(
            ::http::StatusCode::NOT_FOUND,
            storage_contract::ErrorCode::OperationNotFound,
            "operation is not registered",
        )
    })
}

pub(super) fn find_authorized_record(
    engine: &Engine,
    identity: &Identity,
    operation_id: storage_contract::OperationId,
) -> Result<Option<OperationRecord>, ApiError> {
    let Some(lookup) = engine
        .find_operation_lookup(operation_id)
        .map_err(super::error)?
    else {
        return Ok(None);
    };
    if lookup.owner() != identity.principal {
        return Err(ApiError::forbidden());
    }
    let record = lookup.into_record().map_err(super::error)?;
    authorize_record(engine, identity, &record)?;
    Ok(Some(record))
}

pub(super) fn authorize_record(
    engine: &Engine,
    identity: &Identity,
    record: &OperationRecord,
) -> Result<(), ApiError> {
    if record.owner() != Some(identity.principal.as_str()) {
        return Err(ApiError::forbidden());
    }
    if let Some(history) = &record.result().history_tree_id {
        repo_for_history(engine, history)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/operations/tests.rs"]
mod tests;
