//! Short control-plane calls never acquire the Engine mutex or a data-job slot.
use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::ApiPath,
    integrity_scrub::{IntegrityScrubJobId, StatusResponse},
};
use data_serialization::Serialize;
use http_routing::{
    Extension, Json, Router,
    extract::{Request, State},
    routing::{get, post},
};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/scrub", get(status).post(start))
        .route("/api/admin/scrub/{job_id}/cancel", post(cancel))
}
#[derive(Serialize)]
struct Started {
    #[serialization(with = "storage_contract::u64_string")]
    job_id: u64,
}
async fn start(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    request: Request,
) -> Result<(http::StatusCode, Json<Started>), ApiError> {
    empty_request(request).await?;
    let job = state.backend.start_integrity_scrub(&identity)?;
    Ok((http::StatusCode::ACCEPTED, Json(Started { job_id: job.0 })))
}
async fn status(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<StatusResponse>, ApiError> {
    state
        .backend
        .integrity_scrub_status(&identity)
        .map(StatusResponse::from)
        .map(Json)
}
async fn cancel(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(job): ApiPath<String>,
    request: Request,
) -> Result<http::StatusCode, ApiError> {
    empty_request(request).await?;
    state
        .backend
        .cancel_integrity_scrub(&identity, IntegrityScrubJobId::parse(&job)?)?;
    Ok(http::StatusCode::ACCEPTED)
}

async fn empty_request(request: Request) -> Result<(), ApiError> {
    match crate::json_body(request).await? {
        json_codec::Value::Object(fields) if fields.is_empty() => Ok(()),
        _ => Err(ApiError::invalid(
            "scrub control requests accept no fields; policy is host configuration",
        )),
    }
}
