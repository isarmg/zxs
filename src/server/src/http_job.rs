//! Ownership of authorized HTTP backend work across request cancellation.
use crate::{ApiError, Backend, auth::AuthStore};
use std::sync::{Arc, Mutex};

pub(crate) struct HttpBackendJob<P> {
    backend: Arc<dyn Backend>,
    auth: Arc<Mutex<AuthStore>>,
    permit: P,
}

impl<P: Send + 'static> HttpBackendJob<P> {
    pub(crate) fn new(backend: Arc<dyn Backend>, auth: Arc<Mutex<AuthStore>>, permit: P) -> Self {
        Self {
            backend,
            auth,
            permit,
        }
    }

    pub(crate) fn run<T: Send + 'static>(
        self,
        work: impl FnOnce(&dyn Backend) -> Result<T, ApiError> + Send + 'static,
    ) -> impl Future<Output = Result<T, ApiError>> + Send {
        self.run_with_permit(move |backend, permit| {
            let _permit = permit;
            work(backend)
        })
    }

    // Enqueue now, before a caller wraps the returned wait in a timeout.
    pub(crate) fn run_with_permit<T: Send + 'static>(
        self,
        work: impl FnOnce(&dyn Backend, P) -> Result<T, ApiError> + Send + 'static,
    ) -> impl Future<Output = Result<T, ApiError>> + Send {
        let job = tokio::task::spawn_blocking(move || self.execute(work));
        async move { job.await.map_err(ApiError::join)? }
    }

    fn execute<T>(
        self,
        work: impl FnOnce(&dyn Backend, P) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let Self {
            backend,
            auth: _auth_owner,
            permit,
        } = self;
        work(backend.as_ref(), permit)
    }
}
