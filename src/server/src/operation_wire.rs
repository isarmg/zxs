use crate::ApiError;
use json_codec::{Value, json};

#[derive(Debug, error_definition::Error)]
pub(crate) enum ContainerTagsWireError {
    #[error("container tag receipt is invalid")]
    Decode(#[source] json_codec::Error),
    #[error("container tag receipt differs from its request")]
    Validation(#[source] storage_contract::file_container_tags::TagsError),
    #[error("container tag outcome cannot be encoded")]
    Encode(#[source] json_codec::Error),
}

pub(crate) fn operation_wire(
    mut op: storage_contract::OperationOutcome,
) -> Result<Value, ApiError> {
    if op.kind == storage_contract::file_container_tags::KIND
        && matches!(
            op.commit_state,
            storage_contract::CommitState::Published | storage_contract::CommitState::Acked
        )
    {
        op.result = container_tags_result(op.result)?;
    }
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
                .as_object_mut()
                .and_then(|result| result.remove("version_protection"))
                .filter(Value::is_array)
                .ok_or_else(|| {
                    ApiError::new(
                        ::http::StatusCode::INTERNAL_SERVER_ERROR,
                        storage_contract::ErrorCode::StateInvalid,
                        "frozen publication protection evidence is missing",
                    )
                })?;
            Value::Object(json_codec::Map::from_iter(
                [
                    (
                        "outcome",
                        if op.protection_at_publish
                            == storage_contract::ProtectionAtPublish::TargetMet
                        {
                            "COMMITTED_AND_PROTECTED"
                        } else {
                            "COMMITTED_DEGRADED"
                        }
                        .into(),
                    ),
                    ("commit_id", op.commit_id.into()),
                    ("branch_id", op.branch_id.into()),
                    (
                        "branch_generation",
                        field(&op.result, "branch_generation")?.into(),
                    ),
                    ("publication_sequence", op.sequence.to_string().into()),
                    (
                        "membership_epoch",
                        field(&op.result, "membership_epoch")?.into(),
                    ),
                    ("protection_at_publish", json!(op.protection_at_publish)),
                    ("versions", versions),
                ]
                .map(|(key, value)| (key.into(), value)),
            ))
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
        Ok(Value::Object(json_codec::Map::from_iter(
            [
                ("operation_id", json!(op.operation_id)),
                ("operation_kind", "content_save".into()),
                ("request_digest", op.request_digest.into()),
                ("state", json!(state)),
                ("commit_state", json!(op.commit_state)),
                ("result", result),
                ("error", error),
            ]
            .map(|(key, value)| (key.into(), value)),
        )))
    } else {
        Ok(Value::Object(json_codec::Map::from_iter(
            [
                ("operation_id", json!(op.operation_id)),
                ("operation_kind", op.kind.into()),
                ("request_digest", op.request_digest.into()),
                (
                    "state",
                    json!(if matches!(
                        op.commit_state,
                        storage_contract::CommitState::Published
                            | storage_contract::CommitState::Acked
                    ) {
                        OperationPhase::Completed
                    } else {
                        OperationPhase::InDoubt
                    }),
                ),
                ("commit_state", json!(op.commit_state)),
                ("protection_at_publish", json!(op.protection_at_publish)),
                ("publication_sequence", op.sequence.to_string().into()),
                ("result", op.result),
                ("error", Value::Null),
            ]
            .map(|(key, value)| (key.into(), value)),
        )))
    }
}

/// The public tag outcome is typed. Transaction-only publication fields stay
/// in the immutable receipt and are not accepted as arbitrary DTO extensions.
fn container_tags_result(mut value: Value) -> Result<Value, ApiError> {
    if let Some(fields) = value.as_object_mut() {
        for name in ["timestamp", "membership_epoch", "version_protection"] {
            fields.remove(name);
        }
    }
    let result: storage_contract::file_container_tags::Replacement =
        json_codec::from_value(value).map_err(|source| {
            ApiError::container_tags_wire(ContainerTagsWireError::Decode(source))
        })?;
    result.validate(&result.request).map_err(|source| {
        ApiError::container_tags_wire(ContainerTagsWireError::Validation(source))
    })?;
    json_codec::to_value(result).map_err(|source| {
        ApiError::container_tags_wire(ContainerTagsWireError::Encode(source))
    })
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::invalid(format!("missing string field {key}")))
}

pub(crate) fn operation_observation_wire(
    op: crate::OperationObservation,
) -> Result<Value, ApiError> {
    let mut value = operation_wire(op.outcome)?;
    value
        .as_object_mut()
        .expect("invariant: operation projection is an object")
        .insert(
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

#[cfg(test)]
#[path = "../../../test/server/unit/container_tags_wire.rs"]
mod container_tags_tests;
