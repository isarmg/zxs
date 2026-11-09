//! Bounded authenticated jobs adapt one explicit command; GET never advances it.
use crate::{ApiError, AppState, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json, Router,
    extract::{Request, State},
    routing::{get, post},
};
use storage_contract::{OperationId, backup_transaction as wire};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/backup-transactions", get(current).post(start))
        .route("/api/admin/backup-transactions/context", get(context))
        .route("/api/admin/backup-transactions/{operation}", get(status))
        .route(
            "/api/admin/backup-transactions/{operation}/continue",
            post(advance),
        )
}

fn job(
    state: AppState,
) -> Result<crate::http_job::HttpBackendJob<tokio::sync::OwnedSemaphorePermit>, ApiError> {
    let permit = state
        .management_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| {
            ApiError::limit("management request capacity reached").with_source(source)
        })?;
    Ok(crate::http_job::HttpBackendJob::new(
        state.backend,
        state.auth,
        permit,
    ))
}

async fn body<T: data_serialization::de::DeserializeOwned>(
    request: Request,
) -> Result<T, ApiError> {
    let bytes = crate::http_boundary::read_body(request, wire::MAX_REQUEST_BYTES).await?;
    storage_contract::decode_strict(&bytes).map_err(ApiError::invalid_source)
}

async fn context(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<wire::Context>, ApiError> {
    job(state)?
        .run(move |backend| backend.backup_transaction_context(&identity))
        .await
        .map(Json)
}

async fn current(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<wire::Current>, ApiError> {
    job(state)?
        .run(move |backend| backend.current_backup_transaction(&identity))
        .await
        .map(Json)
}

async fn status(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(operation): ApiPath<OperationId>,
) -> Result<Json<wire::Snapshot>, ApiError> {
    job(state)?
        .run(move |backend| backend.backup_transaction_status(&identity, operation))
        .await
        .map(Json)
}

async fn start(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    request: Request,
) -> Result<(http::StatusCode, Json<wire::Snapshot>), ApiError> {
    let request: wire::StartRequest = body(request).await?;
    let result = job(state)?
        .run(move |backend| backend.start_backup_transaction(&identity, request))
        .await?;
    Ok((http::StatusCode::ACCEPTED, Json(result)))
}

async fn advance(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(operation): ApiPath<OperationId>,
    request: Request,
) -> Result<(http::StatusCode, Json<wire::CommandResponse>), ApiError> {
    let command: wire::Command = body(request).await?;
    let result = job(state)?
        .run(move |backend| backend.continue_backup_transaction(&identity, operation, command))
        .await?;
    // Accepted is never a transaction-success assertion, including a late reply.
    Ok((http::StatusCode::ACCEPTED, Json(result)))
}
