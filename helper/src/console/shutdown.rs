//! The shutdown decision owns the optional service handoff. A stop-only request
//! and a late activation cannot both win, even if their HTTP requests overlap.
use tokio::sync::watch;

pub(super) enum Decision<H> {
    Running,
    StopOnly,
    Handoff(H),
}

pub(super) struct Shutdown<H> {
    decision: watch::Sender<Decision<H>>,
}

impl<H> Shutdown<H> {
    pub(super) fn new() -> Self {
        Self {
            decision: watch::channel(Decision::Running).0,
        }
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<Decision<H>> {
        self.decision.subscribe()
    }

    pub(super) fn stopping(&self) -> bool {
        !matches!(*self.decision.borrow(), Decision::Running)
    }

    /// Only the immediate, nonblocking permit acquisition belongs inside this
    /// read guard. Never hold it while reading a request or working on a disk.
    pub(super) fn admit<R>(&self, acquire: impl FnOnce() -> R) -> Option<R> {
        let decision = self.decision.borrow();
        match &*decision {
            Decision::Running => Some(acquire()),
            Decision::StopOnly | Decision::Handoff(_) => None,
        }
    }

    fn begin(&self, requested: Decision<H>) -> bool {
        self.decision.send_if_modified(move |decision| match decision {
            Decision::Running => {
                *decision = requested;
                true
            }
            Decision::StopOnly | Decision::Handoff(_) => false,
        })
    }

    pub(super) fn stop(&self) {
        self.begin(Decision::StopOnly);
    }

    pub(super) fn queue(&self, handoff: H) -> bool {
        self.begin(Decision::Handoff(handoff))
    }

    /// Closing the owner cannot reopen admission, and transfers at most once.
    pub(super) fn take_after_drain(&self) -> Option<H> {
        match self.decision.send_replace(Decision::StopOnly) {
            Decision::Handoff(handoff) => Some(handoff),
            Decision::Running | Decision::StopOnly => None,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/helper/unit/console/shutdown_tests.rs"]
mod tests;
