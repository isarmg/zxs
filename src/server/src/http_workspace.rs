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
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend
                .create_workspace(&identity, command)
                .map(WorkspaceResponse::from)
        })
        .await
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
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend.abort_workspace(&identity, &id)?;
            Ok(Json(AbortedWorkspace {
                workspace_id: id,
                state: storage_contract::WorkspaceState::Aborted,
            }))
        })
        .await
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
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend
                .workspace(&identity, &id)
                .map(WorkspaceResponse::from)
        })
        .await
        .map(Json)
}

pub(crate) fn public_commit(
    workspace: &str,
    body: json_codec::Value,
) -> Result<storage_contract::PublicCommitEnvelope, ApiError> {
    storage_contract::PublicCommitEnvelope::parse(workspace, body).map_err(|error| match error {
        storage_contract::PublicCommitParseError::Shape(source) => ApiError::invalid_source(source),
        storage_contract::PublicCommitParseError::Fence(source) => ApiError::invalid_source(source),
        storage_contract::PublicCommitParseError::Field(source) => ApiError::new(
            ::http::StatusCode::BAD_REQUEST,
            source.code(),
            source.message(),
        )
        .with_source(source),
    })
}

pub(crate) async fn commit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    request: Request,
) -> Result<Json<json_codec::Value>, ApiError> {
    let body = crate::with_id(crate::json_body(request).await?, "workspace_id", id.clone())?;
    let spool = state.spool_budget.clone();
    let claims = state.upload_claims.clone();
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            let command = public_commit(&id, body)?;
            let result = backend.commit_workspace_public(&identity, command)?;
            retire_committed_copy(backend, &identity, &id, None, &spool, &claims);
            crate::operation_wire::operation_wire(result.into())
        })
        .await
        .map(Json)
}

pub(crate) fn retire_committed_copy(
    backend: &dyn crate::Backend,
    identity: &Identity,
    workspace: &str,
    item: Option<&str>,
    spool: &crate::http_spool::HttpSpool,
    claims: &std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
) {
    let Some(budget) = spool.retirement_budget() else {
        return;
    };
    if !budget.has_candidates(workspace, item) {
        return;
    }
    let result = backend
        .http_spool_workspace(identity, workspace)
        .and_then(|proof| match proof {
            Some(workspace) => match item {
                Some(item) => budget.retire_item(&workspace, item, claims),
                None => budget.retire_workspace(&workspace, claims),
            },
            None => {
                budget.resolve_workspace_failure(workspace, item);
                Ok(())
            }
        });
    if let Err(error) = result {
        // The publication result is already authoritative. This local cache
        // failure belongs to health/recovery, never to the commit outcome.
        budget.note_workspace_failure(workspace, item, &error);
    }
}

pub(crate) async fn seal(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    request: Request,
) -> Result<Json<storage_contract::SealedWorkspace>, ApiError> {
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
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| backend.seal_workspace(&identity, &id))
        .await
        .map(Json)
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct NamespaceBody {
    #[serialization(deserialize_with = "nonempty")]
    workspace_id: String,
    kind: storage_contract::NamespaceOperationKind,
    #[serialization(deserialize_with = "nonempty")]
    path: String,
    #[serialization(default, deserialize_with = "namespace_destination")]
    destination: Option<String>,
}
fn namespace_destination<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    match json_codec::Value::deserialize(deserializer)? {
        json_codec::Value::Null => Ok(None),
        json_codec::Value::String(value) => Ok(Some(value)),
        _ => Err(D::Error::custom("destination must be a string")),
    }
}
pub(crate) fn namespace_command(
    value: json_codec::Value,
) -> Result<crate::StageNamespace, ApiError> {
    let body: NamespaceBody = json_codec::from_value(value).map_err(ApiError::invalid_source)?;
    let change = storage_contract::NamespaceOperation::parse(
        body.kind,
        &body.path,
        body.destination.as_deref(),
    )
    .map_err(|source| {
        ApiError::new(
            ::http::StatusCode::BAD_REQUEST,
            source.code(),
            source.message(),
        )
        .with_source(source)
    })?;
    Ok(crate::StageNamespace {
        workspace_id: body.workspace_id,
        change,
    })
}
pub(crate) async fn namespace(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    request: Request,
) -> Result<Json<WorkspaceResponse>, ApiError> {
    let body = crate::with_id(crate::json_body(request).await?, "workspace_id", id)?;
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            let command = namespace_command(body)?;
            backend
                .stage_namespace(&identity, command)
                .map(WorkspaceResponse::from)
        })
        .await
        .map(Json)
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct WriteSessionBody {
    #[serialization(deserialize_with = "nonempty")]
    branch_id: String,
}
pub(crate) async fn write_session(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    request: Request,
) -> Result<Json<crate::backend_workspace::WriteSessionResponse>, ApiError> {
    let body = crate::with_id(crate::json_body(request).await?, "branch_id", id)?;
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            let body: WriteSessionBody =
                json_codec::from_value(body).map_err(ApiError::invalid_source)?;
            backend
                .create_write_session(&identity, &body.branch_id)
                .map(crate::backend_workspace::WriteSessionResponse::from)
        })
        .await
        .map(Json)
}
