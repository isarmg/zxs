use crate::ApiError;
use json_codec::{Value, json};

pub(crate) fn operation_wire(op: storage_contract::OperationResult) -> Result<Value, ApiError> {
    let health = match op.current_health {
        storage_contract::CurrentHealth::Recovering => storage_contract::CurrentHealth::Unknown,
        health @ (storage_contract::CurrentHealth::Healthy
        | storage_contract::CurrentHealth::Degraded
        | storage_contract::CurrentHealth::Unavailable
        | storage_contract::CurrentHealth::Unknown) => health,
    };
    let current_health =
        json!({"state":health,"checked_at":checked_at(std::time::SystemTime::now())?});
    let content_save = op.kind == "workspace.commit"
        && op.protection_at_publish != storage_contract::ProtectionAtPublish::NotApplicable;
    if content_save {
        let complete = matches!(
            op.commit_state,
            storage_contract::CommitState::Published
                | storage_contract::CommitState::Acked
                | storage_contract::CommitState::PublishedDegraded
        );
        let state = if complete {
            "completed"
        } else if op.commit_state == storage_contract::CommitState::Aborted {
            "failed"
        } else if matches!(
            op.commit_state,
            storage_contract::CommitState::InDoubt
                | storage_contract::CommitState::RecoveryRequired
        ) {
            "in_doubt"
        } else {
            "processing"
        };
        let result = if complete {
            let versions = op
                .result
                .get("version_protection")
                .filter(|versions| versions.is_array())
                .ok_or_else(|| {
                    ApiError::new(
                        ::http::StatusCode::INTERNAL_SERVER_ERROR,
                        storage_contract::ErrorCode::StateInvalid,
                        "frozen publication protection evidence is missing",
                    )
                })?;
            json!({"outcome":if op.protection_at_publish==storage_contract::ProtectionAtPublish::TargetMet{"COMMITTED_AND_PROTECTED"}else{"COMMITTED_DEGRADED"},"commit_id":op.commit_id,"branch_id":op.branch_id,"branch_generation":field(&op.result,"branch_generation")?,"publication_sequence":op.sequence.to_string(),"membership_epoch":field(&op.result,"membership_epoch")?,"protection_at_publish":op.protection_at_publish,"versions":versions})
        } else {
            Value::Null
        };
        let error = if op.commit_state == storage_contract::CommitState::Aborted {
            json_codec::to_value(ApiError::conflict(
                storage_contract::ErrorCode::OperationAborted,
                "the operation was aborted before publication",
            ))
            .expect("invariant: internal response serialization succeeds")
        } else {
            Value::Null
        };
        Ok(
            json!({"operation_id":op.operation_id,"operation_kind":"content_save","request_digest":op.request_digest,"state":state,"commit_state":op.commit_state,"result":result,"error":error,"current_health":current_health}),
        )
    } else {
        Ok(
            json!({"operation_id":op.operation_id,"operation_kind":op.kind,"request_digest":op.request_digest,"state":if matches!(op.commit_state,storage_contract::CommitState::Published|storage_contract::CommitState::Acked){"completed"}else{"in_doubt"},"commit_state":op.commit_state,"protection_at_publish":op.protection_at_publish,"publication_sequence":op.sequence.to_string(),"result":op.result,"error":null,"current_health":current_health}),
        )
    }
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::invalid(format!("missing string field {key}")))
}

fn checked_at(time: std::time::SystemTime) -> Result<String, ApiError> {
    let invalid = || {
        ApiError::new(
            ::http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::IoError,
            "system clock cannot be represented as a UTC timestamp",
        )
    };
    let duration = time
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|source| invalid().with_source(source))?;
    // Duration's u64 seconds and subsecond field always fit i128 nanoseconds.
    let nanos =
        i128::from(duration.as_secs()) * 1_000_000_000 + i128::from(duration.subsec_nanos());
    let time = date_time::OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .map_err(|source| invalid().with_source(source))?;
    Ok(time
        .format(&date_time::format_description::well_known::Rfc3339)
        .expect("invariant: nonnegative UTC time is within the RFC3339 year range"))
}

#[cfg(test)]
#[path = "../../../test/server/unit/operation_wire_tests.rs"]
mod tests;
