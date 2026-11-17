//! Exactly the two optional native startup jobs for this network server, not
//! a task registry. Waiting for service shutdown must not depend on tearing
//! down the caller's Tokio runtime.
use tokio::{sync::oneshot, task::JoinError};

#[derive(Default)]
pub(crate) struct StartupTail(Option<oneshot::Sender<()>>);

pub(crate) struct StartupTails([oneshot::Receiver<()>; 2]);

impl StartupTails {
    pub(crate) fn network() -> (Self, StartupTail, StartupTail) {
        let (http, http_done) = oneshot::channel();
        let (smb, smb_done) = oneshot::channel();
        (
            Self([http_done, smb_done]),
            StartupTail(Some(http)),
            StartupTail(Some(smb)),
        )
    }

    /// The service first discards its optional publishers/listener. Only work
    /// already admitted can remain; an unpolled startup releases its tail on drop.
    pub(crate) async fn join<T>(self, service: impl std::future::Future<Output = T>) -> T {
        let result = service.await;
        for done in self.0 {
            let _ = done.await;
        }
        result
    }
}

impl StartupTail {
    pub(crate) async fn blocking<T: Send + 'static>(
        self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, JoinError> {
        // Field order is intentional. If the service dropped the async waiter,
        // a completed but undelivered result must release its acquired spool
        // lock/socket before shutdown is notified. Sending on closure exit is
        // too early: Tokio can still hold that result.
        struct Output<T> {
            value: T,
            _tail: StartupTail,
        }
        let output = tokio::task::spawn_blocking(move || Output {
            value: work(),
            _tail: self,
        })
        .await?;
        Ok(output.value)
    }
}

impl Drop for StartupTail {
    fn drop(&mut self) {
        if let Some(done) = self.0.take() {
            // If the entire service future was cancelled there is no normal
            // completion receiver. The job still owns its resource on Tokio.
            let _ = done.send(());
        }
    }
}
