//! Query decoding and response encoding are the listing's HTTP boundary.
use crate::{
    ApiError, AppState, ReadListing,
    auth::Identity,
    backend_listing::ListingResponse,
    http_boundary::{ApiPath, ApiQuery},
};
use http_routing::{Extension, extract::State, response::Response};
use std::collections::BTreeMap;
pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(slot): ApiPath<String>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
    connection: Option<Extension<crate::http_shutdown::ResponseConnection>>,
) -> Result<Response, ApiError> {
    crate::history_read::run(
        state,
        connection.map(|Extension(connection)| connection),
        move |backend, control| {
            backend.listing(&identity, ReadListing::parse(slot, query)?, control)
        },
        |snapshot| {
            crate::backend_file_history::encode_response_bounded(ListingResponse::from(snapshot))
        },
    )
    .await
}
