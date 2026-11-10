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
    let job =
        crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?;
    let cancellation = HistoryWindowCancellation::new();
    let _cancel = CancelOnDrop(cancellation.clone());
    let bytes = job
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
