use super::{Failure, State, admit_job, input};
use crate::initialization::packages;
use http_routing::{
    Json,
    extract::{Request, State as ExtractState},
    http::StatusCode,
    response::{IntoResponse, Response},
};
pub(super) async fn script() -> impl IntoResponse {
    (
        [("content-type", "text/javascript; charset=utf-8")],
        include_str!("../../../web/packages.js"),
    )
}
fn failure(source: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    Failure::message(
        StatusCode::CONFLICT,
        "system_packages",
        &crate::diagnostic_chain(source.as_ref()),
    )
}
pub(super) async fn preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let value = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        packages::preview().map_err(failure)
    })
    .await
    .map_err(|source| {
        Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "packages_worker", source)
    })??;
    Ok(Json(value).into_response())
}
pub(super) async fn refresh(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let request = input::<packages::Refresh>(request).await?;
    let value = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        packages::refresh(request).map_err(failure)
    })
    .await
    .map_err(|source| {
        Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "packages_worker", source)
    })??;
    Ok(Json(value).into_response())
}
pub(super) async fn install(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let request = input::<packages::Commit>(request).await?;
    let value = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        packages::install(request).map_err(failure)
    })
    .await
    .map_err(|source| {
        Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "packages_worker", source)
    })??;
    Ok(Json(value).into_response())
}
pub(super) async fn finish(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let request = input::<packages::Commit>(request).await?;
    let value = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        packages::finish(request).map_err(failure)
    })
    .await
    .map_err(|source| {
        Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "packages_worker", source)
    })??;
    Ok(Json(value).into_response())
}
