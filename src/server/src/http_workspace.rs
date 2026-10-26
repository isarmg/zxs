//! Strict HTTP parsing and final wire projection for typed workspace operations.
use crate::{
    ApiError, AppState, CreateWorkspace,
    auth::Identity,
    backend_workspace::{AbortedWorkspace, WorkspaceResponse},
    http_boundary::{ApiPath, ApiQuery},
};
use data_serialization::{Deserialize, Deserializer, de::Error};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct CreateBody {
    #[serialization(deserialize_with = "nonempty")]
    branch_id: String,
    #[serialization(default, deserialize_with = "supplied_identity")]
    source_commit_id: Option<String>,
}
fn nonempty<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(D::Error::custom("identity must be a nonempty string"));
    }
    Ok(value)
}
fn supplied_identity<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    nonempty(deserializer).map(Some)
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct AbortBody {
    #[serialization(default, deserialize_with = "supplied_identity")]
    workspace_id: Option<String>,
}

async fn body<T: data_serialization::de::DeserializeOwned>(
    request: Request,
) -> Result<T, ApiError> {
    let value = crate::json_body(request).await?;
    if !value.is_object() {
        return Err(ApiError::invalid("expected JSON object"));
    }
    json_codec::from_value(value).map_err(ApiError::invalid_source)
}

pub(crate) async fn workspace(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    request: Request,
) -> Result<Json<WorkspaceResponse>, ApiError> {
    let body: CreateBody = body(request).await?;
    let command = CreateWorkspace {
        branch_id: body.branch_id,
        source_commit_id: body.source_commit_id,
    };
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .create_workspace(&identity, command)
            .map(WorkspaceResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

pub(crate) async fn abort(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    request: Request,
) -> Result<Json<AbortedWorkspace>, ApiError> {
    let body: AbortBody = body(request).await?;
    if body
        .workspace_id
        .as_ref()
        .is_some_and(|body_id| body_id != &id)
    {
        return Err(ApiError::invalid("path and body identity disagree"));
    }
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state.backend.abort_workspace(&identity, &id)?;
        Ok(Json(AbortedWorkspace {
            workspace_id: id,
            state: storage_contract::WorkspaceState::Aborted,
        }))
    })
    .await
    .map_err(ApiError::join)?
}

pub(crate) async fn workspace_info(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<std::collections::BTreeMap<String, String>>,
) -> Result<Json<WorkspaceResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .workspace(&identity, &id)
            .map(WorkspaceResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
