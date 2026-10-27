//! Request-local activity projection; it grants no future admission.
//! Persisted action names are open strings; the projected row shapes are closed.
use crate::ApiError;
use crate::backend_branch::{ContentDispositionResponse, ContentDispositionSnapshot};
use data_serialization::Serialize;

pub(crate) const HISTORY_LIMIT: usize = 200;

#[derive(Debug)]
pub struct ActivityHistorySnapshot {
    pub events: Vec<ActivityEventSnapshot>,
}

#[derive(Debug)]
pub struct ActivityEventSnapshot {
    pub sequence: u64,
    pub event_id: String,
    pub timestamp: Option<u64>,
    pub slot_id: String,
    pub path: String,
    pub action: String,
    pub principal: Option<String>,
    pub file_id: String,
    pub message: String,
    pub detail: ActivityDetailSnapshot,
}

#[derive(Debug)]
pub enum ActivityDetailSnapshot {
    Plain {
        version_id: Option<String>,
    },
    Content(ContentDispositionSnapshot),
    FileOperation {
        version_id: Option<String>,
        node_id: Option<String>,
        branch_id: Option<String>,
    },
    DirectoryOperation {
        version_id: Option<String>,
        directory_branch_id: String,
        head_tree_id: Option<String>,
    },
    Backup {
        version_id: Option<String>,
        target_copies: Option<u64>,
    },
    Path {
        version_id: String,
        node_id: String,
        previous_path: String,
        branch_id: String,
    },
    Tree {
        version_id: Option<String>,
        directory_branch_id: String,
        head_tree_id: String,
        parent_tree_id: Option<String>,
    },
}

#[derive(Debug, Serialize)]
pub(crate) struct ActivityHistoryResponse {
    events: Vec<ActivityEvent>,
    limit: usize,
}

#[derive(Debug, Serialize)]
struct ActivityEvent {
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    event_id: String,
    timestamp: Option<String>,
    slot_id: String,
    path: String,
    action: String,
    principal: Option<String>,
    file_id: String,
    message: String,
    #[serialization(flatten)]
    detail: ActivityDetail,
}

#[derive(Debug, Serialize)]
#[serialization(untagged)]
enum ActivityDetail {
    Plain {
        version_id: Option<String>,
    },
    Content(ContentDispositionResponse),
    FileOperation {
        version_id: Option<String>,
        node_id: Option<String>,
        branch_id: Option<String>,
    },
    DirectoryOperation {
        version_id: Option<String>,
        directory_branch_id: String,
        head_tree_id: Option<String>,
    },
    Backup {
        version_id: Option<String>,
        target_copies: Option<u64>,
    },
    Path {
        version_id: String,
        node_id: String,
        previous_path: String,
        branch_id: String,
    },
    Tree {
        version_id: Option<String>,
        directory_branch_id: String,
        head_tree_id: String,
        parent_tree_id: Option<String>,
    },
}

impl TryFrom<ActivityHistorySnapshot> for ActivityHistoryResponse {
    type Error = ApiError;
    fn try_from(snapshot: ActivityHistorySnapshot) -> Result<Self, ApiError> {
        let events = snapshot
            .events
            .into_iter()
            .map(|event| {
                let ActivityEventSnapshot {
                    sequence,
                    event_id,
                    timestamp,
                    slot_id,
                    path,
                    action,
                    principal,
                    file_id,
                    message,
                    detail,
                } = event;
                Ok(ActivityEvent {
                    sequence,
                    event_id,
                    timestamp: timestamp.map(utc_timestamp).transpose()?,
                    slot_id,
                    path,
                    action,
                    principal,
                    file_id,
                    message,
                    detail: detail.into(),
                })
            })
            .collect::<Result<_, ApiError>>()?;
        Ok(Self {
            events,
            limit: HISTORY_LIMIT,
        })
    }
}
impl From<ActivityDetailSnapshot> for ActivityDetail {
    fn from(detail: ActivityDetailSnapshot) -> Self {
        match detail {
            ActivityDetailSnapshot::Plain { version_id } => Self::Plain { version_id },
            ActivityDetailSnapshot::Content(content) => Self::Content(content.into()),
            ActivityDetailSnapshot::FileOperation {
                version_id,
                node_id,
                branch_id,
            } => Self::FileOperation {
                version_id,
                node_id,
                branch_id,
            },
            ActivityDetailSnapshot::DirectoryOperation {
                version_id,
                directory_branch_id,
                head_tree_id,
            } => Self::DirectoryOperation {
                version_id,
                directory_branch_id,
                head_tree_id,
            },
            ActivityDetailSnapshot::Backup {
                version_id,
                target_copies,
            } => Self::Backup {
                version_id,
                target_copies,
            },
            ActivityDetailSnapshot::Path {
                version_id,
                node_id,
                previous_path,
                branch_id,
            } => Self::Path {
                version_id,
                node_id,
                previous_path,
                branch_id,
            },
            ActivityDetailSnapshot::Tree {
                version_id,
                directory_branch_id,
                head_tree_id,
                parent_tree_id,
            } => Self::Tree {
                version_id,
                directory_branch_id,
                head_tree_id,
                parent_tree_id,
            },
        }
    }
}

/// Persisted UTC seconds are formatted only at the response boundary.
pub(crate) fn utc_timestamp(seconds: u64) -> Result<String, ApiError> {
    let invalid = || {
        ApiError::new(
            ::http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::StateInvalid,
            "history timestamp is invalid",
        )
    };
    date_time::OffsetDateTime::from_unix_timestamp(
        i64::try_from(seconds).map_err(|source| invalid().with_source(source))?,
    )
    .map_err(|source| invalid().with_source(source))?
    .format(&date_time::format_description::well_known::Rfc3339)
    .map_err(|source| invalid().with_source(source))
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_activity.rs"]
mod tests;
