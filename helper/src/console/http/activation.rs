use super::*;
use crate::deployment::activation;

fn failure(error: Box<dyn std::error::Error + Send + Sync>) -> Failure {
    Failure::message(
        StatusCode::CONFLICT,
        "service_activation",
        &crate::diagnostic_chain(error.as_ref()),
    )
}
pub(super) async fn activate(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let input: activation::Commit = input(request).await?;
    let (result, _permit) =
        tokio::task::spawn_blocking(move || (activation::activate(input).map_err(failure), permit))
            .await
            .map_err(|error| {
                Failure::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "activation_worker",
                    error,
                )
            })?;
    let (response, handoff) = result?;
    state.request_handoff(handoff).map_err(failure)?;
    Ok((StatusCode::ACCEPTED, Json(response)).into_response())
}
