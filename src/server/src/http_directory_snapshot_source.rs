//! Protected bounded source verification, without repository initialization or publication.
use crate::{
    ApiError, AppState, ReadDirectorySnapshotSource,
    http_boundary::{ApiQuery, Authenticated},
};
use http_routing::{extract::State, response::Response};

// Source capture caps its escaped manifest estimate at 4 MiB. The extra page
// allowance holds only the bounded identity, status and budget fields.
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024 + 4096;

pub(crate) async fn read(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiQuery(request): ApiQuery<ReadDirectorySnapshotSource>,
) -> Result<Response, ApiError> {
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let preview = backend.directory_snapshot_source(&identity, request, cancellation)?;
        crate::history_window_encode::encode_bounded(&preview, MAX_RESPONSE_BYTES)
    })
    .await
}
