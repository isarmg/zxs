//! Shared fields of the actual merge and tag mutation envelopes.
use crate::ApiError;
use json_codec::{Map, Value};
pub(super) fn string(value: Option<Value>, key: &str) -> Result<String, ApiError> {
    match value {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        _ => Err(ApiError::invalid(format!("missing string field {key}"))),
    }
}
pub(super) fn location(
    body: &mut Map<String, Value>,
) -> Result<Option<storage_contract::FileLocation>, ApiError> {
    Ok(match (body.remove("repository_id"), body.remove("path")) {
        (None, None) => None,
        (Some(repo), Some(path)) => Some(storage_contract::FileLocation {
            repository_id: string(Some(repo), "repository_id")?,
            path: string(Some(path), "path")?,
        }),
        (Some(_), None) | (None, Some(_)) => {
            return Err(ApiError::invalid(
                "repository_id and path must be supplied together",
            ));
        }
    })
}
pub(super) fn operation(value: Option<Value>) -> Result<storage_contract::OperationId, ApiError> {
    string(value, "operation_id")?
        .parse::<storage_contract::OperationId>()
        .map_err(|source| {
            ApiError::new(
                http::StatusCode::BAD_REQUEST,
                source.code(),
                source.message().into_owned(),
            )
            .with_source(source)
        })
}
