//! Bounded file-history routes. Each handler parses a typed request, runs the
//! backend read and bounded encoding on one blocking slot. The engine lock is
//! released by the read; the response and transport-owned chunks retain the
//! slot through actual socket drain, cancellation or the existing deadline.
use crate::backend_file_history::{EncodedPage, PageBudget};
use crate::*;
use http_routing::response::Response;

/// Check the raw query before percent decoding or allocating a field map.
/// One maximal cursor plus its escaped context fits comfortably; pages have
/// at most seven fields, and no endpoint accepts a repeated field.
pub(crate) struct HistoryQuery {
    pub(crate) query: BTreeMap<String, String>,
    pub(crate) connection: Option<crate::http_shutdown::ResponseConnection>,
}
impl<S: Send + Sync> http_routing::extract::FromRequestParts<S> for HistoryQuery {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut ::http::request::Parts,
        state: &S,
    ) -> Result<Self, ApiError> {
        let raw = parts.uri.query().unwrap_or_default();
        if raw.len() > 128 * 1024 || raw.split('&').take(9).count() > 8 {
            return Err(ApiError::invalid("history query exceeds its parse budget"));
        }
        let ApiQuery(query) = ApiQuery::from_request_parts(parts, state).await?;
        Ok(Self {
            query,
            connection: parts
                .extensions
                .get::<crate::http_shutdown::ResponseConnection>()
                .cloned(),
        })
    }
}

fn permit(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))
}

async fn page<T: Send + 'static>(
    state: AppState,
    connection: Option<crate::http_shutdown::ResponseConnection>,
    read: impl FnOnce(&dyn Backend) -> Result<T, ApiError> + Send + 'static,
    encode: impl FnOnce(&T) -> Result<EncodedPage, ApiError> + Send + 'static,
) -> Result<Response, ApiError> {
    let permit = permit(&state)?;
    let limits = state.config.http_transfer_limits.clone();
    let deadline = tokio::time::Instant::now() + limits.total_timeout;
    let (encoded, permit) = tokio::time::timeout_at(
        deadline,
        crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit).run_with_permit(
            move |backend, permit| {
                let captured = read(backend)?;
                Ok((encode(&captured)?, permit))
            },
        ),
    )
    .await
    .map_err(|source| crate::transfer_limits::timeout_error().with_source(source))??;
    Ok(encoded.into_leased_response(permit, &limits, deadline, connection))
}

pub(crate) async fn nodes(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    crate::history_read::run(
        state,
        connection,
        move |backend, control| {
            backend.file_history_snapshot(&identity, ReadFileHistory::parse(file, query)?, control)
        },
        |captured| captured.encode(PageBudget::PROTOCOL),
    )
    .await
}

pub(crate) async fn node_parents(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((file, node)): ApiPath<(String, String)>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    crate::history_read::run(
        state,
        connection,
        move |backend, control| {
            backend.file_history_node_parents(
                &identity,
                ReadNodeParents::parse(file, node, query)?,
                control,
            )
        },
        |captured| captured.encode(PageBudget::PROTOCOL),
    )
    .await
}

pub(crate) async fn node_field(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((file, node, field)): ApiPath<(String, String, String)>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    crate::history_read::run(
        state,
        connection,
        move |backend, control| {
            backend.file_history_node_field(
                &identity,
                ReadNodeField::parse(file, node, field, query)?,
                control,
            )
        },
        |captured| captured.encode(),
    )
    .await
}

pub(crate) async fn branches(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    crate::history_read::run(
        state,
        connection,
        move |backend, control| {
            backend.file_history_branches(
                &identity,
                ReadHistoryBranches::parse(file, query)?,
                control,
            )
        },
        |captured| captured.encode(PageBudget::PROTOCOL),
    )
    .await
}

pub(crate) async fn merges(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        connection,
        move |backend| {
            backend.file_history_merges(&identity, ReadHistoryMerges::parse(file, query)?)
        },
        |captured| captured.encode(PageBudget::PROTOCOL),
    )
    .await
}

pub(crate) async fn merge_inputs(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((file, merge)): ApiPath<(String, String)>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        connection,
        move |backend| {
            backend.file_history_merge_inputs(
                &identity,
                ReadMergeInputs(crate::backend_file_history::ReadIndexedSlice::parse(
                    file, "merge_id", merge, query,
                )?),
            )
        },
        |captured| captured.encode_inputs(PageBudget::PROTOCOL),
    )
    .await
}

pub(crate) async fn merge_parents(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((file, merge)): ApiPath<(String, String)>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        connection,
        move |backend| {
            backend.file_history_merge_parents(
                &identity,
                ReadMergeParents(crate::backend_file_history::ReadIndexedSlice::parse(
                    file, "merge_id", merge, query,
                )?),
            )
        },
        |captured| captured.encode_merge_parents(PageBudget::PROTOCOL),
    )
    .await
}

pub(crate) async fn versions(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        connection,
        move |backend| backend.file_version_page(&identity, ReadVersionPage::parse(file, query)?),
        |captured| captured.encode(PageBudget::PROTOCOL),
    )
    .await
}

pub(crate) async fn version_sources(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    ApiPath((file, version)): ApiPath<(String, String)>,
    HistoryQuery { query, connection }: HistoryQuery,
) -> Result<Response, ApiError> {
    page(
        state,
        connection,
        move |backend| {
            backend.file_version_sources(
                &identity,
                ReadVersionSources(crate::backend_file_history::ReadIndexedSlice::parse(
                    file,
                    "version_id",
                    version,
                    query,
                )?),
            )
        },
        |captured| captured.encode_sources(PageBudget::PROTOCOL),
    )
    .await
}
