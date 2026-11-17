//! Dedicated connection threads; only their shutdown joins use the runtime pool.
use super::*;
use std::{
    os::unix::net::UnixStream,
    sync::atomic::{AtomicBool, Ordering},
    thread::JoinHandle,
};

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
struct Worker {
    thread: JoinHandle<io::Result<()>>,
    activity: Option<(u64, Arc<ConnectionActivity>)>,
}
struct Workers {
    connections: Option<Connections>,
    threads: Vec<Worker>,
    quiescing: Arc<AtomicBool>,
}

/// Only a frame boundary with no live handles is idle. Starting an exchange
/// clears idle before consulting the shared admission flag.
pub(super) struct ConnectionActivity {
    quiescing: Arc<AtomicBool>,
    idle: AtomicBool,
}

impl ConnectionActivity {
    pub(super) fn receiving(&self, has_handles: bool) -> bool {
        self.idle.store(!has_handles, Ordering::SeqCst);
        has_handles || !self.quiescing()
    }

    pub(super) fn handling(&self) {
        self.idle.store(false, Ordering::SeqCst);
    }

    pub(super) fn quiescing(&self) -> bool {
        self.quiescing.load(Ordering::SeqCst)
    }
}
impl Workers {
    fn quiesce(&self) {
        self.quiescing.store(true, Ordering::SeqCst);
        if let Some(connections) = &self.connections {
            let connections = closing_connections(connections);
            for worker in &self.threads {
                if let Some((id, activity)) = &worker.activity
                    && activity.idle.load(Ordering::SeqCst)
                    && let Some(stream) = connections.get(id)
                {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
            }
        }
    }

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
            Self::join(thread.thread);
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
    #[cfg(test)]
    pub(super) fn spawn(
        &self,
        work: impl FnOnce() -> io::Result<()> + Send + 'static,
    ) -> io::Result<()> {
        self.spawn_worker(None, |_| work())
    }

    pub(super) fn spawn_connection(
        &self,
        id: u64,
        work: impl FnOnce(Option<Arc<ConnectionActivity>>) -> io::Result<()> + Send + 'static,
    ) -> io::Result<()> {
        self.spawn_worker(Some(id), work)
    }

    fn spawn_worker(
        &self,
        id: Option<u64>,
        work: impl FnOnce(Option<Arc<ConnectionActivity>>) -> io::Result<()> + Send + 'static,
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
        if workers.quiescing.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "SMB admission stopped"));
        }
        // Reap only finished threads on the async accept path. Keep retained
        // handles bounded as well as live sessions, including the permit-drop
        // interval immediately before a worker returns.
        let mut index = 0;
        while index < workers.threads.len() {
            if workers.threads[index].thread.is_finished() {
                Workers::join(workers.threads.swap_remove(index).thread);
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
        let activity = id.map(|id| (id, Arc::new(ConnectionActivity {
            quiescing: Arc::clone(&workers.quiescing),
            idle: AtomicBool::new(true),
        })));
        let observation = activity.as_ref().map(|(_, activity)| Arc::clone(activity));
        let thread = std::thread::Builder::new()
            .name("smb-connection".into())
            .spawn(move || work(observation))?;
        workers.threads.push(Worker { thread, activity });
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
                quiescing: Arc::new(AtomicBool::new(false)),
            })))),
            drain: None,
        }
    }
    pub(crate) fn registry(&self) -> ConnectionRegistry {
        self.registry.clone()
    }
    /// The listener is no longer polled on the normal-stop path. Existing
    /// handle owners are responsible for closing their own exchanges.
    pub(crate) async fn wait_idle(&self) {
        loop {
            let idle = match self.registry.0.lock() {
                Ok(state) => state.as_ref().is_none_or(|workers| {
                    workers.quiesce();
                    workers.threads.iter().all(|worker| worker.thread.is_finished())
                }),
                // Let the existing force-close/join path collect a damaged owner.
                Err(_) => true,
            };
            if idle {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
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
