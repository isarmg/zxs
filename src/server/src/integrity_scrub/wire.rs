use super::*;
use data_serialization::{Serialize, Serializer};

pub(crate) struct StatusResponse(IntegrityScrubStatus);
impl From<IntegrityScrubStatus> for StatusResponse {
    fn from(status: IntegrityScrubStatus) -> Self {
        Self(status)
    }
}
impl Serialize for StatusResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let IntegrityScrubStatus {
            phase,
            latest,
            last_complete_success_at,
            freshness,
        } = &self.0;
        Status {
            phase: Phase::from(phase),
            latest: latest.as_ref().map(|run| Run::from(run.as_ref())),
            last_complete_success_at: *last_complete_success_at,
            freshness: match freshness {
                IntegrityScrubFreshness::NotChecked => "not_checked",
                IntegrityScrubFreshness::Fresh => "fresh",
                IntegrityScrubFreshness::Expired => "expired",
                IntegrityScrubFreshness::Unavailable => "unavailable",
            },
        }
        .serialize(serializer)
    }
}
#[derive(Serialize)]
struct Status<'a> {
    phase: Phase<'a>,
    latest: Option<Run<'a>>,
    #[serialization(serialize_with = "optional_timestamp")]
    last_complete_success_at: Option<SystemTime>,
    freshness: &'static str,
}
#[derive(Serialize)]
#[serialization(tag = "state", rename_all = "snake_case")]
enum Phase<'a> {
    NotReady,
    Idle,
    Queued {
        #[serialization(with = "storage_contract::u64_string")]
        job_id: u64,
        cancellation_requested: bool,
    },
    Running {
        #[serialization(with = "storage_contract::u64_string")]
        job_id: u64,
        baseline: Baseline<'a>,
        #[serialization(serialize_with = "timestamp")]
        started_at: SystemTime,
        bytes_read: String,
        #[serialization(with = "storage_contract::u64_string")]
        expected_payload_bytes: u64,
        cancellation_requested: bool,
    },
    Stopping,
    SchedulerUnavailable {
        error: &'a ApiError,
    },
}
#[derive(Serialize)]
struct Baseline<'a> {
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    snapshot_hash: Option<&'a str>,
}
impl<'a> From<&'a IntegrityScrubPhase> for Phase<'a> {
    fn from(phase: &'a IntegrityScrubPhase) -> Self {
        match phase {
            IntegrityScrubPhase::NotReady => Self::NotReady,
            IntegrityScrubPhase::Idle => Self::Idle,
            IntegrityScrubPhase::Stopping => Self::Stopping,
            IntegrityScrubPhase::Queued {
                job_id,
                cancellation_requested,
            } => Self::Queued {
                job_id: job_id.0,
                cancellation_requested: *cancellation_requested,
            },
            IntegrityScrubPhase::Running {
                job_id,
                baseline,
                started_at,
                bytes_read,
                expected_payload_bytes,
                cancellation_requested,
            } => Self::Running {
                job_id: job_id.0,
                baseline: Baseline {
                    sequence: baseline.sequence,
                    snapshot_hash: baseline.snapshot_hash.as_deref(),
                },
                started_at: *started_at,
                bytes_read: bytes_read.to_string(),
                expected_payload_bytes: *expected_payload_bytes,
                cancellation_requested: *cancellation_requested,
            },
            IntegrityScrubPhase::SchedulerUnavailable(error) => {
                Self::SchedulerUnavailable { error }
            }
        }
    }
}
#[derive(Serialize)]
struct Run<'a> {
    #[serialization(with = "storage_contract::u64_string")]
    job_id: u64,
    trigger: &'static str,
    outcome: Outcome<'a>,
    diagnostics: Diagnostics,
}

#[derive(Serialize)]
struct Diagnostics {
    bytes_read: String,
    scan_elapsed_millis: Option<String>,
    expected_payload_bytes: Option<String>,
    policy: Policy,
    metadata_coverage_at_finish: Option<bool>,
    feasibility: &'static str,
}

#[derive(Serialize)]
struct Policy {
    interval_seconds: Option<String>,
    max_age_seconds: String,
    bytes_per_second: String,
}

impl From<&IntegrityScrubDiagnostics> for Diagnostics {
    fn from(diagnostics: &IntegrityScrubDiagnostics) -> Self {
        let IntegrityScrubDiagnostics {
            bytes_read,
            scan_elapsed,
            expected_payload_bytes,
            policy,
            metadata_coverage_at_finish,
        } = diagnostics;
        Self {
            bytes_read: bytes_read.to_string(),
            scan_elapsed_millis: scan_elapsed.map(|elapsed| elapsed.as_millis().to_string()),
            expected_payload_bytes: expected_payload_bytes.map(|bytes| bytes.to_string()),
            policy: Policy {
                interval_seconds: policy
                    .interval
                    .map(|interval| interval.as_secs().to_string()),
                max_age_seconds: policy.max_age.as_secs().to_string(),
                bytes_per_second: policy.bytes_per_second.to_string(),
            },
            metadata_coverage_at_finish: *metadata_coverage_at_finish,
            feasibility: match diagnostics.feasibility() {
                IntegrityScrubFeasibility::UnknownScale => "unknown_scale",
                IntegrityScrubFeasibility::UnknownTotal => "unknown_total",
                IntegrityScrubFeasibility::PayloadCannotFitMaxAge => "payload_cannot_fit_max_age",
            },
        }
    }
}
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
enum Outcome<'a> {
    Report {
        #[serialization(serialize_with = "serialize_report")]
        report: &'a storage_engine::IntegrityScrubReport,
    },
    Rejected {
        error: &'a ApiError,
    },
}
impl<'a> From<&'a IntegrityScrubRun> for Run<'a> {
    fn from(run: &'a IntegrityScrubRun) -> Self {
        let IntegrityScrubRun {
            job_id,
            trigger,
            outcome,
            diagnostics,
        } = run;
        Self {
            job_id: job_id.0,
            trigger: match trigger {
                IntegrityScrubTrigger::Manual => "manual",
                IntegrityScrubTrigger::Periodic => "periodic",
            },
            outcome: match outcome {
                IntegrityScrubOutcome::Report(report) => Outcome::Report { report },
                IntegrityScrubOutcome::Rejected(error) => Outcome::Rejected { error },
            },
            diagnostics: Diagnostics::from(diagnostics),
        }
    }
}

fn serialize_report<S: Serializer>(
    report: &storage_engine::IntegrityScrubReport,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    super::report::Report::from(report).serialize(serializer)
}

fn utc(value: &SystemTime) -> Result<String, ApiError> {
    let elapsed = value
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|source| ApiError::io(std::io::Error::other(source)))?;
    crate::backend_activity::utc_timestamp(elapsed.as_secs())
}
pub(super) fn timestamp<S: Serializer>(
    value: &SystemTime,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    utc(value)
        .map_err(data_serialization::ser::Error::custom)?
        .serialize(serializer)
}
fn optional_timestamp<S: Serializer>(
    value: &Option<SystemTime>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value
        .as_ref()
        .map(utc)
        .transpose()
        .map_err(data_serialization::ser::Error::custom)?
        .serialize(serializer)
}
