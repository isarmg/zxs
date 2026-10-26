//! Keep optional listeners owned by the HTTP service, including during drain.
use std::{
    future::Future,
    io,
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;

/// Own native management writers even if serve setup fails or its future is
/// cancelled. Blocking joins run on Tokio's tracked blocking pool, whose
/// runtime teardown waits for them instead of silently detaching a writer.
pub(crate) struct ManagementShutdown {
    backend: Option<Arc<dyn crate::Backend>>,
    smb: crate::smb_rpc::ConnectionShutdown,
    drain: Option<tokio::task::JoinHandle<()>>,
    completion: Option<Arc<Mutex<NativeDrain>>>,
}
/// Tokio may discard queued non-mandatory blocking jobs during runtime
/// shutdown. The closure must own a drain-on-drop obligation, not just an Arc.
struct NativeDrain {
    backend: Option<Arc<dyn crate::Backend>>,
    completion: Option<DrainCompletion>,
}
impl NativeDrain {
    fn new(backend: Arc<dyn crate::Backend>) -> Self {
        Self {
            backend: Some(backend),
            completion: None,
        }
    }
    fn run(&mut self) {
        if let Some(backend) = self.backend.take() {
            self.completion = Some(DrainCompletion(Some(backend.drain_management())));
        }
    }
}
struct DrainCompletion(Option<Result<(), crate::ApiError>>);
impl Drop for DrainCompletion {
    fn drop(&mut self) {
        if let Some(Err(error)) = self.0.take() {
            tracing::error!(error = ?error, "management shutdown failed");
        }
    }
}

impl Drop for NativeDrain {
    fn drop(&mut self) {
        self.run();
    }
}

// The queue and a failed-enqueue fallback share one drain/result owner. This
// private handoff lock is never used by backend workers and stays held until
// the actual drain and result publication finish.
struct ManagementDrain(Arc<Mutex<NativeDrain>>);
impl Drop for ManagementDrain {
    fn drop(&mut self) {
        let mut drain = match self.0.lock() {
            Ok(drain) => drain,
            Err(poisoned) => {
                tracing::error!("closing poisoned management drain handoff");
                poisoned.into_inner()
            }
        };
        drain.run();
    }
}
impl ManagementShutdown {
    pub(crate) fn new(backend: Arc<dyn crate::Backend>) -> Self {
        Self {
            backend: Some(backend),
            smb: crate::smb_rpc::ConnectionShutdown::new(),
            drain: None,
            completion: None,
        }
    }
    pub(crate) fn smb_connections(&self) -> crate::smb_rpc::ConnectionRegistry {
        self.smb.registry()
    }
    fn begin(&mut self) {
        self.smb.begin();
        let Some(backend) = self.backend.take() else {
            return;
        };
        backend.stop_management();
        let handoff = Arc::new(Mutex::new(NativeDrain::new(backend)));
        self.completion = Some(Arc::clone(&handoff));
        let drain = ManagementDrain(Arc::clone(&handoff));
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            // Task creation can panic after enqueue. Both consumers hold the
            // same handoff, so fallback waits for a concurrent drain or takes
            // it once, preserving its concrete result for finish/Drop.
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runtime.spawn_blocking(move || drop(drain))
            })) {
                Ok(task) => self.drain = Some(task),
                Err(panic) => {
                    drop(ManagementDrain(handoff));
                    let message = panic
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| panic.downcast_ref::<&str>().copied());
                    tracing::error!(
                        message,
                        "management shutdown enqueue panicked; writers drained synchronously"
                    );
                }
            }
        } else {
            drop(drain);
        }
    }
    pub(crate) async fn finish(&mut self) -> io::Result<()> {
        self.begin();
        let smb_result = self.smb.finish().await;
        if let Some(drain) = &mut self.drain {
            drain.await.map_err(io::Error::other)?;
        }
        self.drain = None;
        if let Some(completion) = self.completion.take() {
            completion
                .lock()
                .expect("invariant: completed management drain is not poisoned")
                .completion
                .take()
                .expect("invariant: completed drain owns its result")
                .0
                .take()
                .expect("invariant: completed drain result is not consumed")
                .map_err(io::Error::other)?;
        }
        smb_result
    }
}
impl Drop for ManagementShutdown {
    fn drop(&mut self) {
        self.begin();
    }
}

#[cfg(test)]
pub(crate) async fn run<H, Http, Bridge, Shutdown>(
    http: H,
    bridge: Bridge,
    shutdown: Shutdown,
) -> io::Result<()>
where
    H: FnOnce(oneshot::Receiver<()>) -> Http,
    Http: Future<Output = io::Result<()>>,
    Bridge: Future<Output = ()>,
    Shutdown: Future<Output = ()>,
{
    run_with_management(
        http,
        bridge,
        shutdown,
        ManagementShutdown {
            backend: None,
            smb: crate::smb_rpc::ConnectionShutdown::new(),
            drain: None,
            completion: None,
        },
    )
    .await
}

pub(crate) async fn run_with_management<H, Http, Bridge, Shutdown>(
    http: H,
    bridge: Bridge,
    shutdown: Shutdown,
    mut management: ManagementShutdown,
) -> io::Result<()>
where
    H: FnOnce(oneshot::Receiver<()>) -> Http,
    Http: Future<Output = io::Result<()>>,
    Bridge: Future<Output = ()>,
    Shutdown: Future<Output = ()>,
{
    // Closing this sender also requests HTTP shutdown if the caller cancels us.
    // The HTTP library owns its connection tasks and signal waiter separately.
    let (stop_http, stopped) = oneshot::channel();
    let mut http = Box::pin(http(stopped));
    // Do not spawn/detach this listener: dropping its future closes admission,
    // shuts down connected SMB sockets, and removes its owned socket path.
    let mut bridge = Box::pin(bridge);
    let mut bridge_finished = false;
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => {
                // Both protocols stop on the same signal, not after HTTP's
                // potentially long drain. Actual blocking operations retain
                // their permits and durability/recovery ownership until done.
                drop(stop_http);
                drop(bridge);
                management.begin();
                tracing::info!("shutdown requested; admission closed, draining admitted work");
                return drain(http, &mut management).await;
            }
            result = &mut http => {
                drop(bridge);
                management.begin();
                let drained = management.finish().await;
                return combine_shutdown_results(result, drained);
            },
            _ = &mut bridge, if !bridge_finished => {
                // Optional bridge failure must not take management HTTP down.
                bridge_finished = true;
            }
        }
    }
}

#[derive(Debug, error_definition::Error)]
#[error("HTTP shutdown failed: {http}; management shutdown also failed: {management}")]
struct HttpAndManagementError {
    #[source]
    http: io::Error,
    management: io::Error,
}

fn combine_shutdown_results(http: io::Result<()>, management: io::Result<()>) -> io::Result<()> {
    match (http, management) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(http), Err(management)) => Err(io::Error::other(HttpAndManagementError {
            http,
            management,
        })),
    }
}

async fn drain<Http>(http: Http, management: &mut ManagementShutdown) -> io::Result<()>
where
    Http: Future<Output = io::Result<()>>,
{
    let started = std::time::Instant::now();
    let work = async {
        let result = http.await;
        let drained = management.finish().await;
        combine_shutdown_results(result, drained)
    };
    tokio::pin!(work);
    loop {
        tokio::select! {
            result = &mut work => return result,
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                tracing::warn!(elapsed_seconds = started.elapsed().as_secs(),
                    "shutdown still awaiting admitted work; no durability operations were force-cancelled");
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/service_lifecycle.rs"]
mod tests;
