//! Admission and cancellation ownership for bounded exact-root history responses.
use crate::{ApiError, AppState, Backend};
use bytes::Bytes;
use http_routing::{
    body::Body,
    response::{IntoResponse, Response},
};
use storage_contract::history_window::HistoryWindowCancellation;
use tokio::sync::OwnedSemaphorePermit;
struct PageBytes {
    bytes: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}
impl AsRef<[u8]> for PageBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
struct CancelOnDrop(HistoryWindowCancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
pub(crate) async fn read(
    state: AppState,
    work: impl FnOnce(&dyn Backend, HistoryWindowCancellation) -> Result<Vec<u8>, ApiError>
    + Send
    + 'static,
) -> Result<Response, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    let cancellation = HistoryWindowCancellation::new();
    let _cancel = CancelOnDrop(cancellation.clone());
    let bytes = crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run_with_permit(move |backend, permit| {
            let bytes = work(backend, cancellation)?;
            Ok(Bytes::from_owner(PageBytes {
                bytes,
                _permit: permit,
            }))
        })
        .await?;
    Ok((
        [(http::header::CONTENT_TYPE, "application/json")],
        Body::from(bytes),
    )
        .into_response())
}
