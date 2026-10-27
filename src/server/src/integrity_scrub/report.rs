//! HTTP projection of the bounded engine report; no Engine owner survives here.
use data_serialization::Serialize;
use std::time::SystemTime;
use storage_engine::{
    IntegrityScrubArea, IntegrityScrubCompletion, IntegrityScrubIssue, IntegrityScrubReport,
    IntegrityScrubTarget,
};

#[derive(Serialize)]
#[serialization(tag = "state", rename_all = "snake_case")]
enum Outcome {
    Running,
    Expired,
    CompleteSuccess,
    PartialFailure,
    Cancelled,
    Failed { failure: crate::ApiError },
}

#[derive(Serialize)]
pub(super) struct Report<'a> {
    #[serialization(flatten)]
    outcome: Outcome,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    snapshot_hash: Option<&'a str>,
    #[serialization(serialize_with = "super::wire::timestamp")]
    started_at: SystemTime,
    #[serialization(serialize_with = "super::wire::timestamp")]
    finished_at: SystemTime,
    metadata_members_checked: usize,
    metadata_publications_verified: usize,
    metadata_objects_verified: usize,
    journal_frames_verified: usize,
    unpublished_journal_records: usize,
    identity_failures: usize,
    metadata_failures: usize,
    payload_copies_required: usize,
    payload_copies_verified: usize,
    payload_copies_unavailable: usize,
    payload_copies_unverified: usize,
    tombstones_verified: usize,
    issues: Vec<Issue<'a>>,
    issues_omitted: usize,
}

impl<'a> From<&'a IntegrityScrubReport> for Report<'a> {
    fn from(report: &'a IntegrityScrubReport) -> Self {
        let IntegrityScrubReport {
            sequence,
            snapshot_hash,
            started_at,
            finished_at,
            completion,
            metadata_members_checked,
            metadata_publications_verified,
            metadata_objects_verified,
            journal_frames_verified,
            unpublished_journal_records,
            identity_failures,
            metadata_failures,
            payload_copies_required,
            payload_copies_verified,
            payload_copies_unavailable,
            payload_copies_unverified,
            tombstones_verified,
            issues,
            issues_omitted,
        } = report;
        let healthy = report.is_healthy();
        let outcome = match completion {
            IntegrityScrubCompletion::Running => Outcome::Running,
            IntegrityScrubCompletion::Superseded => Outcome::Expired,
            IntegrityScrubCompletion::Complete if healthy => Outcome::CompleteSuccess,
            IntegrityScrubCompletion::Complete => Outcome::PartialFailure,
            IntegrityScrubCompletion::Cancelled => Outcome::Cancelled,
            IntegrityScrubCompletion::Failed(error) => Outcome::Failed {
                failure: crate::engine_backend::error_response(error),
            },
        };
        Self {
            outcome,
            sequence: *sequence,
            snapshot_hash: snapshot_hash.as_deref(),
            started_at: *started_at,
            finished_at: *finished_at,
            metadata_members_checked: *metadata_members_checked,
            metadata_publications_verified: *metadata_publications_verified,
            metadata_objects_verified: *metadata_objects_verified,
            journal_frames_verified: *journal_frames_verified,
            unpublished_journal_records: *unpublished_journal_records,
            identity_failures: *identity_failures,
            metadata_failures: *metadata_failures,
            payload_copies_required: *payload_copies_required,
            payload_copies_verified: *payload_copies_verified,
            payload_copies_unavailable: *payload_copies_unavailable,
            payload_copies_unverified: *payload_copies_unverified,
            tombstones_verified: *tombstones_verified,
            issues: issues.iter().map(Issue::from).collect(),
            issues_omitted: *issues_omitted,
        }
    }
}

#[derive(Serialize)]
#[serialization(rename_all = "snake_case")]
enum Area {
    Identity,
    Metadata,
    Payload,
}

#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
enum Target<'a> {
    Disk {
        slot_id: &'a str,
    },
    Journal {
        slot_id: &'a str,
    },
    Version {
        slot_id: &'a str,
        version_id: &'a str,
    },
}

#[derive(Serialize)]
struct Issue<'a> {
    area: Area,
    target: Target<'a>,
    failure: crate::ApiError,
}
impl<'a> From<&'a IntegrityScrubIssue> for Issue<'a> {
    fn from(issue: &'a IntegrityScrubIssue) -> Self {
        let IntegrityScrubIssue {
            area,
            target,
            source,
        } = issue;
        let area = match area {
            IntegrityScrubArea::Identity => Area::Identity,
            IntegrityScrubArea::Metadata => Area::Metadata,
            IntegrityScrubArea::Payload => Area::Payload,
        };
        let target = match target {
            IntegrityScrubTarget::Disk { slot_id } => Target::Disk { slot_id },
            IntegrityScrubTarget::Journal { slot_id } => Target::Journal { slot_id },
            IntegrityScrubTarget::Version {
                slot_id,
                version_id,
            } => Target::Version {
                slot_id,
                version_id,
            },
        };
        Self {
            area,
            target,
            failure: crate::engine_backend::error_response(source),
        }
    }
}
