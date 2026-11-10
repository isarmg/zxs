use super::{Failure, State, admit_job, input};
use http_routing::{
    Json,
    extract::{Request, State as ExtractState},
    http::StatusCode,
    response::{IntoResponse, Response},
};

pub(super) async fn script() -> impl IntoResponse {
    (
        [("content-type", "text/javascript; charset=utf-8")],
        include_str!("../../../web/service_account.js"),
    )
}
fn failure(source: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    Failure::message(
        StatusCode::CONFLICT,
        "local_service_account",
        &crate::diagnostic_chain(source.as_ref()),
    )
}
pub(super) async fn preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let request = input::<crate::initialization::account::Input>(request).await?;
    let value = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::initialization::account::preview(request).map_err(failure)
    })
    .await
    .map_err(|source| {
        Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "account_worker", source)
    })??;
    Ok(Json(value).into_response())
}
pub(super) async fn prepare(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let request = input::<crate::initialization::account::Prepare>(request).await?;
    let value = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::initialization::account::prepare(request).map_err(failure)
    })
    .await
    .map_err(|source| {
        Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "account_worker", source)
    })??;
    Ok(Json(value).into_response())
}
