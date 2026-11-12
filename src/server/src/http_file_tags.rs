//! Current container tags, independent of content nodes and branch selection.
use crate::{
    ApiError, AppState, ReplaceContainerTags,
    http_boundary::{ApiJson, ApiPath, ApiQuery, Authenticated, JsonBody},
};
use data_serialization::{Deserialize, Deserializer};
use http_routing::{Json, extract::State};
use storage_contract::{
    FileId, Generation, OperationId, RepositoryId,
    file_container_tags::{Context, ReadRequest, ReplaceCommand, Snapshot, Tag, TagSet, TagsError},
};

/// Preserve the public identifier error after body/query and job admission.
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
    repository_id: RepositoryId,
    path: name_policy::PortablePath,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct WireBody {
    repository_id: RepositoryId,
    path: name_policy::PortablePath,
    expected_generation: Generation,
    tags: Vec<String>,
    operation_id: String,
}

pub(crate) struct Body {
    context: Context,
    expected_generation: Generation,
    tags: TagSet,
    operation_id: OperationId,
}

impl JsonBody for Body {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        let wire: WireBody = json_codec::from_value(value).map_err(ApiError::invalid_source)?;
        let operation_id = wire.operation_id.parse().map_err(ApiError::contract_input)?;
        // The HTTP body has already passed its byte budget. Keep the original
        // item count and duplicate policy rather than silently collecting a set.
        let tags = wire
            .tags
            .into_iter()
            .map(Tag::parse)
            .collect::<Result<Vec<_>, _>>()
            .and_then(TagSet::new)
            .map_err(|source| ApiError::contract_input(TagsError::into_error(source)))?;
        Ok(Self {
            context: Context {
                repository_id: wire.repository_id,
                path: wire.path,
            },
            expected_generation: wire.expected_generation,
            tags,
            operation_id,
        })
    }
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FilePath>,
    ApiQuery(query): ApiQuery<Query>,
) -> Result<Json<Snapshot>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            let request = ReadRequest {
                file_id: file.0.map_err(ApiError::contract_input)?,
                context: Context {
                    repository_id: query.repository_id,
                    path: query.path,
                },
            };
            backend.file_container_tags(&identity, request)
        })
        .await
        .map(Json)
}

pub(crate) async fn submit(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FilePath>,
    ApiJson(body): ApiJson<Body>,
) -> Result<Json<json_codec::Value>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            let request = ReplaceContainerTags {
                command: ReplaceCommand {
                    file_id: file.0.map_err(ApiError::contract_input)?,
                    context: body.context,
                    expected_generation: body.expected_generation,
                    tags: body.tags,
                },
                operation_id: body.operation_id,
            };
            backend
                .replace_container_tags(&identity, request)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_file_tags.rs"]
mod tests;
