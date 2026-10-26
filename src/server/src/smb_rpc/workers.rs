//! Dedicated connection threads; only their shutdown joins use the runtime pool.
use super::*;
use std::{os::unix::net::UnixStream, thread::JoinHandle};

pub(super) type Connections = Arc<Mutex<BTreeMap<u64, UnixStream>>>;

// Only resource release may consume a poisoned socket inventory. It contains
// owned descriptors, not transactional business state; this does not clear
// poison or authorize admission to resume.
pub(super) fn closing_connections(
    connections: &Connections,
) -> std::sync::MutexGuard<'_, BTreeMap<u64, UnixStream>> {
    match connections.lock() {
        Ok(connections) => connections,
        Err(poisoned) => {
            tracing::error!("closing poisoned SMB socket inventory");
            poisoned.into_inner()
        }
    }
}

pub(crate) struct ConnectionRegistry(Arc<Mutex<Option<Workers>>>);
struct Workers {
    connections: Option<Connections>,
    threads: Vec<JoinHandle<io::Result<()>>>,
}
impl Workers {
    fn stop(&mut self) {
        let Some(connections) = self.connections.take() else {
            return;
        };
        let connections = closing_connections(&connections);
        for stream in connections.values() {
            if let Err(error) = stream.shutdown(std::net::Shutdown::Both)
                && error.kind() != io::ErrorKind::NotConnected
            {
                tracing::warn!(error = ?error, "SMB socket shutdown failed");
            }
        }
    }
    fn join(thread: JoinHandle<io::Result<()>>) {
        match thread.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(error = ?error, "SMB local RPC session ended"),
            Err(panic) => {
                let message = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&str>().copied());
                tracing::error!(message, "SMB local RPC worker panicked");
            }
        }
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        self.stop();
        for thread in self.threads.drain(..) {
            Self::join(thread);
        }
    }
}
impl Clone for ConnectionRegistry {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl ConnectionRegistry {
    pub(super) fn attach_connections(&self, connections: Connections) {
        self.0
            .lock()
            .expect("invariant: worker registry lock is not poisoned")
            .as_mut()
            .expect("invariant: listener starts before shutdown")
            .connections = Some(connections);
    }
    pub(super) fn spawn(
        &self,
        work: impl FnOnce() -> io::Result<()> + Send + 'static,
    ) -> io::Result<()> {
        let mut state = self
            .0
            .lock()
            .expect("invariant: worker registry lock is not poisoned");
        let Some(workers) = state.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "SMB admission stopped",
            ));
        };
        // Reap only finished threads on the async accept path. Keep retained
        // handles bounded as well as live sessions, including the permit-drop
        // interval immediately before a worker returns.
        let mut index = 0;
        while index < workers.threads.len() {
            if workers.threads[index].is_finished() {
                Workers::join(workers.threads.swap_remove(index));
            } else {
                index += 1;
            }
        }
        if workers.threads.len() == MAX_CONNECTIONS {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "SMB worker capacity reached",
            ));
        }
        workers.threads.push(
            std::thread::Builder::new()
                .name("smb-connection".into())
                .spawn(work)?,
        );
        Ok(())
    }
}

// Both the queued closure and enqueue-failure fallback can carry this
// obligation. The handoff lock stays held through join, so a concurrent
// fallback waits for actual completion, not merely for another consumer's take.
// Connection workers never access this lock.
struct WorkerDrain(Arc<Mutex<Option<Workers>>>);
impl Drop for WorkerDrain {
    fn drop(&mut self) {
        let mut pending = match self.0.lock() {
            Ok(pending) => pending,
            Err(poisoned) => {
                tracing::error!("closing poisoned SMB drain handoff");
                poisoned.into_inner()
            }
        };
        drop(pending.take());
        drop(pending);
    }
}

pub(crate) struct ConnectionShutdown {
    registry: ConnectionRegistry,
    drain: Option<tokio::task::JoinHandle<()>>,
}
impl ConnectionShutdown {
    pub(crate) fn new() -> Self {
        Self {
            registry: ConnectionRegistry(Arc::new(Mutex::new(Some(Workers {
                connections: Some(Arc::new(Mutex::new(BTreeMap::new()))),
                threads: Vec::with_capacity(MAX_CONNECTIONS),
            })))),
            drain: None,
        }
    }
    pub(crate) fn registry(&self) -> ConnectionRegistry {
        self.registry.clone()
    }
    pub(crate) fn begin(&mut self) {
        // Closing takes the entire set even after an unwind poisoned this
        // owner. Leave None and keep the mutex poisoned: no admission resumes.
        let workers = match self.registry.0.lock() {
            Ok(mut state) => state.take(),
            Err(poisoned) => {
                tracing::error!("closing poisoned SMB worker registry");
                poisoned.into_inner().take()
            }
        };
        if let Some(mut workers) = workers {
            workers.stop();
            // The closure owns Workers' join-on-drop obligation. Even if a
            // queued job is discarded during runtime teardown, no native
            // handle is detached. Outside a runtime we synchronously drain.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let handoff = Arc::new(Mutex::new(Some(workers)));
                let drain = WorkerDrain(Arc::clone(&handoff));
                // Tokio may panic on OS thread creation *after* enqueueing the
                // closure. Catch only this task-creation boundary; the shared
                // take-once obligation also covers that queued closure.
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.spawn_blocking(move || drop(drain))
                })) {
                    Ok(task) => self.drain = Some(task),
                    Err(panic) => {
                        drop(WorkerDrain(handoff));
                        let message = panic
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| panic.downcast_ref::<&str>().copied());
                        tracing::error!(
                            message,
                            "SMB shutdown enqueue panicked; workers joined synchronously"
                        );
                    }
                }
            } else {
                drop(workers);
            }
        }
    }
    pub(crate) async fn finish(&mut self) -> io::Result<()> {
        self.begin();
        if let Some(drain) = &mut self.drain {
            drain.await.map_err(io::Error::other)?;
        }
        self.drain = None;
        Ok(())
    }
}
impl Drop for ConnectionShutdown {
    fn drop(&mut self) {
        self.begin();
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/smb_rpc/worker_tests.rs"]
mod tests;
