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

/// The original run admission, not a reference to whichever receipt is newest.
/// Its metadata epoch is retained alongside it in the private plan/outcome.
pub(in crate::engine) struct ScheduledScrub {
    owner: Weak<ScheduleState>,
    admission: Arc<()>,
}

impl ScheduledScrub {
    pub(super) fn revoke(&self, epoch: &MetadataEpoch) {
        let Some(state) = self.owner.upgrade() else {
            return;
        };
        let mut observation = state
            .observation
            .lock()
            .expect("invariant: scrub schedule lock poisoned");
        if observation.active
            && Arc::ptr_eq(&observation.admission, &self.admission)
            && epoch.matches(&state.integrity)
        {
            observation.admission = Arc::new(());
            observation.coverage = None;
        }
    }
}

struct Observation {
    active: bool,
    coverage: Option<Arc<Coverage>>,
    admission: Arc<()>,
}

struct Coverage {
    baseline: Weak<State>,
    sequence: u64,
    snapshot: Option<String>,
    epoch: MetadataEpoch,
    started: Instant,
    completed: Instant,
}

/// A captured receipt retains its original deadlines, never a mutable health bit.
pub(in crate::engine) struct ScrubCoveragePermit {
    schedule: Weak<ScheduleState>,
    coverage: Arc<Coverage>,
    admission: Arc<()>,
}

impl ScrubCoveragePermit {
    /// Lock order is schedule -> metadata generation, as in capture/finish.
    /// Success consumes this permit at the irreversible decision-admission
    /// point. Later stop/fault revokes future decisions, not this started one.
    pub(in crate::engine) fn admit(
        self,
        context: &crate::engine::PlanningContext<'_>,
        epoch: &MetadataEpoch,
    ) -> bool {
        let Some(state) = self.schedule.upgrade() else {
            return false;
        };
        if !context.owns_scrub_schedule(&state) {
            return false;
        }
        let observation = state
            .observation
            .lock()
            .expect("invariant: scrub schedule lock poisoned");
        epoch.admits(context.metadata_integrity, || {
            observation.active
                && Arc::ptr_eq(&observation.admission, &self.admission)
                && self.coverage.epoch.same_generation(epoch)
                && self
                    .coverage
                    .matches_baseline(context, &state, Instant::now())
        })
    }

    pub(in crate::engine) fn matches(&self, context: &crate::engine::PlanningContext<'_>) -> bool {
        let Some(state) = self.schedule.upgrade() else {
            return false;
        };
        if !context.owns_scrub_schedule(&state) {
            return false;
        }
        let observation = state
            .observation
            .lock()
            .expect("invariant: scrub schedule lock poisoned");
        observation.active
            && Arc::ptr_eq(&observation.admission, &self.admission)
            && self.coverage.matches(context, &state, Instant::now())
    }
}

impl Coverage {
    fn matches(
        &self,
        context: &crate::engine::PlanningContext<'_>,
        schedule: &ScheduleState,
        now: Instant,
    ) -> bool {
        self.epoch.matches(context.metadata_integrity)
            && self.matches_baseline(context, schedule, now)
    }

    fn matches_baseline(
        &self,
        context: &crate::engine::PlanningContext<'_>,
        schedule: &ScheduleState,
        now: Instant,
    ) -> bool {
        !context.blocked
            && now.duration_since(self.started) < schedule.max_age
            && now.duration_since(self.completed) < schedule.interval
            && self.sequence <= context.state.sequence
            && (self.sequence != context.state.sequence
                || (self.baseline.ptr_eq(&Arc::downgrade(context.state))
                    && &self.snapshot == context.snapshot))
    }
}

impl crate::engine::PlanningContext<'_> {
    fn owns_scrub_schedule(&self, state: &Arc<ScheduleState>) -> bool {
        self.integrity_scrub_schedule
            .is_some_and(|current| Weak::ptr_eq(current, &Arc::downgrade(state)))
            && Arc::ptr_eq(self.metadata_integrity, &state.integrity)
            && self.config.same_admission(&state.config)
    }

    pub(in crate::engine) fn capture_scrub_coverage(&self) -> Option<ScrubCoveragePermit> {
        let schedule = self.integrity_scrub_schedule?;
        let state = schedule.upgrade()?;
        if !self.owns_scrub_schedule(&state) {
            return None;
        }
        let observation = state
            .observation
            .lock()
            .expect("invariant: scrub schedule lock poisoned");
        let coverage = observation.coverage.as_ref()?;
        (observation.active && coverage.matches(self, &state, Instant::now())).then(|| {
            ScrubCoveragePermit {
                schedule: schedule.clone(),
                coverage: Arc::clone(coverage),
                admission: Arc::clone(&observation.admission),
            }
        })
    }
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
                admission: Arc::new(()),
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
        let observation = schedule
            .state
            .observation
            .lock()
            .expect("invariant: scrub schedule lock poisoned");
        if !self.owns_scrub_schedule(&schedule.state) || !observation.active {
            return Err(Error::new(
                ErrorCode::PreparationStale,
                "integrity scrub schedule is no longer owned by this Engine",
            ));
        }
        let mut plan = self.prepare_integrity_scrub()?;
        plan.schedule = Some(ScheduledScrub {
            owner: Arc::downgrade(&schedule.state),
            admission: Arc::clone(&observation.admission),
        });
        Ok(plan)
    }

    fn owns_scrub_schedule(&self, state: &Arc<ScheduleState>) -> bool {
        self.planning_context().owns_scrub_schedule(state)
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
            && observation
                .coverage
                .as_ref()
                .is_some_and(|coverage| coverage.matches(&self.planning_context(), &state, now))
    }

    pub(super) fn finish_scrub_schedule(&self, outcome: &IntegrityScrubOutcome) {
        let Some(run) = &outcome.schedule else {
            return;
        };
        let Some(state) = run.owner.upgrade() else {
            return;
        };
        if !self.owns_scrub_schedule(&state) {
            return;
        }
        let mut observation = state
            .observation
            .lock()
            .expect("invariant: scrub schedule lock poisoned");
        if !observation.active
            || !Arc::ptr_eq(&observation.admission, &run.admission)
            || !outcome.metadata_epoch.matches(&self.metadata_integrity)
        {
            return;
        }
        // All per-member graph faults have already revoked this exact epoch,
        // including failed sources hidden by a later healthy-copy fallback.
        // Local version/proof/body availability is deliberately not graph authority.
        let coverage = (!self.blocked
            && matches!(
                outcome.report.completion,
                IntegrityScrubCompletion::Complete
            )
            && outcome.report.metadata_members_checked == self.config.metadata_members().len())
        .then(|| {
            Arc::new(Coverage {
                baseline: Arc::downgrade(&outcome.baseline),
                sequence: outcome.baseline.sequence,
                snapshot: outcome.report.snapshot_hash.clone(),
                epoch: outcome.metadata_epoch.clone(),
                started: outcome.started,
                completed: Instant::now(),
            })
        });
        if coverage.is_none() {
            // Healthy refreshes preserve still-fresh original receipts. Failed
            // runs revoke old admissions permanently, even after a later success.
            observation.admission = Arc::new(());
        }
        observation.coverage = coverage;
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/integrity_scrub/schedule_tests.rs"]
mod tests;
