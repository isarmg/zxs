//! Bounded history inspection with cooperative worker cancellation and socket leases.
use crate::http_file_history::HistoryQuery;
use crate::*;

async fn page(
    state: AppState,
    identity: Identity,
    request: ReadHistoryGraph,
    connection: Option<crate::http_shutdown::ResponseConnection>,
) -> Result<Response, ApiError> {
    crate::history_read::run(
        state,
        connection,
        move |backend, control| backend.history_graph(&identity, request, control),
        |captured| captured.encode(),
    )
    .await
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
