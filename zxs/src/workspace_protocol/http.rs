//! Bridge from an authenticated bounded request to the checked Store.
mod json;
mod reply;

use super::{
    Request,
    dispatch::{self, Body, Failure},
};
use crate::{
    container_store::Store,
    headless_http::{Incoming, Outcome, Problem},
};

pub(crate) fn dispatch_request(
    store: &Store,
    request: Request,
    incoming: &mut Incoming<'_>,
    live: &dyn Fn() -> bool,
) -> Outcome {
    execute(store, request, incoming, live).unwrap_or_else(Outcome::Failed)
}

fn execute(
    store: &Store,
    request: Request,
    incoming: &mut Incoming<'_>,
    live: &dyn Fn() -> bool,
) -> Result<Outcome, Problem> {
    let body = match incoming {
        Incoming::Empty => Body::Empty,
        Incoming::Blob(reader) => Body::Blob(reader),
        Incoming::Json(reader) => {
            let length = reader.remaining();
            let decoded = json::decode(&request, reader, length).ok_or(Problem::Malformed)?;
            Body::Json { length, decoded }
        }
    };
    let result =
        dispatch::dispatch(store, request, body, |_| live()).map_err(|failure| match failure {
            Failure::Denied => Problem::Denied,
            Failure::BadBody => Problem::Malformed,
            Failure::Missing => Problem::Missing,
            Failure::Conflict => Problem::Conflict,
            Failure::InvalidBody => Problem::InvalidBody,
            Failure::Unavailable => Problem::Unavailable,
        })?;
    reply::project(result)
}
