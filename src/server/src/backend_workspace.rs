//! Workspace commands and explicit HTTP projections; local staging paths never enter wire DTOs.
use data_serialization::Serialize;
use std::collections::BTreeMap;
use storage_contract::{FileContext, NamespaceOperation, WorkspacePurpose, WorkspaceState};

#[derive(Clone, Debug)]
pub struct CreateWorkspace {
    pub branch_id: String,
    pub source_commit_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct WorkspaceResponse {
    workspace_id: String,
    purpose: WorkspacePurpose,
    branch_id: String,
    /// Immutable operation actor, bound to staged data and replay, not an ACL.
    principal: String,
    base_head: String,
    #[serialization(with = "storage_contract::u64_string")]
    fencing_token: u64,
    files: BTreeMap<String, StagedFileResponse>,
    /// Freeze the file selections visible when a directory workspace opens.
    /// Two unwritten branches can share a source version but are distinct
    /// destinations, so comparing version IDs alone cannot fence a switch.
    #[serialization(skip_serializing_if = "BTreeMap::is_empty")]
    selected_file_branches: BTreeMap<String, String>,
    #[serialization(skip_serializing_if = "BTreeMap::is_empty")]
    selected_file_contexts: BTreeMap<String, FileContext>,
    /// Selected composition identities at open time; selection is a write even
    /// when two directory snapshots happen to expose identical file bytes.
    #[serialization(skip_serializing_if = "BTreeMap::is_empty")]
    selected_directory_branches: BTreeMap<String, String>,
    #[serialization(
        skip_serializing_if = "BTreeMap::is_empty",
        with = "storage_contract::u64_string_map"
    )]
    directory_selection_generations: BTreeMap<String, u64>,
    #[serialization(
        skip_serializing_if = "BTreeMap::is_empty",
        with = "storage_contract::u64_string_map"
    )]
    file_selection_generations: BTreeMap<String, u64>,
    namespace_operations: Vec<NamespaceOperation>,
    #[serialization(with = "storage_contract::u64_string")]
    created_at: u64,
    state: WorkspaceState,
}

impl From<storage_contract::Workspace> for WorkspaceResponse {
    fn from(workspace: storage_contract::Workspace) -> Self {
        let storage_contract::Workspace {
            workspace_id,
            purpose,
            branch_id,
            principal,
            base_head,
            fencing_token,
            files,
            selected_file_branches,
            selected_file_contexts,
            selected_directory_branches,
            directory_selection_generations,
            file_selection_generations,
            namespace_operations,
            created_at,
            state,
        } = workspace;
        Self {
            workspace_id,
            purpose,
            branch_id,
            principal,
            base_head,
            fencing_token,
            files: files
                .into_iter()
                .map(|(id, file)| (id, file.into()))
                .collect(),
            selected_file_branches,
            selected_file_contexts,
            selected_directory_branches,
            directory_selection_generations,
            file_selection_generations,
            namespace_operations,
            created_at,
            state,
        }
    }
}

#[derive(Serialize)]
struct StagedFileResponse {
    item_id: String,
    path: name_policy::PortablePath,
    #[serialization(with = "storage_contract::u64_string")]
    size: u64,
    content_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    dirty_generation: u64,
    base_version_id: Option<String>,
}
impl From<storage_contract::StagedFile> for StagedFileResponse {
    fn from(file: storage_contract::StagedFile) -> Self {
        let storage_contract::StagedFile {
            item_id,
            path,
            source_path: _,
            size,
            content_hash,
            dirty_generation,
            base_version_id,
        } = file;
        Self {
            item_id,
            path,
            size,
            content_hash,
            dirty_generation,
            base_version_id,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct AbortedWorkspace {
    pub(crate) workspace_id: String,
    pub(crate) state: WorkspaceState,
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_workspace.rs"]
mod tests;

/// One engine snapshot, captured after current workspace admission.
#[derive(Clone, Debug)]
pub struct PublishedUploadStatus {
    pub generation: u64,
    pub logical_size: u64,
    pub content_hash: String,
    pub state: PublishedUploadState,
}

#[derive(Clone, Debug)]
pub enum PublishedUploadState {
    Complete,
    Sealed { manifest_hash: String },
}
