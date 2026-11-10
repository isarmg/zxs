//! Captured SMB identity is rechecked under the engine owner lock before move.
use crate::auth::SmbBinding;
use storage_contract::{OperationId, history_window::HistoryFileId};

/// Captured current-object intent. Transport bindings remain authorization
/// facts; the engine alone determines the object's owner and move members.
#[derive(Debug)]
pub struct MoveCurrentObject {
    pub source_binding: SmbBinding,
    pub target: MoveObjectTarget,
    pub movement: storage_contract::file_relocation::ObjectMoveRequest,
    pub expected_source_file_id: Option<HistoryFileId>,
    pub expected_target_file_id: Option<HistoryFileId>,
    pub operation_id: OperationId,
}

#[derive(Debug)]
pub enum MoveObjectTarget {
    Repository(SmbBinding),
    EmptyDisk { disk_id: String, binding_generation: u64 },
}

#[derive(Debug)]
pub struct MoveCurrentFile {
    pub source_binding: SmbBinding,
    pub target_binding: SmbBinding,
    pub source_path: name_policy::LookupPath,
    pub target_name: MoveTargetName,
    pub expected_source_entry_id: String,
    pub expected_source_file_id: HistoryFileId,
    pub operation_id: OperationId,
}

/// Both original bindings and both file identities are rechecked by the owner.
#[derive(Debug)]
pub struct ReplaceCurrentFile {
    pub request: MoveCurrentFile,
    pub expected_target_entry_id: String,
    pub expected_target_file_id: HistoryFileId,
}

/// A strict creation path inside the admitted target disk-root namespace.
#[derive(Debug)]
pub struct MoveTargetName(name_policy::PortablePath);

impl MoveTargetName {
    pub fn parse(name: &str) -> Result<Self, crate::ApiError> {
        let path =
            name_policy::PortablePath::parse(name).map_err(crate::ApiError::invalid_source)?;
        Ok(Self(path))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub(crate) fn lookup(&self) -> name_policy::LookupPath {
        name_policy::LookupPath::from(&self.0)
    }

}

/// The destination disk was still an empty displayed root at admission. Its
/// stable binding, not a fabricated repository or branch, is the retry address.
#[derive(Debug)]
pub struct MoveCurrentFileToEmptyDisk {
    pub source_binding: SmbBinding,
    pub target_slot_id: String,
    pub target_disk_id: String,
    pub target_binding_generation: u64,
    pub source_path: name_policy::LookupPath,
    pub target_name: MoveTargetName,
    pub expected_source_entry_id: String,
    pub expected_source_file_id: HistoryFileId,
    pub operation_id: OperationId,
}

impl MoveCurrentFile {
    /// One mapping is shared by backend submission and SMB receipt binding.
    pub(crate) fn movement(
        &self,
        target_slot_id: &str,
        target_entry_id: Option<&str>,
    ) -> Result<storage_contract::file_relocation::ObjectMoveRequest, crate::ApiError> {
        use storage_contract::file_relocation::{NamespaceEntryId, ObjectMoveRequest};
        use storage_contract::history_window::HistoryRepositoryId;
        Ok(ObjectMoveRequest {
            source_repository_id: HistoryRepositoryId::parse(&self.source_binding.repository_id)
                .map_err(crate::ApiError::contract_input)?,
            source_entry_id: NamespaceEntryId::parse(&self.expected_source_entry_id)
                .map_err(crate::ApiError::contract_input)?,
            source_path: self.source_path.clone(),
            target_slot_id: target_slot_id.to_owned(),
            target_repository_id: Some(HistoryRepositoryId::parse(&self.target_binding.repository_id)
                .map_err(crate::ApiError::contract_input)?),
            target_path: self.target_name.0.clone(),
            replace: target_entry_id.is_some(),
            target_entry_id: target_entry_id.map(NamespaceEntryId::parse)
                .transpose().map_err(crate::ApiError::contract_input)?,
        })
    }
}

impl MoveCurrentFileToEmptyDisk {
    /// The original empty-disk intent stays None even after its root is created.
    pub(crate) fn movement(
        &self,
    ) -> Result<storage_contract::file_relocation::ObjectMoveRequest, crate::ApiError> {
        use storage_contract::file_relocation::{NamespaceEntryId, ObjectMoveRequest};
        use storage_contract::history_window::HistoryRepositoryId;
        Ok(ObjectMoveRequest {
            source_repository_id: HistoryRepositoryId::parse(&self.source_binding.repository_id)
                .map_err(crate::ApiError::contract_input)?,
            source_entry_id: NamespaceEntryId::parse(&self.expected_source_entry_id)
                .map_err(crate::ApiError::contract_input)?,
            source_path: self.source_path.clone(),
            target_slot_id: self.target_slot_id.clone(),
            target_repository_id: None,
            target_path: self.target_name.0.clone(),
            replace: false,
            target_entry_id: None,
        })
    }
}
