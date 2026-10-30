use crate::integrity_scrub::{IntegrityScrubPolicy, IntegrityScrubTrigger};
use std::time::{Duration, Instant};

/// One monotonic cadence, with no pending ticks while the worker is occupied.
#[derive(Default)]
pub(super) struct Cadence {
    anchor: Option<Instant>,
    awaiting_storage: bool,
}

impl Cadence {
    pub(super) fn wait(&self, policy: IntegrityScrubPolicy, now: Instant) -> Option<Duration> {
        policy.interval().map(|interval| {
            let interval = if self.awaiting_storage {
                interval.min(Duration::from_secs(5))
            } else {
                interval
            };
            self.anchor.map_or(Duration::ZERO, |anchor| {
                interval.saturating_sub(now.duration_since(anchor))
            })
        })
    }

    pub(super) fn dispatch(&mut self, trigger: IntegrityScrubTrigger, now: Instant) {
        if matches!(trigger, IntegrityScrubTrigger::Manual)
            || self.anchor.is_none()
            || self.awaiting_storage
        {
            self.anchor = Some(now);
        }
        self.awaiting_storage = false;
    }

    pub(super) fn storage_unavailable(&mut self, now: Instant) {
        self.anchor = Some(now);
        self.awaiting_storage = true;
    }

    pub(super) fn finish(&mut self, policy: IntegrityScrubPolicy, healthy: bool, now: Instant) {
        self.anchor = Some(if healthy && let Some(interval) = policy.interval() {
            let anchor = self
                .anchor
                .expect("invariant: a completed scrub was dispatched");
            let elapsed = now.duration_since(anchor);
            // Policy intervals are positive whole seconds. Retain the most
            // recent tick, so even an exactly due tick is skipped at finish.
            // Subtraction avoids constructing a far-future Instant deadline.
            let remainder = Duration::new(
                elapsed.as_secs() % interval.as_secs(),
                elapsed.subsec_nanos(),
            );
            now - remainder
        } else {
            // Rejections, incomplete scans and cancellations retain a full
            // retry interval instead of catching up immediately after failure.
            now
        });
        self.awaiting_storage = false;
    }
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/management/integrity_scrub/cadence_tests.rs"]
mod tests;
