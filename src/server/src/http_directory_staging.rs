//! Read durable staging and explicitly commit the reviewed directory tree.
use crate::{ApiError, AppState, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::Value;
use storage_contract::directory_staging::{DirectoryCommitBody, DirectoryStagingStatus};

pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((repository, branch)): ApiPath<(String, String)>,
) -> Result<Json<DirectoryStagingStatus>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.directory_staging(&identity, &repository, &branch))
        .await
        .map(Json)
}

pub(crate) async fn commit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((repository, branch)): ApiPath<(String, String)>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let body: DirectoryCommitBody = json_codec::from_value(crate::json_body(request).await?)
        .map_err(ApiError::invalid_source)?;
    storage_contract::validate_id(&body.expected_tree_id, "dtree")
        .map_err(ApiError::invalid_source)?;
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .commit_directory_changes(&identity, &repository, &branch, body)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}
