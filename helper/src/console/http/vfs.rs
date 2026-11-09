use super::*;
use crate::installation::vfs as installation;

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct PreviewInput {}

pub(super) async fn preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let _: PreviewInput = input(request).await?;
    let value = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        installation::preview().map_err(failure)
    })
    .await
    .map_err(join_failure)??;
    Ok(Json(value).into_response())
}
pub(super) async fn status(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: installation::Original = input(request).await?;
    let value = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        installation::status(input).map_err(failure)
    })
    .await
    .map_err(join_failure)??;
    Ok(Json(value).into_response())
}
pub(super) async fn commit(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: installation::Commit = input(request).await?;
    let value = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        installation::commit(input).map_err(failure)
    })
    .await
    .map_err(join_failure)??;
    Ok(Json(value).into_response())
}
fn join_failure(error: tokio::task::JoinError) -> Failure {
    Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "vfs_installation_worker", error)
}
fn failure(error: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    Failure::message(
        StatusCode::CONFLICT, "sealed_vfs_installation",
        &crate::diagnostic_chain(error.as_ref()),
    )
}
