//! Stable file identity and exact deletion identity cross the adapter boundary.
use crate::ApiError;
use data_serialization::Serialize;
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

/// Ordinary HTTP deletion is bound to one stable file and its current location,
/// not to a user-selected subset of directory or file branches.
pub struct DeleteFile {
    pub repository_id: HistoryRepositoryId,
    pub file_id: HistoryFileId,
    pub path: name_policy::LookupPath,
    pub operation_id: storage_contract::OperationId,
}

pub struct DeleteCurrentFile {
    pub branch_id: String,
    pub path: name_policy::LookupPath,
    pub expected_entry_id: String,
    pub file_id: HistoryFileId,
    pub operation_id: storage_contract::OperationId,
}

pub struct RestoreDeletedFile {
    pub file_id: HistoryFileId,
    pub deletion_node_id: String,
    pub operation_id: storage_contract::OperationId,
}

pub struct ReadFileDeletions {
    pub(crate) after: Option<HistoryFileId>,
    pub(crate) snapshot: Option<String>,
}
impl ReadFileDeletions {
    pub fn after(&self) -> Option<&HistoryFileId> {
        self.after.as_ref()
    }
    pub fn snapshot(&self) -> Option<&str> {
        self.snapshot.as_deref()
    }

    pub fn parse(mut query: std::collections::BTreeMap<String, String>) -> Result<Self, ApiError> {
        let after = query.remove("after");
        let snapshot = query.remove("snapshot");
        if !query.is_empty() || after.is_some() != snapshot.is_some() {
            return Err(ApiError::invalid(
                "after and snapshot must be supplied together",
            ));
        }
        if snapshot
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 256)
        {
            return Err(ApiError::invalid("invalid deletion listing snapshot"));
        }
        let after = after
            .map(|id| HistoryFileId::parse(&id).map_err(ApiError::contract_input))
            .transpose()?;
        Ok(Self { after, snapshot })
    }
}

#[derive(Serialize)]
pub struct FileDeletionSummary {
    pub file_id: String,
    pub deletion_node_id: String,
    pub repository_id: String,
    pub path: String,
}

#[derive(Serialize)]
pub struct FileDeletionsPage {
    pub snapshot: String,
    pub entries: Vec<FileDeletionSummary>,
    pub next_after: Option<String>,
}
