//! Owned directory-scope observations; summary reads never copy namespace payloads.
use data_serialization::{Serialize, Serializer, ser::SerializeSeq};
use std::collections::BTreeMap;
use std::sync::Arc;
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
pub struct DirectoryScopesSnapshot {
    pub repository_id: String,
    pub branches: DirectoryScopeRows,
}
pub enum DirectoryScopeRows {
    Full(Vec<DirectoryScopeBranch>),
    Summary(Vec<DirectoryBranchChoice>),
}
pub struct DirectoryScopeBranch {
    pub branch: Arc<DirectoryBranch>,
    pub current: bool,
    pub file_count: usize,
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

    #[cfg(feature = "core")]
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
pub(crate) struct DirectoryScopesResponse {
    repository_id: String,
    directory_branches: DirectoryScopeResponses,
}
#[derive(Serialize)]
#[serialization(untagged)]
enum DirectoryScopeResponses {
    Full(SharedDirectoryScopes),
    Summary(Vec<DirectoryChoiceRecord>),
}
struct SharedDirectoryScopes(Vec<DirectoryScopeBranch>);
impl Serialize for SharedDirectoryScopes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for row in &self.0 {
            sequence.serialize_element(&DirectoryScopeRecord::from(row))?;
        }
        sequence.end()
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
impl<'a> From<&'a DirectoryScopeBranch> for DirectoryScopeRecord<'a> {
    fn from(snapshot: &'a DirectoryScopeBranch) -> Self {
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
        } = branch.as_ref();
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
impl From<DirectoryScopesSnapshot> for DirectoryScopesResponse {
    fn from(snapshot: DirectoryScopesSnapshot) -> Self {
        let directory_branches = match snapshot.branches {
            DirectoryScopeRows::Full(rows) => {
                DirectoryScopeResponses::Full(SharedDirectoryScopes(rows))
            }
            DirectoryScopeRows::Summary(rows) => DirectoryScopeResponses::Summary(
                rows.into_iter().map(DirectoryChoiceRecord::from).collect(),
            ),
        };
        Self {
            repository_id: snapshot.repository_id,
            directory_branches,
        }
    }
}
