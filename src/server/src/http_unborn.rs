//! Native first-history commit through the existing authenticated JSON boundary.
use crate::{ApiError, AppState, CommitUnbornFile, UnbornFileSnapshot};
use crate::http_boundary::{ApiJson, ApiPath, ApiQuery, Authenticated, JsonBody};
use crate::http_file_draft::LocationQuery;
use http_routing::{Json, extract::State};
use storage_contract::FileId;

pub(crate) struct CommitBody(CommitUnbornFile);
impl JsonBody for CommitBody {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        if !value.is_object()
            || !value.get("target").is_some_and(json_codec::Value::is_object)
            || !value.get("expected").is_some_and(json_codec::Value::is_object)
            || !value.get("expected").and_then(|item| item.get("content"))
                .is_some_and(json_codec::Value::is_object)
        {
            return Err(ApiError::invalid("first commit target and expectation must be objects"));
        }
        json_codec::from_value(value).map(Self).map_err(ApiError::invalid_source)
    }
}

pub(crate) async fn status(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FileId>, ApiQuery(query): ApiQuery<LocationQuery>,
) -> Result<Json<UnbornFileSnapshot>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.unborn_file_working(&identity, query.target(file)))
        .await.map(Json)
}

pub(crate) async fn commit(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FileId>, ApiJson(body): ApiJson<CommitBody>,
) -> Result<Json<json_codec::Value>, ApiError> {
    if body.0.target.file != file {
        return Err(ApiError::invalid("first commit file differs from route"));
    }
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.commit_unborn_file(&identity, body.0)
            .and_then(|result| crate::operation_wire::operation_wire(result.into())))
        .await.map(Json)
}
