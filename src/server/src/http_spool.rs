//! Optional HTTP staging startup. Only a complete exclusive inventory admits work.
use crate::{ApiError, ServerConfig, spool_budget::SpoolBudget};
use std::sync::{Arc, Mutex};

enum State {
    Pending,
    Ready(Arc<SpoolBudget>),
    Unavailable,
}

pub(crate) struct HttpSpool {
    state: Mutex<State>,
}

/// A non-cloneable, consumed startup owner. The blocking job never publishes
/// readiness itself; dropping this future prevents publication of a late result.
pub(crate) struct Startup {
    spool: Arc<HttpSpool>,
}

impl HttpSpool {
    /// Explicitly synchronous inventory for callers constructing an embedded router.
    pub(crate) fn open_blocking(config: &ServerConfig) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(match config.open_http_spool() {
                Some(budget) => State::Ready(budget),
                None => State::Unavailable,
            }),
        })
    }

    pub(crate) fn pending() -> (Arc<Self>, Startup) {
        let state = Arc::new(Self {
            state: Mutex::new(State::Pending),
        });
        (state.clone(), Startup { spool: state })
    }

    pub(crate) fn status(&self) -> &'static str {
        match self.state.lock().as_deref() {
            Ok(State::Pending) => "pending",
            Ok(State::Ready(_)) => "ready",
            Ok(State::Unavailable) | Err(_) => "unavailable",
        }
    }

    pub(crate) fn budget(&self) -> Result<Arc<SpoolBudget>, ApiError> {
        match self.state.lock().as_deref() {
            Ok(State::Ready(budget)) => Ok(budget.clone()),
            Ok(State::Pending) => Err(ApiError::new(
                503,
                storage_contract::ErrorCode::HttpUploadsPending,
                "HTTP upload staging inventory is still running; retry after it completes",
            )),
            Ok(State::Unavailable) | Err(_) => Err(ApiError::new(
                503,
                storage_contract::ErrorCode::HttpUploadsUnavailable,
                "HTTP upload staging is unavailable; repair the local spool and restart",
            )),
        }
    }
}

impl Startup {
    pub(crate) async fn run(self, config: ServerConfig) {
        self.run_with(move || config.open_http_spool()).await;
    }

    async fn run_with(self, work: impl FnOnce() -> Option<Arc<SpoolBudget>> + Send + 'static) {
        // As for SMB startup_io, Tokio owns any started blocking work through
        // runtime teardown even if the service drops this awaiting future.
        // The closure owns only inventory inputs/result, never the publisher.
        let result = tokio::task::spawn_blocking(work).await;
        let next = match result {
            Ok(Some(budget)) => State::Ready(budget),
            Ok(None) => State::Unavailable,
            Err(error) => {
                tracing::error!(%error, "HTTP spool startup job failed; management remains available");
                State::Unavailable
            }
        };
        if let Ok(mut state) = self.spool.state.lock() {
            debug_assert!(matches!(*state, State::Pending));
            *state = next;
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_spool.rs"]
mod tests;
