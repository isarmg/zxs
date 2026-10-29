//! Bounded history inspection with cooperative worker cancellation and socket leases.
use crate::http_file_history::HistoryQuery;
use crate::*;

async fn page(
    state: AppState,
    identity: Identity,
    request: ReadHistoryGraph,
    connection: Option<crate::http_shutdown::ResponseConnection>,
) -> Result<Response, ApiError> {
    let scan = state
        .graph_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| {
            ApiError::limit("history inspection capacity reached").with_source(source)
        })?;
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    let limits = state.config.http_transfer_limits.clone();
    let deadline = tokio::time::Instant::now() + limits.total_timeout;
    // A scan retains an old State; its work lease is shorter than socket drain.
    let work_deadline =
        deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(30));
    let control = GraphReadControl::until(work_deadline.into_std());
    let _wait_owner = control.wait_owner();
    let (encoded, permit) = tokio::time::timeout_at(
        work_deadline,
        crate::http_job::HttpBackendJob::new(state.backend, state.auth, (scan, permit))
            .run_with_permit(move |backend, (scan, permit)| {
                let captured = backend.history_graph(&identity, request, &control)?;
                drop(scan);
                control.checkpoint().map_err(|source| {
                    ApiError::new(
                        ::http::StatusCode::REQUEST_TIMEOUT,
                        source.code(),
                        source.message(),
                    )
                    .with_source(source)
                })?;
                Ok((captured.encode()?, permit))
            }),
    )
    .await
    .map_err(|source| {
        ApiError::new(
            ::http::StatusCode::REQUEST_TIMEOUT,
            storage_contract::ErrorCode::RequestTimeout,
            "history inspection deadline exceeded",
        )
        .with_source(source)
    })??;
    Ok(encoded.into_leased_response(permit, &limits, deadline, connection))
}

pub(crate) async fn graph(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(history): ApiPath<String>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        identity,
        ReadHistoryGraph::page(history, query)?,
        connection,
    )
    .await
}

pub(crate) async fn history_field(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((history, field)): ApiPath<(String, String)>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        identity,
        ReadHistoryGraph::history_field(history, field, query)?,
        connection,
    )
    .await
}

pub(crate) async fn commit_field(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((history, id, field)): ApiPath<(String, String, String)>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        identity,
        ReadHistoryGraph::commit_field(history, id, field, query)?,
        connection,
    )
    .await
}

pub(crate) async fn merge_link_field(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((history, id, field)): ApiPath<(String, String, String)>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        identity,
        ReadHistoryGraph::merge_link_field(history, id, field, query)?,
        connection,
    )
    .await
}

pub(crate) async fn boundary_field(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((history, id, field)): ApiPath<(String, String, String)>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        identity,
        ReadHistoryGraph::boundary_field(history, id, field, query)?,
        connection,
    )
    .await
}

pub(crate) async fn commit_pending(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((history, id)): ApiPath<(String, String)>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        identity,
        ReadHistoryGraph::commit_pending(history, id, query)?,
        connection,
    )
    .await
}

pub(crate) async fn branch_pending(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((history, id)): ApiPath<(String, String)>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        identity,
        ReadHistoryGraph::branch_pending(history, id, query)?,
        connection,
    )
    .await
}

pub(crate) async fn deletions(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((history, id)): ApiPath<(String, String)>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        identity,
        ReadHistoryGraph::deletions(history, id, query)?,
        connection,
    )
    .await
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_graph.rs"]
mod tests;
