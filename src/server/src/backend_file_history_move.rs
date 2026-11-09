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

/// A creation name at the target disk root, never a directory path.
#[derive(Debug)]
pub struct MoveTargetName(name_policy::PortablePath);

impl MoveTargetName {
    pub fn parse(name: &str) -> Result<Self, crate::ApiError> {
        let path =
            name_policy::PortablePath::parse(name).map_err(crate::ApiError::invalid_source)?;
        if path.parent().is_some() {
            return Err(crate::ApiError::unsupported());
        }
        Ok(Self(path))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub(crate) fn lookup(&self) -> name_policy::LookupPath {
        name_policy::LookupPath::from(&self.0)
    }
}
