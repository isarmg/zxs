//! Keep optional listeners owned by the HTTP service, including during drain.
use std::{future::Future, io, sync::Arc};
use tokio::sync::oneshot;

/// Own native management writers even if serve setup fails or its future is
/// cancelled. Blocking joins run on Tokio's tracked blocking pool, whose
/// runtime teardown waits for them instead of silently detaching a writer.
pub(crate) struct ManagementShutdown {
    backend: Option<Arc<dyn crate::Backend>>,
    drain: Option<tokio::task::JoinHandle<Result<(), crate::ApiError>>>,
}
/// Tokio may discard queued non-mandatory blocking jobs during runtime
/// shutdown. The closure must own a drain-on-drop obligation, not just an Arc.
struct NativeDrain(Option<Arc<dyn crate::Backend>>);
impl NativeDrain {
    fn run(mut self) -> Result<(), crate::ApiError> {
        match self.0.take() {
            Some(backend) => backend.drain_management(),
            None => Ok(()),
        }
    }
}
impl Drop for NativeDrain {
    fn drop(&mut self) {
        if let Some(backend) = self.0.take()
            && let Err(error) = backend.drain_management()
        {
            tracing::error!(%error, "management shutdown failed");
        }
    }
}
impl ManagementShutdown {
    pub(crate) fn new(backend: Arc<dyn crate::Backend>) -> Self {
        Self {
            backend: Some(backend),
            drain: None,
        }
    }
    fn begin(&mut self) {
        let Some(backend) = self.backend.take() else {
            return;
        };
        backend.stop_management();
        let drain = NativeDrain(Some(backend));
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            self.drain = Some(runtime.spawn_blocking(move || drain.run()));
        } else {
            // A guard may be dropped outside a runtime by an embedding caller.
            // Never panic or detach its native writers in that fallback path.
            drop(drain);
        }
    }
    pub(crate) async fn finish(&mut self) -> io::Result<()> {
        self.begin();
        if let Some(drain) = self.drain.take() {
            drain
                .await
                .map_err(io::Error::other)?
                .map_err(io::Error::other)?;
        }
        Ok(())
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
            drain: None,
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
                return result.and(drained);
            },
            _ = &mut bridge, if !bridge_finished => {
                // Optional bridge failure must not take management HTTP down.
                bridge_finished = true;
            }
        }
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
        result.and(drained)
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
