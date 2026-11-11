use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
};
use data_serialization::Deserialize;
use http_routing::{
    Extension,
    extract::{Request, State},
    response::Response,
};
use storage_contract::file_deletion_review::{MAX_RESPONSE_BYTES, UndoBody};
use storage_contract::{BranchId, FileDeletionId, FileId, OperationId};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ExactQuery {
    deletion_node_id: FileDeletionId,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct BranchQuery {
    deletion_node_id: FileDeletionId,
    after: Option<BranchId>,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ParentQuery {
    deletion_node_id: FileDeletionId,
    offset: Option<String>,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct EmptyQuery {}

pub(crate) async fn summary(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<FileId>,
    ApiQuery(query): ApiQuery<ExactQuery>,
) -> Result<Response, ApiError> {
    crate::bounded_history_http::read(state, move |backend, _| {
        crate::history_window_encode::encode_bounded(
            &backend.file_deletion_summary(&identity, file, query.deletion_node_id)?,
            MAX_RESPONSE_BYTES,
        )
    })
    .await
}

pub(crate) async fn branches(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<FileId>,
    ApiQuery(query): ApiQuery<BranchQuery>,
) -> Result<Response, ApiError> {
    crate::bounded_history_http::read(state, move |backend, _| {
        crate::history_window_encode::encode_bounded(
            &backend.file_deletion_branches(
                &identity,
                file,
                query.deletion_node_id,
                query.after,
            )?,
            MAX_RESPONSE_BYTES,
        )
    })
    .await
}

pub(crate) async fn parents(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<FileId>,
    ApiQuery(query): ApiQuery<ParentQuery>,
) -> Result<Response, ApiError> {
    let offset = match query.offset {
        Some(offset) => {
            storage_contract::parse_u64_decimal(&offset).map_err(ApiError::contract_input)?
        }
        None => 0,
    };
    crate::bounded_history_http::read(state, move |backend, _| {
        crate::history_window_encode::encode_bounded(
            &backend.file_deletion_parents(&identity, file, query.deletion_node_id, offset)?,
            MAX_RESPONSE_BYTES,
        )
    })
    .await
}

pub(crate) async fn undo(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<FileId>,
    ApiQuery(_query): ApiQuery<EmptyQuery>,
    request: Request,
) -> Result<Response, ApiError> {
    let body: UndoBody = super::body(request).await?;
    let file_id = storage_contract::history_window::HistoryFileId::parse(&file.to_string())
        .expect("invariant: typed file ID has the history file ID representation");
    // The queued job owns the mutation through disconnect, as ordinary undo does.
    // The retained operation ID remains the reconciliation authority afterward.
    crate::bounded_history_http::read(state, move |backend, _| {
        let result = backend.restore_deleted_file(
            &identity,
            crate::RestoreDeletedFile {
                file_id,
                deletion_node_id: body.deletion_node_id.to_string(),
                operation_id: body.operation_id,
            },
        )?;
        crate::history_window_encode::encode_bounded(
            &super::projection::undo(result.into())?,
            MAX_RESPONSE_BYTES,
        )
    })
    .await
}

pub(crate) async fn outcome(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(operation): ApiPath<OperationId>,
    ApiQuery(_query): ApiQuery<EmptyQuery>,
) -> Result<Response, ApiError> {
    crate::bounded_history_http::read(state, move |backend, _| {
        crate::history_window_encode::encode_bounded(
            &super::projection::undo(backend.operation_outcome(&identity, operation)?)?,
            MAX_RESPONSE_BYTES,
        )
    })
    .await
}
