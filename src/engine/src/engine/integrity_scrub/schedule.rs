//! A managed owner's finite coverage lease. Registration alone proves nothing.
use super::{IntegrityScrubCompletion, IntegrityScrubOutcome, IntegrityScrubPlan};
use crate::engine::provenance::{MetadataEpoch, MetadataIntegrity};
use crate::{Engine, Error, ErrorCode, Result, State, ValidatedConfig};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// Hold this in the actual periodic worker, and revoke it before stopping that
/// worker. Neither configuration nor a caller-created report can grant coverage.
pub struct IntegrityScrubSchedule {
    state: Arc<ScheduleState>,
}

pub(in crate::engine) struct ScheduleState {
    integrity: Arc<MetadataIntegrity>,
    config: ValidatedConfig,
    interval: Duration,
    max_age: Duration,
    observation: Mutex<Observation>,
}

struct Observation {
    active: bool,
    coverage: Option<Coverage>,
}

struct Coverage {
    baseline: Weak<State>,
    sequence: u64,
    snapshot: Option<String>,
    epoch: MetadataEpoch,
    started: Instant,
    completed: Instant,
}

impl IntegrityScrubSchedule {
    /// One-way revocation. A stopped owner must register a new lease and execute
    /// a new complete scan, never reactivate a prior success or heartbeat.
    pub fn revoke(&self) {
        let mut observation = match self.state.observation.lock() {
            Ok(observation) => observation,
            // Shutdown must revoke even after a worker panic; never reopen admission.
            Err(poisoned) => poisoned.into_inner(),
        };
        observation.active = false;
        observation.coverage = None;
    }
}

impl Drop for IntegrityScrubSchedule {
    fn drop(&mut self) {
        self.revoke();
    }
}

impl Engine {
    /// Register one continuous periodic owner. No coverage exists until its
    /// private scan outcome passes the live-owner finish boundary.
    pub fn begin_integrity_scrub_schedule(
        &mut self,
        interval: Duration,
        max_age: Duration,
    ) -> Result<IntegrityScrubSchedule> {
        if interval.is_zero() || max_age < interval {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "integrity scrub freshness must cover a nonzero scheduling interval",
            ));
        }
        if let Some(current) = self
            .integrity_scrub_schedule
            .as_ref()
            .and_then(Weak::upgrade)
            && current
                .observation
                .lock()
                .expect("invariant: scrub schedule lock poisoned")
                .active
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "an integrity scrub schedule already owns this Engine",
            ));
        }
        let state = Arc::new(ScheduleState {
            integrity: Arc::clone(&self.metadata_integrity),
            config: self.config.clone(),
            interval,
            max_age,
            observation: Mutex::new(Observation {
                active: true,
                coverage: None,
            }),
        });
        self.integrity_scrub_schedule = Some(Arc::downgrade(&state));
        Ok(IntegrityScrubSchedule { state })
    }

    /// Bind an actual scan to its owning periodic worker. Manual standalone scans
    /// remain observations and cannot establish a future scheduling obligation.
    pub fn prepare_scheduled_integrity_scrub(
        &self,
        schedule: &IntegrityScrubSchedule,
    ) -> Result<IntegrityScrubPlan> {
        if !self.owns_scrub_schedule(&schedule.state)
            || !schedule
                .state
                .observation
                .lock()
                .expect("invariant: scrub schedule lock poisoned")
                .active
        {
            return Err(Error::new(
                ErrorCode::PreparationStale,
                "integrity scrub schedule is no longer owned by this Engine",
            ));
        }
        let mut plan = self.prepare_integrity_scrub()?;
        plan.schedule = Some(Arc::downgrade(&schedule.state));
        Ok(plan)
    }

    fn owns_scrub_schedule(&self, state: &Arc<ScheduleState>) -> bool {
        self.integrity_scrub_schedule
            .as_ref()
            .is_some_and(|current| Weak::ptr_eq(current, &Arc::downgrade(state)))
            && Arc::ptr_eq(&self.metadata_integrity, &state.integrity)
            && self.config.same_admission(&state.config)
    }

    /// Coverage is for the captured retained graph in this continuous writer,
    /// not a claim that later publications or every payload is currently healthy.
    /// A missed next interval, stale scan start, fault or owner loss requires full
    /// publication validation. Normal publication never refreshes these clocks.
    pub fn has_current_integrity_scrub_coverage(&self) -> bool {
        self.scrub_coverage_at(Instant::now())
    }

    fn scrub_coverage_at(&self, now: Instant) -> bool {
        let Some(state) = self
            .integrity_scrub_schedule
            .as_ref()
            .and_then(Weak::upgrade)
        else {
            return false;
        };
        if self.blocked || !self.owns_scrub_schedule(&state) {
            return false;
        }
        let observation = state
            .observation
            .lock()
            .expect("invariant: scrub schedule lock poisoned");
        observation.active
            && observation.coverage.as_ref().is_some_and(|coverage| {
                coverage.epoch.matches(&self.metadata_integrity)
                    && now.duration_since(coverage.started) < state.max_age
                    && now.duration_since(coverage.completed) < state.interval
                    && coverage.sequence <= self.state.sequence
                    && (coverage.sequence != self.state.sequence
                        || (coverage.baseline.ptr_eq(&Arc::downgrade(&self.state))
                            && coverage.snapshot == self.snapshot))
            })
    }

    pub(super) fn finish_scrub_schedule(&self, outcome: &IntegrityScrubOutcome) {
        let Some(state) = outcome.schedule.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        if !self.owns_scrub_schedule(&state) {
            return;
        }
        let mut observation = state
            .observation
            .lock()
            .expect("invariant: scrub schedule lock poisoned");
        if !observation.active {
            return;
        }
        // All per-member graph faults have already revoked this exact epoch,
        // including failed sources hidden by a later healthy-copy fallback.
        // Local version/proof/body availability is deliberately not graph authority.
        observation.coverage = (!self.blocked
            && matches!(
                outcome.report.completion,
                IntegrityScrubCompletion::Complete
            )
            && outcome.report.metadata_members_checked == self.config.metadata_members().len()
            && outcome.metadata_epoch.matches(&self.metadata_integrity))
        .then(|| Coverage {
            baseline: Arc::downgrade(&outcome.baseline),
            sequence: outcome.baseline.sequence,
            snapshot: outcome.report.snapshot_hash.clone(),
            epoch: outcome.metadata_epoch.clone(),
            started: outcome.started,
            completed: Instant::now(),
        });
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/integrity_scrub/schedule_tests.rs"]
mod tests;
