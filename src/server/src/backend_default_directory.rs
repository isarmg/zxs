//! Complete default-directory metadata encoded from borrowed publication records.
use crate::{
    ApiError,
    backend_file_history::{BoundedWriter, EncodedPage},
};
use data_serialization::Serialize;
use storage_contract::DirectoryBranch;

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

pub struct SelectedDirectoryBranch<'a> {
    pub directory_branch_id: &'a str,
    pub name: &'a str,
}
/// A root-directory summary borrowing its publication's actual record.
pub struct DefaultDirectoryBranchRow<'a> {
    pub branch: &'a DirectoryBranch,
    pub current: bool,
}
/// Complete encoded response; retains neither publication nor authorization.
#[derive(Debug)]
pub struct DefaultDirectoryBranchSnapshot {
    encoded: EncodedPage,
}
impl DefaultDirectoryBranchSnapshot {
    /// Encode once, rejecting complete responses above 8 MiB without truncation.
    pub fn encode(
        repository_id: &str,
        namespace_branch_id: &str,
        selected: Option<SelectedDirectoryBranch<'_>>,
        rows: &[DefaultDirectoryBranchRow<'_>],
    ) -> Result<Self, ApiError> {
        let mut writer = BoundedWriter::new(storage_contract::client_protocol::MAX_RESPONSE_BYTES);
        let (directory_branch_id, name) = selected.map_or((None, "main"), |selected| {
            (Some(selected.directory_branch_id), selected.name)
        });
        writer.punctuation(b"{\"repository_id\":")?;
        writer.json(&repository_id)?;
        writer.punctuation(b",\"namespace_branch_id\":")?;
        writer.json(&namespace_branch_id)?;
        writer.punctuation(b",\"directory_branch_id\":")?;
        writer.json(&directory_branch_id)?;
        writer.punctuation(b",\"name\":")?;
        writer.json(&name)?;
        writer.punctuation(b",\"directory_branches\":[")?;
        for (index, row) in rows.iter().enumerate() {
            if index != 0 {
                writer.punctuation(b",")?;
            }
            writer.json(&DirectoryBranchRecord::from(row))?;
        }
        writer.punctuation(b"]}")?;
        Ok(Self {
            encoded: writer.finish(),
        })
    }
    pub(crate) fn into_encoded(self) -> EncodedPage {
        self.encoded
    }
    #[cfg(test)]
    pub(crate) fn body(&self) -> &[u8] {
        self.encoded.body()
    }
}
#[derive(Serialize)]
struct DirectoryBranchRecord<'a> {
    directory_branch_id: &'a str,
    name: &'a str,
    path: &'static str,
    current: bool,
    materialized: bool,
    head_tree_id: &'a str,
    file_count: String,
    parent_branch_id: &'a Option<String>,
}
impl<'a> From<&DefaultDirectoryBranchRow<'a>> for DirectoryBranchRecord<'a> {
    fn from(row: &DefaultDirectoryBranchRow<'a>) -> Self {
        let DirectoryBranch {
            directory_branch_id,
            repository_id: _,
            directory_id: _,
            path: _,
            name,
            parent_branch_id,
            head_tree_id,
            commit_id: _,
            entries: _,
            file_branches,
            file_contexts: _,
            inherited_files: _,
            directory_refs: _,
            materialized,
            sequence: _,
        } = row.branch;
        Self {
            directory_branch_id,
            name,
            path: "",
            current: row.current,
            materialized: *materialized,
            head_tree_id,
            file_count: file_branches.len().to_string(),
            parent_branch_id,
        }
    }
}
#[cfg(test)]
#[path = "../../../test/server/unit/backend_default_directory.rs"]
mod tests;
