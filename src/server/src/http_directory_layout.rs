//! Explicit derived physical layout repair, not a business operation or commit.
use crate::{ApiError, AppState, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use storage_contract::directory_git::{DirectoryLayoutReconcileBody, DirectoryLayoutResult};

pub(crate) async fn reconcile(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository): ApiPath<String>,
    request: Request,
) -> Result<Json<DirectoryLayoutResult>, ApiError> {
    storage_contract::validate_id(&repository, "repo").map_err(ApiError::invalid_source)?;
    let body: DirectoryLayoutReconcileBody =
        json_codec::from_value(crate::json_body(request).await?)
            .map_err(ApiError::invalid_source)?;
    storage_contract::validate_id(&body.expected_namespace_head, "commit")
        .map_err(ApiError::invalid_source)?;
    let job =
        crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?;
    // The blocking job retains its permit after the caller disconnects. There
    // is no operation_id or durable job lookup for this current-state repair.
    job.run(move |backend| backend.reconcile_directory_layout(&identity, &repository, body))
        .await
        .map(Json)
}
