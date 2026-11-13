//! Explicit container policy and current-body reads, never historical node mutation.
use crate::{
    ApiError, AppState, ReadContainerEdit, SetContainerCommitMode,
    http_boundary::{ApiJson, ApiPath, ApiQuery, Authenticated, JsonBody},
};
use data_serialization::{Deserialize, Deserializer};
use http_routing::{Json, extract::State, response::Response};
use storage_contract::{FileId, Generation, OperationId, RepositoryId};
use storage_contract::container_edit::{CommitMode, ContainerEdit};

pub(crate) struct FilePath(storage_contract::Result<FileId>);
impl<'de> Deserialize<'de> for FilePath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(Self(text.parse()))
    }
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Query {
    repository: RepositoryId,
    path: name_policy::PortablePath,
}
impl Query {
    fn request(self, file: FilePath) -> Result<ReadContainerEdit, ApiError> {
        Ok(ReadContainerEdit {
            file: file.0.map_err(ApiError::contract_input)?,
            repository: self.repository,
            path: self.path,
        })
    }
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct ModeBody {
    repository: RepositoryId,
    path: name_policy::PortablePath,
    expected_generation: Generation,
    mode: CommitMode,
    operation_id: OperationId,
}
pub(crate) struct Body(ModeBody);
impl JsonBody for Body {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        json_codec::from_value(value).map(Self).map_err(ApiError::invalid_source)
    }
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FilePath>,
    ApiQuery(query): ApiQuery<Query>,
) -> Result<Json<ContainerEdit>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.read_container_edit(&identity, query.request(file)?))
        .await.map(Json)
}

pub(crate) async fn set_mode(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FilePath>,
    ApiJson(Body(body)): ApiJson<Body>,
) -> Result<Json<json_codec::Value>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            let request = SetContainerCommitMode {
                context: ReadContainerEdit {
                    file: file.0.map_err(ApiError::contract_input)?,
                    repository: body.repository,
                    path: body.path,
                },
                expected_generation: body.expected_generation,
                mode: body.mode,
                operation_id: body.operation_id,
            };
            backend.set_container_commit_mode(&identity, request)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        }).await.map(Json)
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct PrepareBody {
    repository: RepositoryId,
    path: name_policy::PortablePath,
    expected_generation: Generation,
    draft_operation: OperationId,
    operation_id: OperationId,
}
pub(crate) struct DraftCommitBody(PrepareBody);
impl JsonBody for DraftCommitBody {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        json_codec::from_value(value).map(Self).map_err(ApiError::invalid_source)
    }
}

pub(crate) async fn prepare_commit(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FilePath>,
    ApiJson(DraftCommitBody(body)): ApiJson<DraftCommitBody>,
) -> Result<Json<storage_contract::SealedWorkspace>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.prepare_container_draft_commit(&identity,
            crate::PrepareContainerDraftCommit {
                context: ReadContainerEdit {
                    file: file.0.map_err(ApiError::contract_input)?,
                    repository: body.repository, path: body.path,
                },
                expected_generation: body.expected_generation,
                draft_operation: body.draft_operation, operation_id: body.operation_id,
            })).await.map(Json)
}

pub(crate) async fn content(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FilePath>,
    ApiQuery(query): ApiQuery<Query>,
    method: http::Method,
    headers: http::HeaderMap,
) -> Result<Response, ApiError> {
    crate::transfer::current_container_content(state, identity, query.request(file)?, method, headers)
        .await
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_container_drafts.rs"]
mod tests;
