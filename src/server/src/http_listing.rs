//! Query decoding and response encoding are the listing's HTTP boundary.
use crate::{
    ApiError, AppState, ReadListing,
    backend_listing::ListingResponse,
    http_boundary::{ApiPath, ApiQuery, Authenticated},
};
use http_routing::{
    Json,
    extract::{FromRequestParts, State},
    http::request::Parts,
};
use std::collections::BTreeMap;

/// Query decoding rejects before capacity; domain failures retain worker ordering.
pub(crate) struct ListingInput(Result<ReadListing, ApiError>);

impl<S: Send + Sync> FromRequestParts<S> for ListingInput {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let ApiPath(slot) = ApiPath::<String>::from_request_parts(parts, state).await?;
        let ApiQuery(query) =
            ApiQuery::<BTreeMap<String, String>>::from_request_parts(parts, state).await?;
        Ok(Self(ReadListing::parse(slot, query)))
    }
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ListingInput(request): ListingInput,
) -> Result<Json<ListingResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .listing(&identity, request?)
                .map(ListingResponse::from)
        })
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_listing.rs"]
mod tests;
