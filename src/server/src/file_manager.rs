//! Path-oriented file manager routes over controlled disk namespaces.
use crate::*;
macro_rules! list_handler {
    ($name:ident, $action:expr) => {
        async fn $name(
            State(state): State<AppState>,
            http_routing::Extension(identity): http_routing::Extension<Identity>,
            ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
        ) -> Result<Json<Value>, ApiError> {
            call(state, identity, $action, json!({"query": query})).await
        }
    };
}
macro_rules! read_handler {
    ($name:ident, $action:expr, $key:literal) => {
        async fn $name(
            State(state): State<AppState>,
            http_routing::Extension(identity): http_routing::Extension<Identity>,
            ApiPath(id): ApiPath<String>,
            ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
        ) -> Result<Json<Value>, ApiError> {
            let mut input = json!({"query": query});
            input[$key] = id.into();
            call(state, identity, $action, input).await
        }
    };
}
macro_rules! id_post_handler {
    ($name:ident,$action:expr,$key:literal) => {
        async fn $name(
            State(state): State<AppState>,
            http_routing::Extension(identity): http_routing::Extension<Identity>,
            ApiPath(id): ApiPath<String>,
            request: Request,
        ) -> Result<Json<Value>, ApiError> {
            call(
                state,
                identity,
                $action,
                with_id(json_body(request).await?, $key, id)?,
            )
            .await
        }
    };
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/file-manager/disks", get(disks))
        .route("/api/file-manager/disks/{slot}/entries", get(entries))
        .route(
            "/api/file-manager/disks/{slot}/write-sessions",
            post(write_session),
        )
        .route("/api/file-manager/disks/{slot}/namespace", post(namespace))
        .route(
            "/api/file-manager/disks/{slot}/namespace-batch",
            post(namespace_batch),
        )
        .route(
            "/api/file-manager/disks/{slot}/directory-branches",
            post(http_disk_directory::create),
        )
        .route(
            "/api/file-manager/disks/{slot}/directory-select",
            post(http_disk_directory::select),
        )
        .route("/api/file-manager/files/{file}/history", get(file_history))
        .route("/api/file-manager/files/{file}/branches", post(branch))
        .route(
            "/api/file-manager/files/{file}/branches/{branch}",
            patch(rename_file_branch),
        )
        .route("/api/file-manager/files/{file}/select", post(select))
        .route("/api/file-manager/files/{file}/merge", post(merge))
        .route("/api/file-manager/files/{file}/tags", post(tags))
        .route(
            "/api/repositories/{repository}/backup-configuration",
            get(backup_configuration).put(configure_backups),
        )
        .route(
            "/api/file-manager/files/{file}/nodes/{node}",
            patch(node_note),
        )
        .route("/api/file-manager/history", get(history))
        .route(
            "/api/file-manager/files/{file}/metadata",
            post(file_metadata),
        )
        .route(
            "/api/file-manager/versions/{version}/metadata",
            post(version_metadata),
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
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .disk_status(&identity)
            .map(backend_diagnostics::FileManagerDisksResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
list_handler!(history, crate::BackendAction::FileManagerHistory);
read_handler!(entries, crate::BackendAction::FileManagerEntries, "slot_id");
read_handler!(
    file_history,
    crate::BackendAction::FileManagerFileHistory,
    "file_id"
);
id_post_handler!(
    write_session,
    crate::BackendAction::FileManagerWriteSession,
    "slot_id"
);
id_post_handler!(
    namespace,
    crate::BackendAction::FileManagerNamespace,
    "slot_id"
);
id_post_handler!(
    namespace_batch,
    crate::BackendAction::FileManagerNamespaceBatch,
    "slot_id"
);
id_post_handler!(branch, crate::BackendAction::FileManagerBranch, "file_id");
async fn rename_file_branch(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((file, branch)): ApiPath<(String, String)>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let input = with_id(json_body(request).await?, "file_id", file)?;
    let input = with_id(input, "branch_id", branch)?;
    call(
        state,
        identity,
        crate::BackendAction::FileManagerRenameBranch,
        input,
    )
    .await
}
async fn node_note(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((file, node)): ApiPath<(String, String)>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let input = with_id(json_body(request).await?, "file_id", file)?;
    let input = with_id(input, "node_id", node)?;
    call(
        state,
        identity,
        crate::BackendAction::FileManagerNodeNote,
        input,
    )
    .await
}
id_post_handler!(select, crate::BackendAction::FileManagerSelect, "file_id");
id_post_handler!(merge, crate::BackendAction::FileManagerMerge, "file_id");
id_post_handler!(tags, crate::BackendAction::FileManagerTags, "file_id");
read_handler!(
    backup_configuration,
    crate::BackendAction::FileManagerBackupConfiguration,
    "repository_id"
);
id_post_handler!(
    configure_backups,
    crate::BackendAction::FileManagerConfigureBackups,
    "repository_id"
);
id_post_handler!(
    file_metadata,
    crate::BackendAction::FileManagerFileMetadata,
    "file_id"
);
id_post_handler!(
    version_metadata,
    crate::BackendAction::FileManagerVersionMetadata,
    "version_id"
);
