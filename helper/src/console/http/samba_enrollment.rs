//! Secret-bearing enrollment uses the same admission, work owner and redaction
//! boundary as existing-account resets. Status never replays an external command.
use super::*;
use crate::deployment::samba_password::{self, enrollment};

pub(super) async fn preview(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: samba_password::Input = input(request).await?;
    let preview = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        enrollment::preview(input).map_err(failure)
    })
    .await
    .map_err(join_failure)??;
    Ok(Json(preview).into_response())
}
pub(super) async fn create(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: enrollment::Create = secret_input(request).await?;
    let receipt = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        enrollment::create(input).map_err(failure)
    })
    .await
    .map_err(join_failure)??;
    Ok(Json(receipt).into_response())
}
pub(super) async fn status(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: samba_password::Original = input(request).await?;
    let status = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        enrollment::status(input).map_err(failure)
    })
    .await
    .map_err(join_failure)??;
    Ok(Json(status).into_response())
}
fn join_failure(error: tokio::task::JoinError) -> Failure {
    Failure::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "samba_enrollment_worker",
        error,
    )
}
fn failure(error: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    Failure::message(
        StatusCode::CONFLICT,
        "local_samba_account_enrollment",
        &error.to_string(),
    )
}
