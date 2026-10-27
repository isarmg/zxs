//! Decode the monitoring projection once; no storage or publication authority.
use data_serialization::{Deserialize, Deserializer};

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Status {
    pub phase: Phase,
    pub latest: Option<Run>,
    pub last_complete_success_at: Option<String>,
    pub freshness: Freshness,
}
impl Status {
    pub(super) fn decode(value: json_codec::Value) -> Result<Self, json_codec::Error> {
        let status: Self = json_codec::from_value(value)?;
        if status.freshness == Freshness::Fresh && status.last_complete_success_at.is_none() {
            return Err(data_serialization::de::Error::custom(
                "fresh integrity status requires a complete-success timestamp",
            ));
        }
        Ok(status)
    }
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serialization(rename_all = "snake_case")]
pub(super) enum Freshness {
    NotChecked,
    Fresh,
    Expired,
    Unavailable,
}
#[derive(Clone, Debug, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case")]
pub(super) enum Phase {
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
        baseline: Baseline,
        started_at: String,
        #[serialization(deserialize_with = "decimal_u128")]
        bytes_read: u128,
        #[serialization(with = "storage_contract::u64_string")]
        expected_payload_bytes: u64,
        cancellation_requested: bool,
    },
    Stopping,
    SchedulerUnavailable {
        error: Failure,
    },
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct Baseline {
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub snapshot_hash: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct Run {
    #[serialization(with = "storage_contract::u64_string")]
    pub job_id: u64,
    pub trigger: Trigger,
    pub outcome: Outcome,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub(super) enum Trigger {
    Manual,
    Periodic,
}
#[derive(Clone, Debug, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(super) enum Outcome {
    Report { report: Report },
    Rejected { error: Failure },
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct Report {
    #[serialization(flatten)]
    pub completion: Completion,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub snapshot_hash: Option<String>,
    pub started_at: String,
    pub finished_at: String,
    pub payload_copies_required: u64,
    pub payload_copies_verified: u64,
    pub metadata_failures: u64,
    pub identity_failures: u64,
    pub payload_copies_unavailable: u64,
    pub payload_copies_unverified: u64,
}
#[derive(Clone, Debug, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case")]
pub(super) enum Completion {
    Running,
    CompleteSuccess,
    PartialFailure,
    Cancelled,
    Expired,
    Failed { failure: Failure },
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct Failure {
    pub code: storage_contract::ErrorCode,
    pub message: String,
    pub request_id: String,
}
fn decimal_u128<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
    let text = String::deserialize(deserializer)?;
    if text.is_empty()
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(data_serialization::de::Error::custom(
            "read bytes must be a canonical unsigned decimal string",
        ));
    }
    text.parse().map_err(data_serialization::de::Error::custom)
}
