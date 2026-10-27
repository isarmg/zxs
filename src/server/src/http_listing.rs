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
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    let backend = state.backend;
    let auth_owner = state.auth;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _auth_owner = auth_owner;
        backend
            .listing(&identity, ReadListing::parse(slot, query)?)
            .map(ListingResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
