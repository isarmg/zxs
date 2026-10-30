//! Complete directory-scope responses encoded from borrowed publication records.
use crate::{
    ApiError,
    backend_file_history::{BoundedWriter, EncodedPage},
};
use data_serialization::Serialize;
use std::collections::BTreeMap;
use storage_contract::{DirectoryBranch, DirectoryReference, Entry, FileContext};

pub enum DirectoryBranchFilter {
    All,
    Root,
    Directory(name_policy::LookupPath),
}
impl DirectoryBranchFilter {
    #[cfg(any(feature = "core", test))]
    pub(crate) fn key(&self) -> Option<&str> {
        match self {
            Self::All => None,
            Self::Root => Some(""),
            Self::Directory(path) => Some(path.key()),
        }
    }
}
#[derive(Clone, Copy)]
pub enum DirectoryBranchProjection {
    Full,
    Summary,
}
pub struct ReadDirectoryScopes<'a> {
    pub repository_id: &'a str,
    pub filter: &'a DirectoryBranchFilter,
    pub projection: DirectoryBranchProjection,
}
/// Complete encoded response; owns neither publication records nor authorization.
#[derive(Debug)]
pub struct DirectoryScopesSnapshot {
    encoded: EncodedPage,
}
/// Lightweight observation borrowing the actual publication's record.
pub struct DirectoryScopeBranch<'a> {
    pub branch: &'a DirectoryBranch,
    pub current: bool,
    pub file_count: usize,
}
impl DirectoryScopesSnapshot {
    /// Encode the complete response once. Oversized responses fail, never truncate.
    /// Returns `ResourceLimitExceeded` above the protocol's 8 MiB response cap.
    pub fn encode(
        repository_id: &str,
        projection: DirectoryBranchProjection,
        rows: &[DirectoryScopeBranch<'_>],
    ) -> Result<Self, ApiError> {
        let mut writer = BoundedWriter::new(storage_contract::client_protocol::MAX_RESPONSE_BYTES);
        writer.punctuation(b"{\"repository_id\":")?;
        writer.json(&repository_id)?;
        writer.punctuation(b",\"directory_branches\":[")?;
        for (index, row) in rows.iter().enumerate() {
            if index != 0 {
                writer.punctuation(b",")?;
            }
            match projection {
                DirectoryBranchProjection::Full => writer.json(&DirectoryScopeRecord::from(row))?,
                DirectoryBranchProjection::Summary => writer.json(&DirectoryChoiceRecord::from(
                    DirectoryBranchChoice::capture_counted(row.branch, row.current, row.file_count),
                ))?,
            }
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
pub struct DirectoryBranchChoice {
    pub directory_branch_id: String,
    pub repository_id: String,
    pub directory_id: String,
    pub name: String,
    pub parent_branch_id: Option<String>,
    pub head_tree_id: String,
    pub commit_id: String,
    pub path: String,
    pub materialized: bool,
    pub current: bool,
    pub sequence: u64,
    pub file_count: usize,
}
impl DirectoryBranchChoice {
    #[cfg(feature = "core")]
    pub(crate) fn capture(branch: &DirectoryBranch, current: bool) -> Self {
        let file_count = branch
            .entries
            .iter()
            .filter(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
            .count();
        Self::capture_counted(branch, current, file_count)
    }

    pub(crate) fn capture_counted(
        branch: &DirectoryBranch,
        current: bool,
        file_count: usize,
    ) -> Self {
        let DirectoryBranch {
            directory_branch_id,
            repository_id,
            directory_id,
            path,
            name,
            parent_branch_id,
            head_tree_id,
            commit_id,
            entries: _,
            file_branches: _,
            file_contexts: _,
            inherited_files: _,
            directory_refs: _,
            materialized,
            sequence,
        } = branch;
        Self {
            directory_branch_id: directory_branch_id.clone(),
            repository_id: repository_id.clone(),
            directory_id: directory_id.clone(),
            path: path.clone(),
            name: name.clone(),
            parent_branch_id: parent_branch_id.clone(),
            head_tree_id: head_tree_id.clone(),
            commit_id: commit_id.clone(),
            materialized: *materialized,
            current,
            sequence: *sequence,
            file_count,
        }
    }
}
#[derive(Serialize)]
pub(crate) struct DirectoryChoiceRecord {
    directory_branch_id: String,
    repository_id: String,
    directory_id: String,
    name: String,
    parent_branch_id: Option<String>,
    head_tree_id: String,
    commit_id: String,
    path: String,
    materialized: bool,
    current: bool,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    file_count: String,
}
impl From<DirectoryBranchChoice> for DirectoryChoiceRecord {
    fn from(choice: DirectoryBranchChoice) -> Self {
        let DirectoryBranchChoice {
            directory_branch_id,
            repository_id,
            directory_id,
            name,
            parent_branch_id,
            head_tree_id,
            commit_id,
            path,
            materialized,
            current,
            sequence,
            file_count,
        } = choice;
        Self {
            directory_branch_id,
            repository_id,
            directory_id,
            name,
            parent_branch_id,
            head_tree_id,
            commit_id,
            path,
            materialized,
            current,
            sequence,
            file_count: file_count.to_string(),
        }
    }
}
#[derive(Serialize)]
struct DirectoryScopeRecord<'a> {
    directory_branch_id: &'a str,
    repository_id: &'a str,
    directory_id: &'a str,
    path: &'a str,
    name: &'a str,
    parent_branch_id: &'a Option<String>,
    head_tree_id: &'a str,
    commit_id: &'a str,
    entries: &'a [Entry],
    file_branches: &'a BTreeMap<String, String>,
    #[serialization(skip_serializing_if = "BTreeMap::is_empty")]
    file_contexts: &'a BTreeMap<String, FileContext>,
    #[serialization(skip_serializing_if = "BTreeMap::is_empty")]
    inherited_files: &'a BTreeMap<String, String>,
    #[serialization(skip_serializing_if = "BTreeMap::is_empty")]
    directory_refs: &'a BTreeMap<String, DirectoryReference>,
    materialized: bool,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    current: bool,
    file_count: String,
}
impl<'a> From<&DirectoryScopeBranch<'a>> for DirectoryScopeRecord<'a> {
    fn from(snapshot: &DirectoryScopeBranch<'a>) -> Self {
        let DirectoryScopeBranch {
            branch,
            current,
            file_count,
        } = snapshot;
        let DirectoryBranch {
            directory_branch_id,
            repository_id,
            directory_id,
            path,
            name,
            parent_branch_id,
            head_tree_id,
            commit_id,
            entries,
            file_branches,
            file_contexts,
            inherited_files,
            directory_refs,
            materialized,
            sequence,
        } = *branch;
        Self {
            directory_branch_id,
            repository_id,
            directory_id,
            path,
            name,
            parent_branch_id,
            head_tree_id,
            commit_id,
            entries,
            file_branches,
            file_contexts,
            inherited_files,
            directory_refs,
            materialized: *materialized,
            sequence: *sequence,
            current: *current,
            file_count: file_count.to_string(),
        }
    }
}
