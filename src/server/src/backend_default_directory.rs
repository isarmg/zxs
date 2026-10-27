//! Default directory selection metadata and its explicit public response.
use data_serialization::Serialize;
use storage_contract::DirectoryBranchSummary;

pub struct RenameDirectoryBranch {
    pub repository_id: String,
    pub directory_branch_id: String,
    pub name: storage_contract::BranchName,
    pub operation_id: storage_contract::OperationId,
}
pub struct SelectDefaultDirectoryBranch {
    pub repository_id: String,
    pub directory_branch_id: String,
    pub operation_id: storage_contract::OperationId,
}

pub struct SelectedDirectoryBranch {
    pub directory_branch_id: String,
    pub name: String,
}
pub struct DefaultDirectoryBranchSnapshot {
    pub repository_id: String,
    pub namespace_branch_id: String,
    pub selected: Option<SelectedDirectoryBranch>,
    pub directory_branches: Vec<DirectoryBranchSummary>,
}
#[derive(Serialize)]
pub(crate) struct DefaultDirectoryBranchResponse {
    repository_id: String,
    namespace_branch_id: String,
    directory_branch_id: Option<String>,
    name: String,
    directory_branches: Vec<DirectoryBranchRecord>,
}
#[derive(Serialize)]
struct DirectoryBranchRecord {
    directory_branch_id: String,
    name: String,
    path: String,
    current: bool,
    materialized: bool,
    head_tree_id: String,
    file_count: String,
    parent_branch_id: Option<String>,
}
impl From<DirectoryBranchSummary> for DirectoryBranchRecord {
    fn from(summary: DirectoryBranchSummary) -> Self {
        let DirectoryBranchSummary {
            directory_branch_id,
            name,
            path,
            current,
            materialized,
            head_tree_id,
            file_count,
            parent_branch_id,
        } = summary;
        Self {
            directory_branch_id,
            name,
            path,
            current,
            materialized,
            head_tree_id,
            file_count: file_count.to_string(),
            parent_branch_id,
        }
    }
}
impl From<DefaultDirectoryBranchSnapshot> for DefaultDirectoryBranchResponse {
    fn from(snapshot: DefaultDirectoryBranchSnapshot) -> Self {
        let DefaultDirectoryBranchSnapshot {
            repository_id,
            namespace_branch_id,
            selected,
            directory_branches,
        } = snapshot;
        let (directory_branch_id, name) = match selected {
            Some(SelectedDirectoryBranch {
                directory_branch_id,
                name,
            }) => (Some(directory_branch_id), name),
            None => (None, "main".into()),
        };
        Self {
            repository_id,
            namespace_branch_id,
            directory_branch_id,
            name,
            directory_branches: directory_branches
                .into_iter()
                .map(DirectoryBranchRecord::from)
                .collect(),
        }
    }
}
#[cfg(test)]
#[path = "../../../test/server/unit/backend_default_directory.rs"]
mod tests;
