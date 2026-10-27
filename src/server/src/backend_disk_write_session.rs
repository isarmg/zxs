//! Disk write workspaces are fresh sessions, not idempotent operations.
pub struct WriteSessionBranchId(String);
impl TryFrom<String> for WriteSessionBranchId {
    type Error = storage_contract::Error;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        storage_contract::validate_id(&value, "br")?;
        Ok(Self(value))
    }
}
impl WriteSessionBranchId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
pub struct CreateDiskWriteSession {
    pub slot_id: String,
    pub path: name_policy::PortablePath,
    pub branch_id: Option<WriteSessionBranchId>,
}
#[derive(Debug)]
pub struct DiskWriteSession {
    pub workspace: storage_contract::Workspace,
    pub relative_path: String,
    pub display_path: String,
}
#[derive(data_serialization::Serialize)]
pub(crate) struct DiskWriteSessionResponse {
    #[serialization(flatten)]
    workspace: crate::backend_workspace::WorkspaceResponse,
    relative_path: String,
    display_path: String,
}
impl From<DiskWriteSession> for DiskWriteSessionResponse {
    fn from(session: DiskWriteSession) -> Self {
        let DiskWriteSession {
            workspace,
            relative_path,
            display_path,
        } = session;
        Self {
            workspace: workspace.into(),
            relative_path,
            display_path,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_disk_write_session.rs"]
mod tests;
