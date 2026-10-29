use crate::ApiError;
use json_codec::{Value, json};

pub(crate) fn operation_wire(op: storage_contract::OperationOutcome) -> Result<Value, ApiError> {
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
            OperationPhase::Completed
        } else if op.commit_state == storage_contract::CommitState::Aborted {
            OperationPhase::Failed
        } else if matches!(
            op.commit_state,
            storage_contract::CommitState::InDoubt
                | storage_contract::CommitState::RecoveryRequired
        ) {
            OperationPhase::InDoubt
        } else {
            OperationPhase::Processing
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
            json!({"operation_id":op.operation_id,"operation_kind":"content_save","request_digest":op.request_digest,"state":state,"commit_state":op.commit_state,"result":result,"error":error}),
        )
    } else {
        Ok(
            json!({"operation_id":op.operation_id,"operation_kind":op.kind,"request_digest":op.request_digest,"state":if matches!(op.commit_state,storage_contract::CommitState::Published|storage_contract::CommitState::Acked){OperationPhase::Completed}else{OperationPhase::InDoubt},"commit_state":op.commit_state,"protection_at_publish":op.protection_at_publish,"publication_sequence":op.sequence.to_string(),"result":op.result,"error":null}),
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

pub(crate) fn operation_observation_wire(op: crate::OperationObservation) -> Result<Value, ApiError> {
    let mut value = operation_wire(op.outcome)?;
    value.as_object_mut().expect("invariant: operation projection is an object").insert(
        "current_health".into(),
        json_codec::to_value(op.health).expect("invariant: health observation serializes"),
    );
    Ok(value)
}

#[derive(data_serialization::Serialize)]
#[serialization(rename_all = "snake_case")]
enum OperationPhase {
    Completed,
    Failed,
    InDoubt,
    Processing,
}


