//! Explicit irreversible detail action. The browser receipt is separate from cleanup.
use crate::{ApiError, AppState, auth::Identity, http_boundary::ApiPath};
use data_serialization::Deserialize;
use http_routing::{
    Extension,
    body::Body,
    extract::{Request, State},
    response::{IntoResponse, Response},
};
use storage_contract::{
    OperationId,
    history_window::{HistoryFileId, HistoryRoot, HistoryVersionId},
    node_retirement::{self, Selection},
};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct PreviewBody {
    history_root: HistoryRoot,
    version_ids: Vec<HistoryVersionId>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct SubmitBody {
    history_root: HistoryRoot,
    version_ids: Vec<HistoryVersionId>,
    operation_id: OperationId,
    confirmation: String,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct ResumeBody {
    operation_id: OperationId,
}
async fn body<T: data_serialization::de::DeserializeOwned>(
    request: Request,
) -> Result<T, ApiError> {
    let bytes =
        crate::http_boundary::read_body(request, node_retirement::MAX_REQUEST_BYTES).await?;
    storage_contract::decode_strict(&bytes).map_err(ApiError::invalid_source)
}
fn selection(
    file: &str,
    root: HistoryRoot,
    versions: Vec<HistoryVersionId>,
) -> Result<Selection, ApiError> {
    Selection::new(
        HistoryFileId::parse(file).map_err(ApiError::contract_input)?,
        root,
        versions,
    )
    .map_err(ApiError::contract_input)
}
pub(crate) async fn preview(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let body: PreviewBody = body(request).await?;
    let selected = selection(&file, body.history_root, body.version_ids)?;
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let preview = backend.preview_node_retirement(&identity, selected.clone(), cancellation)?;
        preview
            .validate(&selected)
            .map_err(ApiError::invalid_source)?;
        crate::history_window_encode::encode_bounded(&preview, node_retirement::MAX_RESPONSE_BYTES)
    })
    .await
}
pub(crate) async fn submit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let body: SubmitBody = body(request).await?;
    if body.confirmation != node_retirement::CONFIRMATION {
        return Err(ApiError::invalid(
            "explicit permanent-node-retirement confirmation is required",
        ));
    }
    let selected = selection(&file, body.history_root, body.version_ids)?;
    mutate(state, move |backend| {
        backend.retire_file_nodes(&identity, selected, body.operation_id)
    })
    .await
}
pub(crate) async fn resume(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let body: ResumeBody = body(request).await?;
    let file = HistoryFileId::parse(&file).map_err(ApiError::contract_input)?;
    mutate(state, move |backend| {
        backend.resume_node_retirement(&identity, file, body.operation_id)
    })
    .await
}
async fn mutate(
    state: AppState,
    work: impl FnOnce(&dyn crate::Backend) -> Result<node_retirement::Outcome, ApiError>
    + Send
    + 'static,
) -> Result<Response, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    // A disconnected HTTP waiter does not cancel an admitted destructive job.
    // Retrying always uses its original operation ID, never a fresh identity.
    let bytes = crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            crate::history_window_encode::encode_bounded(
                &work(backend)?,
                node_retirement::MAX_RESPONSE_BYTES,
            )
        })
        .await?;
    Ok((
        [(http::header::CONTENT_TYPE, "application/json")],
        Body::from(bytes),
    )
        .into_response())
}
