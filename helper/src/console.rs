//! Independent read-only HTTP owner. No Engine or disk writer is opened here.
mod boundary;
mod http;
mod work;

use crate::Result;
use command_line::Args;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::{Semaphore, watch};

const DEFAULT_PORT: u16 = 9040;
pub(super) const MAX_REQUEST_BYTES: usize = 64 * 1024;
const WARNING: &str = "WARNING: unauthenticated root helper console over plain HTTP. Any reachable person can inspect/export selected disk data or stop this helper. Use only a trusted isolated network/firewall; exit after use. This program does not change your firewall, install systemd or enable autostart.";

#[derive(Args, Clone, Debug)]
pub(crate) struct Options {
    /// Unauthenticated HTTP listener. Open using its literal IP, not a DNS name.
    #[arg(long, default_value = "0.0.0.0:9040")]
    listen: SocketAddr,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
        }
    }
}

#[derive(Clone)]
struct State {
    listen: SocketAddr,
    csrf: Arc<str>,
    jobs: Arc<Semaphore>,
    stopping: watch::Sender<bool>,
}
impl State {
    fn new(listen: SocketAddr, csrf: String, stopping: watch::Sender<bool>) -> Self {
        Self {
            listen,
            csrf: csrf.into(),
            jobs: Arc::new(Semaphore::new(1)),
            stopping,
        }
    }
}

fn require_root(uid: u32) -> Result<()> {
    if uid != 0 {
        return Err(
            "the helper Web console must be run manually as root; no service was started".into(),
        );
    }
    Ok(())
}

pub(crate) fn run(options: Options) -> Result<()> {
    require_root(rustix::process::geteuid().as_raw())?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()?;
    // Dropping this runtime waits for the blocking read owner even if an HTTP
    // request was disconnected. A stuck device is not falsely reported drained.
    runtime.block_on(serve(options))
}

async fn serve(options: Options) -> Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let csrf = storage_engine::OperationId::generate()?.to_string();
    let listener = tokio::net::TcpListener::bind(options.listen).await?;
    let listen = listener.local_addr()?;
    let (stopping, mut stopped) = watch::channel(false);
    let state = State::new(listen, csrf, stopping.clone());
    eprintln!("zxs-helper: {WARNING}");
    eprintln!(
        "zxs-helper: listening on {listen}; open http://<host-IP>:{} . Read-only independent-disk checks/downloads only. Stop all source writers first; no whole-service exclusion has been established.",
        listen.port()
    );
    http_routing::serve(listener, http::router(state))
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = interrupt.recv() => {},
                _ = terminate.recv() => {},
                _ = stopped.changed() => {},
            }
            stopping.send_replace(true);
            eprintln!("zxs-helper: stopping admission; waiting for active requests and disk reads. Do not remove source media until the process exits.");
        })
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "../../test/helper/unit/console/tests.rs"]
mod tests;
