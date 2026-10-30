//! Decode share updates once at HTTP ingress; core owns the permanent receipt.
use crate::http_boundary::{ApiPath, ApiQuery};
use crate::{ApiError, AppState, auth::Identity, backend_smb_binding::SmbBindingResponse};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use std::collections::BTreeMap;

struct UpdateBody {
    branch: String,
    enabled: bool,
    expected_history_generation: u64,
    operation: storage_contract::OperationId,
}
impl UpdateBody {
    fn parse(body: &json_codec::Map<String, json_codec::Value>) -> Result<Self, ApiError> {
        if let Some(key) = body.keys().find(|key| {
            ![
                "repository_id",
                "branch_id",
                "enabled",
                "expected_history_generation",
                "operation_id",
            ]
            .contains(&key.as_str())
        }) {
            return Err(ApiError::invalid(format!("unknown field {key}")));
        }
        let field = |key: &str| {
            body.get(key)
                .and_then(json_codec::Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ApiError::invalid(format!("missing string field {key}")))
        };
        let enabled = body
            .get("enabled")
            .and_then(json_codec::Value::as_bool)
            .ok_or_else(|| ApiError::invalid("enabled must be a boolean"))?;
        let expected_history_generation =
            storage_contract::parse_u64_decimal(field("expected_history_generation")?)
                .map_err(ApiError::invalid_source)?;
        let branch = field("branch_id")?.to_owned();
        let operation = field("operation_id")?
            .parse::<storage_contract::OperationId>()
            .map_err(|source| {
                ApiError::new(
                    http::StatusCode::BAD_REQUEST,
                    source.code(),
                    source.message().into_owned(),
                )
                .with_source(source)
            })?;
        Ok(Self {
            branch,
            enabled,
            expected_history_generation,
            operation,
        })
    }
}
pub(crate) async fn smb_binding(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<SmbBindingResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend
                .smb_binding_status(&identity, &repository)
                .map(SmbBindingResponse::from)
        })
        .await
        .map(Json)
}
pub(crate) async fn update_smb_binding(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository): ApiPath<String>,
    request: Request,
) -> Result<Json<json_codec::Value>, ApiError> {
    let body = crate::with_id(
        crate::json_body(request).await?,
        "repository_id",
        repository.clone(),
    )?;
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            let body = UpdateBody::parse(
                body.as_object()
                    .expect("invariant: with_id binds an object body"),
            )?;
            backend
                .update_smb_binding(
                    &identity,
                    crate::UpdateSmbBinding {
                        repository: &repository,
                        branch: &body.branch,
                        enabled: body.enabled,
                        expected_history_generation: body.expected_history_generation,
                        operation: body.operation,
                    },
                )
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_smb_binding.rs"]
mod tests;
