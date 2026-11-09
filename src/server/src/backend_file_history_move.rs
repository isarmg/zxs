//! Captured SMB identity is rechecked under the engine owner lock before move.
use crate::auth::SmbBinding;
use storage_contract::{OperationId, history_window::HistoryFileId};

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
