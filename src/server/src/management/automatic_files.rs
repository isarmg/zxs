//! Durable policy, not an HTTP caller or a privileged service account, grants
//! publication authority. Exactly one native preparation may be active here.
//! Stale work is discarded, never queued or retried on behalf of another file.
use super::{Inner, StorageState};
use crate::engine_backend::EngineBackend;
use std::sync::{Arc, Weak, atomic::Ordering};

pub(super) fn start(worker: &crate::periodic_worker::PeriodicWorker, owner: Weak<Inner>) -> std::io::Result<()> {
    let mut cursor = None;
    worker.start(std::thread::Builder::new().name("zxs-automatic-file".into()),
        std::time::Duration::from_secs(1), std::time::Duration::from_secs(1), move || {
            let Some(owner) = owner.upgrade() else { return; };
            let Some(backend) = admitted(&owner) else { return; };
            if let Err(error) = backend.run_automatic_file(&mut cursor, || {
                admitted(&owner).is_some_and(|current| Arc::ptr_eq(&current, &backend))
            }) {
                // The saved working state remains authoritative. A later tick
                // starts over from current eligibility, never this preparation.
                tracing::debug!(error = ?error, "automatic FILE preparation did not publish");
            }
        })
}
fn admitted(owner: &Inner) -> Option<Arc<EngineBackend>> {
    if owner.stopping.load(Ordering::Acquire) { return None; }
    let state = crate::lock(&owner.state);
    if !state.admitted || state.effective_status() != StorageState::Ready { return None; }
    state.backend.active_engine().cloned()
}

