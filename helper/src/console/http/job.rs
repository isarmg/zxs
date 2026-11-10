//! One owner for typed JSON jobs, from HTTP admission through blocking work.
use super::{Failure, State, admit_job, input};
use data_serialization::{Deserialize, Serialize, de::DeserializeOwned};
use http_routing::{
    Json,
    extract::{Request, State as ExtractState},
    http::StatusCode,
    response::IntoResponse,
    routing::{MethodRouter, post},
};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Empty {}

#[derive(Clone, Copy)]
pub(super) struct Job {
    operation_error: &'static str,
    worker_error: &'static str,
}
impl Job {
    pub(super) const fn new(operation_error: &'static str, worker_error: &'static str) -> Self {
        Self {
            operation_error,
            worker_error,
        }
    }

    pub(super) fn route<I, O>(self, operation: fn(I) -> crate::Result<O>) -> MethodRouter<State>
    where
        I: DeserializeOwned + Send + 'static,
        O: Serialize + Send + 'static,
    {
        post(
            move |ExtractState(state): ExtractState<State>, request: Request| async move {
                let permit = admit_job(&state, &request)?;
                let input = input::<I>(request).await?;
                let output = tokio::task::spawn_blocking(move || {
                    // A disconnected HTTP caller cannot release the native owner.
                    let _owner = permit;
                    operation(input).map_err(|error| {
                        Failure::message(
                            StatusCode::CONFLICT,
                            self.operation_error,
                            &crate::diagnostic_chain(error.as_ref()),
                        )
                    })
                })
                .await
                .map_err(|error| {
                    Failure::new(StatusCode::INTERNAL_SERVER_ERROR, self.worker_error, error)
                })??;
                Ok::<_, Failure>(Json(output).into_response())
            },
        )
    }
}

#[cfg(test)]
#[path = "../../../../test/helper/unit/console/job_tests.rs"]
mod tests;
