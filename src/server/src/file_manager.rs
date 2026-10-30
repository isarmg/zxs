//! Path-oriented file manager routes over controlled disk namespaces.
use crate::*;
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/file-manager/disks", get(disks))
        .route(
            "/api/file-manager/disks/{slot}/entries",
            get(http_listing::read),
        )
        .route(
            "/api/file-manager/disks/{slot}/write-sessions",
            post(http_disk_write_session::submit),
        )
        .route(
            "/api/file-manager/disks/{slot}/namespace",
            post(http_disk_mkdir::submit),
        )
        .route(
            "/api/file-manager/disks/{slot}/namespace-batch",
            post(http_namespace_batch::submit),
        )
        .route(
            "/api/file-manager/disks/{slot}/directory-branches",
            post(http_disk_directory::create),
        )
        .route(
            "/api/file-manager/disks/{slot}/directory-select",
            post(http_disk_directory::select),
        )
        .route(
            "/api/file-manager/files/{file}/history",
            get(history_window_http::nodes),
        )
        .route(
            "/api/file-manager/files/{file}/history/branches",
            get(history_window_http::branches),
        )
        .route(
            "/api/file-manager/files/{file}/history/merges",
            get(history_window_http::merges),
        )
        .route(
            "/api/file-manager/files/{file}/history/nodes/{node}/parents",
            get(history_window_http::node_parents),
        )
        .route(
            "/api/file-manager/files/{file}/history/nodes/{node}/fields/{field}",
            get(history_window_http::node_field),
        )
        .route(
            "/api/file-manager/files/{file}/history/merges/{merge}/inputs",
            get(history_window_http::merge_inputs),
        )
        .route(
            "/api/file-manager/files/{file}/history/merges/{merge}/parents",
            get(history_window_http::merge_parents),
        )
        .route(
            "/api/file-manager/files/{file}/branches",
            post(http_create_file_branch::create),
        )
        .route(
            "/api/file-manager/files/{file}/branches/{branch}",
            patch(http_rename_file_branch::rename),
        )
        .route(
            "/api/file-manager/files/{file}/select",
            post(http_file_selection::select),
        )
        .route(
            "/api/file-manager/files/{file}/merge",
            post(http_file_merge::submit),
        )
        .route(
            "/api/file-manager/files/{file}/tags",
            post(http_file_tags::submit),
        )
        .route(
            "/api/repositories/{repository}/backup-configuration",
            get(http_backup_configuration::read).put(http_backup_configuration::configure),
        )
        .route(
            "/api/file-manager/files/{file}/nodes/{node}",
            patch(http_note::node),
        )
        .route("/api/file-manager/history", get(history))
        .route(
            "/api/file-manager/files/{file}/metadata",
            post(http_note::file),
        )
        .route(
            "/api/file-manager/versions/{version}/metadata",
            post(http_note::version),
        )
}
async fn disks(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_diagnostics::FileManagerDisksResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend
                .disk_status(&identity)
                .map(backend_diagnostics::FileManagerDisksResponse::from)
        })
        .await
        .map(Json)
}
async fn history(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_activity::ActivityHistoryResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend
                .activity_history(&identity)
                .and_then(backend_activity::ActivityHistoryResponse::try_from)
        })
        .await
        .map(Json)
}
