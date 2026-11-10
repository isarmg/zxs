//! Exact directory request captured before the adapter records submission.
use data_serialization::{Deserialize, Serialize};

pub struct PrepareCurrentDirectoryDeletion {
    pub branch_id: String,
    pub path: name_policy::LookupPath,
    pub expected_entry_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CurrentDirectoryDeletion {
    pub repository_id: String,
    pub branch_id: String,
    pub request: storage_contract::DirectoryNamespaceBatch,
}

pub struct DeleteCurrentDirectory {
    pub captured: CurrentDirectoryDeletion,
    pub operation_id: storage_contract::OperationId,
}

pub struct ReadDirectoryDeletions {
    pub(crate) after: Option<String>,
    pub(crate) snapshot: Option<String>,
}

impl ReadDirectoryDeletions {
    pub fn parse(
        mut query: std::collections::BTreeMap<String, String>,
    ) -> Result<Self, crate::ApiError> {
        let after = query.remove("after");
        let snapshot = query.remove("snapshot");
        if !query.is_empty() || after.is_some() != snapshot.is_some() {
            return Err(crate::ApiError::invalid("after and snapshot must be supplied together"));
        }
        if snapshot.as_ref().is_some_and(|snapshot| snapshot.is_empty() || snapshot.len() > 256) {
            return Err(crate::ApiError::invalid("invalid directory deletion snapshot"));
        }
        if let Some(after) = &after {
            storage_contract::validate_id(after, "entry").map_err(crate::ApiError::contract_input)?;
        }
        Ok(Self { after, snapshot })
    }
}
