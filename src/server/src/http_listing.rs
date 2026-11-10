//! Query decoding and response encoding are the listing's HTTP boundary.
use crate::{
    ApiError, AppState, ReadListing,
    auth::Identity,
    backend_listing::ListingResponse,
    http_boundary::{ApiPath, ApiQuery},
};
use http_routing::{Extension, Json, extract::State};
use std::collections::BTreeMap;
pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(slot): ApiPath<String>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<ListingResponse>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .listing(&identity, ReadListing::parse(slot, query)?)
                .map(ListingResponse::from)
        })
        .await
        .map(Json)
}
