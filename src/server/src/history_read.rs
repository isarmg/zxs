//! Cooperative cancellation shared by graph, activity, directory listings and complete-tree readers.
use crate::{
    ApiError, AppState, Backend, backend_file_history::EncodedPage,
    http_shutdown::ResponseConnection,
};
use http_routing::response::Response;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

/// A request-local stop signal. It never releases a running worker's permit.
pub struct HistoryReadControl {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
}
pub(crate) struct HistoryReadWait(Arc<AtomicBool>);
impl Drop for HistoryReadWait {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
impl HistoryReadControl {
    pub fn until(deadline: Instant) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline,
        }
    }
    pub(crate) fn wait_owner(&self) -> HistoryReadWait {
        HistoryReadWait(Arc::clone(&self.cancelled))
    }
    /// Returns `RequestTimeout` when the request was cancelled or its deadline elapsed.
    pub fn checkpoint(&self) -> storage_contract::Result<()> {
        if self.cancelled.load(Ordering::Relaxed) || Instant::now() >= self.deadline {
            return Err(storage_contract::Error::new(
                storage_contract::ErrorCode::RequestTimeout,
                "history inspection was cancelled or its deadline expired",
            ));
        }
        Ok(())
    }
}

pub(crate) async fn run<T: Send + 'static>(
    state: AppState,
    connection: Option<ResponseConnection>,
    read: impl FnOnce(&dyn Backend, &HistoryReadControl) -> Result<T, ApiError> + Send + 'static,
    encode: impl FnOnce(T) -> Result<EncodedPage, ApiError> + Send + 'static,
) -> Result<Response, ApiError> {
    let scan = state
        .history_read_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| {
            ApiError::limit("history inspection capacity reached").with_source(source)
        })?;
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    let limits = state.config.http_transfer_limits.clone();
    let deadline = tokio::time::Instant::now() + limits.total_timeout;
    // A scan retains an old State; its work lease is shorter than socket drain.
    let work_deadline =
        deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(30));
    let control = HistoryReadControl::until(work_deadline.into_std());
    let _wait_owner = control.wait_owner();
    let (encoded, permit) = tokio::time::timeout_at(
        work_deadline,
        crate::http_job::HttpBackendJob::new(state.backend, state.auth, (scan, permit))
            .run_with_permit(move |backend, (scan, permit)| {
                let captured = read(backend, &control)?;
                drop(scan);
                control.checkpoint().map_err(|source| {
                    ApiError::new(
                        ::http::StatusCode::REQUEST_TIMEOUT,
                        source.code(),
                        source.message(),
                    )
                    .with_source(source)
                })?;
                Ok((encode(captured)?, permit))
            }),
    )
    .await
    .map_err(|source| {
        ApiError::new(
            ::http::StatusCode::REQUEST_TIMEOUT,
            storage_contract::ErrorCode::RequestTimeout,
            "history inspection deadline exceeded",
        )
        .with_source(source)
    })??;
    Ok(encoded.into_leased_response(permit, &limits, deadline, connection))
}
