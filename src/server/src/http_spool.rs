//! Optional HTTP staging startup. Only a complete exclusive inventory admits work.
use crate::{
    AdmittedServerConfig, ApiError, Backend, InternalHttpSpoolRetirement, ServerConfig,
    spool_budget::SpoolBudget,
};
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
    pub(crate) fn open_blocking(config: &ServerConfig, backend: &dyn Backend) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(match config.open_http_spool() {
                Some(budget) => {
                    if let Err(error) = budget.retire_startup(backend) {
                        tracing::warn!(error = ?error, "HTTP staging retirement deferred");
                    }
                    State::Ready(budget)
                }
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
        match *self.state.lock().expect("service state lock poisoned") {
            State::Pending => "pending",
            State::Ready(ref budget) if budget.is_uncertain() => "recovery_required",
            State::Ready(_) => "ready",
            State::Unavailable => "unavailable",
        }
    }

    pub(crate) fn maintenance_issues(&self) -> Option<usize> {
        match &*self.state.lock().expect("service state lock poisoned") {
            State::Ready(budget) => Some(budget.maintenance_issue_count()),
            State::Pending | State::Unavailable => None,
        }
    }

    pub(crate) fn retirement_budget(&self) -> Option<Arc<SpoolBudget>> {
        match &*self.state.lock().expect("service state lock poisoned") {
            State::Ready(budget) => Some(Arc::clone(budget)),
            State::Pending | State::Unavailable => None,
        }
    }

    pub(crate) fn budget(&self) -> Result<Arc<SpoolBudget>, ApiError> {
        match *self.state.lock().expect("service state lock poisoned") {
            State::Ready(ref budget) => Ok(budget.clone()),
            State::Pending => Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::HttpUploadsPending,
                "HTTP upload staging inventory is still running; retry after it completes",
            )),
            State::Unavailable => Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::HttpUploadsUnavailable,
                "HTTP upload staging is unavailable; repair the local spool and restart",
            )),
        }
    }
}

impl Startup {
    pub(crate) async fn run(self, config: Arc<AdmittedServerConfig>, backend: Arc<dyn Backend>) {
        // Subscribe before inventory and the first qualification, so an engine
        // installed during either blocking operation cannot be missed.
        let mut activation = backend.http_spool_activation(&InternalHttpSpoolRetirement(()));
        let mut next = Self::inventory(move || config.open_http_spool()).await;
        if let State::Ready(budget) = &next {
            loop {
                let job_budget = Arc::clone(budget);
                let job_backend = Arc::clone(&backend);
                let result = tokio::task::spawn_blocking(move || {
                    job_budget.retire_startup(job_backend.as_ref())
                })
                .await;
                match result {
                    Ok(Ok(true)) => break,
                    Ok(Ok(false)) => {}
                    Ok(Err(error)) => {
                        tracing::warn!(error = ?error, "HTTP staging retirement requires recovery");
                        break;
                    }
                    Err(error) => {
                        tracing::error!(error = ?error, "HTTP staging retirement job failed");
                        next = State::Unavailable;
                        break;
                    }
                }
                let Some(receiver) = &mut activation else {
                    tracing::warn!(
                        "HTTP staging retirement deferred without an activation subscription"
                    );
                    break;
                };
                if receiver.changed().await.is_err() {
                    tracing::warn!(
                        "HTTP staging retirement deferred after activation channel closed"
                    );
                    break;
                }
            }
        }
        self.publish(next);
    }

    #[cfg(test)]
    async fn run_with(self, work: impl FnOnce() -> Option<Arc<SpoolBudget>> + Send + 'static) {
        self.publish(Self::inventory(work).await);
    }

    async fn inventory(work: impl FnOnce() -> Option<Arc<SpoolBudget>> + Send + 'static) -> State {
        // As for SMB startup_io, Tokio owns any started blocking work through
        // runtime teardown even if the service drops this awaiting future.
        // The closure owns only inventory inputs/result, never the publisher.
        let result = tokio::task::spawn_blocking(work).await;
        match result {
            Ok(Some(budget)) => State::Ready(budget),
            Ok(None) => State::Unavailable,
            Err(error) => {
                tracing::error!(error = ?error, "HTTP spool startup job failed; management remains available");
                State::Unavailable
            }
        }
    }

    fn publish(self, next: State) {
        {
            let mut state = self
                .spool
                .state
                .lock()
                .expect("service state lock poisoned");
            debug_assert!(matches!(*state, State::Pending));
            *state = next;
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_spool.rs"]
mod tests;
