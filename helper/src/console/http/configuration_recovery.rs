use super::*;
use crate::deployment::install::recovery;

pub(super) async fn preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: recovery::Original = input(request).await?;
    let result = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        recovery::preview(input).map_err(configuration_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "configuration_recovery_worker",
            error,
        )
    })??;
    Ok(Json(result).into_response())
}
pub(super) async fn commit(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: recovery::Commit = input(request).await?;
    let result = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        recovery::commit(input).map_err(configuration_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "configuration_recovery_worker",
            error,
        )
    })??;
    Ok(Json(result).into_response())
}
