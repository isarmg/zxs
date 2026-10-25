//! Install signal handlers before startup can perform blocking filesystem work.
use futures::FutureExt;
use std::io;
use tokio::signal::unix::{Signal, SignalKind, signal};

pub struct ShutdownSignal {
    interrupt: Signal,
    terminate: Signal,
    requested: bool,
}

impl ShutdownSignal {
    pub fn install() -> io::Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            requested: false,
        })
    }

    pub(crate) async fn wait(&mut self) {
        if !self.requested {
            tokio::select! {
                _ = self.interrupt.recv() => {},
                _ = self.terminate.recv() => {},
            }
            self.requested = true;
        }
    }

    pub(crate) fn observed(&mut self) -> bool {
        // Snapshot only signals already delivered by the runtime. Installing
        // handlers early retains startup signals, but OS delivery and reactor
        // dispatch are asynchronous: a pending signal can become visible later.
        // The service must continue polling wait(), even after this returns false.
        self.wait().now_or_never().is_some()
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/shutdown.rs"]
mod tests;
