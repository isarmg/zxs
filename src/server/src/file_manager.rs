//! Path-oriented file manager routes over controlled disk namespaces.
use crate::*;
pub(crate) fn routes() -> Router<AppState> {
    let routes = Router::new()
        .route(
            "/api/file-manager/directory-snapshot-source",
            get(http_directory_snapshot_source::read),
        )
        .route("/api/file-manager/disks", get(disks))
        .route(
            "/api/file-manager/detached-history",
            get(http_detached_history::read),
        )
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
            "/api/file-manager/disks/{slot}/file-relocation",
            post(http_file_relocation::submit),
        )
        .route(
            "/api/file-manager/disks/{slot}/object-move",
            post(http_object_move::submit),
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
            "/api/file-manager/disks/{slot}/directory-branches/from-history",
            post(http_disk_directory::create_from_history),
        )
        .route(
            "/api/file-manager/files/{file}/node-retirement/preview",
            post(http_node_retirement::preview),
        )
        .route(
            "/api/file-manager/files/{file}/node-retirement",
            post(http_node_retirement::submit),
        )
        .route(
            "/api/file-manager/files/{file}/node-retirement/resume",
            post(http_node_retirement::resume),
        )
        .route(
            "/api/file-manager/files/{file}/shared-history",
            get(http_shared_history::read),
        )
        .route(
            "/api/file-manager/files/{file}/content-matches",
            get(http_content_matches::read),
        )
        .route(
            "/api/file-manager/files/{file}/history",
            get(history_window_http::nodes),
        )
        .route(
            "/api/file-manager/files/{file}/history/lifecycle",
            get(http_file_lifecycle::read),
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
            "/api/file-manager/files/{file}/default-branch",
            http_routing::routing::put(http_file_selection::set_default),
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
            "/api/file-manager/files/{file}/merge-history/source",
            get(http_file_history_source::read),
        )
        .route(
            "/api/file-manager/files/{file}/merge-history",
            post(http_file_history_merge::submit),
        )
        .route(
            "/api/file-manager/files/{file}/edit-mode",
            get(http_container_drafts::read).post(http_container_drafts::set_mode),
        )
        .route(
            "/api/file-manager/files/{file}/prepare-draft-commit",
            post(crate::http_container_drafts::prepare_commit),
        )
        .route(
            "/api/file-manager/files/{file}/current-content",
            get(http_container_drafts::content),
        )
        .route(
            "/api/file-manager/files/{file}/tags",
            get(http_file_tags::read).post(http_file_tags::submit),
        )
        .route(
            "/api/repositories/{repository}/placement-configuration",
            get(http_placement_configuration::read).put(http_placement_configuration::configure),
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
        );
    #[cfg(feature = "core")]
    {
        routes
            .route(
                "/api/repositories/{repository}/recursive-backup-configuration",
                get(http_backup_configuration::read_recursive)
                    .put(http_backup_configuration::configure_recursive),
            )
            .route(
                "/api/repositories/{repository}/recursive-backup-tree-configuration",
                http_routing::routing::put(http_backup_configuration::configure_recursive_tree),
            )
    }
    #[cfg(not(feature = "core"))]
    {
        routes
    }
}
async fn disks(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_diagnostics::FileManagerDisksResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            let topology = backend.topology_snapshot(&identity)?;
            let disks = backend.disk_status(&identity)?;
            backend_diagnostics::FileManagerDisksResponse::from_snapshot(disks, topology)
        })
        .await
        .map(Json)
}
async fn history(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<backend_activity::ActivityHistoryResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .activity_history(&identity)
                .and_then(backend_activity::ActivityHistoryResponse::try_from)
        })
        .await
        .map(Json)
}
