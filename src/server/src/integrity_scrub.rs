//! Managed full-integrity jobs. These are interval observations, not write permits.
mod report;
mod wire;
use crate::ApiError;
use std::{
    num::NonZeroU64,
    sync::Arc,
    time::{Duration, SystemTime},
};
use storage_engine::IntegrityScrubReport;
pub(crate) use wire::StatusResponse;

/// Validated host policy; no HTTP request can change the background budget.
#[derive(Clone, Copy, Debug)]
pub struct IntegrityScrubPolicy {
    interval: Option<Duration>,
    max_age: Duration,
    bytes_per_second: NonZeroU64,
}
#[derive(Debug, error_definition::Error)]
#[error(
    "scrub maximum report age ({max_age_seconds}s) must not precede its interval ({interval_seconds}s)"
)]
pub struct InvalidScrubPolicy {
    interval_seconds: u64,
    max_age_seconds: u64,
}
impl IntegrityScrubPolicy {
    /// Enable non-overlapping periodic scans with a finite read rate.
    ///
    /// # Errors
    /// Rejects a maximum observation age shorter than the scheduling interval.
    pub fn periodic(
        interval_seconds: NonZeroU64,
        max_age_seconds: NonZeroU64,
        bytes_per_second: NonZeroU64,
    ) -> Result<Self, InvalidScrubPolicy> {
        if max_age_seconds < interval_seconds {
            return Err(InvalidScrubPolicy {
                interval_seconds: interval_seconds.get(),
                max_age_seconds: max_age_seconds.get(),
            });
        }
        Ok(Self {
            interval: Some(Duration::from_secs(interval_seconds.get())),
            max_age: Duration::from_secs(max_age_seconds.get()),
            bytes_per_second,
        })
    }
    /// Keep only explicit manual dispatch; use the same read and freshness budgets.
    pub fn manual(max_age_seconds: NonZeroU64, bytes_per_second: NonZeroU64) -> Self {
        Self {
            interval: None,
            max_age: Duration::from_secs(max_age_seconds.get()),
            bytes_per_second,
        }
    }
    pub(crate) fn interval(self) -> Option<Duration> {
        self.interval
    }
    pub(crate) fn max_age(self) -> Duration {
        self.max_age
    }
    pub(crate) fn read_duration(self, bytes: usize) -> Duration {
        // One bounded read charge, not an accumulating lifetime byte counter.
        let nanos =
            (bytes as u128 * 1_000_000_000).div_ceil(u128::from(self.bytes_per_second.get()));
        Duration::new(
            (nanos / 1_000_000_000)
                .try_into()
                .expect("invariant: one read charge fits a u64 duration"),
            (nanos % 1_000_000_000) as u32,
        )
    }
}
impl Default for IntegrityScrubPolicy {
    fn default() -> Self {
        // Embedders opt in to a schedule. The daemon supplies its periodic policy.
        Self::manual(
            NonZeroU64::new(172_800).expect("invariant: positive freshness"),
            NonZeroU64::new(67_108_864).expect("invariant: positive I/O rate"),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntegrityScrubJobId(pub(crate) u64);
impl IntegrityScrubJobId {
    pub(crate) fn parse(value: &str) -> Result<Self, ApiError> {
        storage_contract::parse_u64_decimal(value)
            .map(Self)
            .map_err(ApiError::invalid_source)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrityScrubTrigger {
    Manual,
    Periodic,
}
#[derive(Clone, Debug)]
pub struct IntegrityScrubBaseline {
    pub sequence: u64,
    pub snapshot_hash: Option<String>,
}
#[derive(Clone, Debug)]
pub enum IntegrityScrubPhase {
    NotReady,
    Idle,
    Queued {
        job_id: IntegrityScrubJobId,
        cancellation_requested: bool,
    },
    Running {
        job_id: IntegrityScrubJobId,
        baseline: IntegrityScrubBaseline,
        started_at: SystemTime,
        bytes_read: u128,
        expected_payload_bytes: u64,
        cancellation_requested: bool,
    },
    Stopping,
    SchedulerUnavailable(Arc<ApiError>),
}
#[derive(Debug)]
pub enum IntegrityScrubOutcome {
    Report(Box<IntegrityScrubReport>),
    Rejected(ApiError),
}
#[derive(Debug)]
pub struct IntegrityScrubRun {
    pub job_id: IntegrityScrubJobId,
    pub trigger: IntegrityScrubTrigger,
    pub outcome: IntegrityScrubOutcome,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrityScrubFreshness {
    NotChecked,
    Fresh,
    Expired,
    Unavailable,
}
#[derive(Clone, Debug)]
pub struct IntegrityScrubStatus {
    pub phase: IntegrityScrubPhase,
    pub latest: Option<Arc<IntegrityScrubRun>>,
    pub last_complete_success_at: Option<SystemTime>,
    pub freshness: IntegrityScrubFreshness,
}

pub(crate) enum ScrubAdmission {
    Manual(crate::auth::Identity),
    Periodic,
}
/// The real scan admission: a scheduled scan must carry its live owner's lease.
pub(crate) enum ScrubExecution<'a> {
    Standalone(&'a crate::auth::Identity),
    Scheduled {
        identity: Option<&'a crate::auth::Identity>,
        owner: &'a storage_engine::IntegrityScrubSchedule,
    },
}
impl ScrubAdmission {
    pub(crate) fn trigger(&self) -> IntegrityScrubTrigger {
        match self {
            Self::Manual(_) => IntegrityScrubTrigger::Manual,
            Self::Periodic => IntegrityScrubTrigger::Periodic,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/integrity_scrub/policy_tests.rs"]
mod tests;
