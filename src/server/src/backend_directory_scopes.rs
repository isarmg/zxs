//! Owned directory-scope observations; summary reads never copy namespace payloads.
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
pub struct DirectoryScopesSnapshot {
    pub repository_id: String,
    pub branches: DirectoryScopeRows,
}
pub enum DirectoryScopeRows {
    Full(Vec<DirectoryScopeBranch>),
    Summary(Vec<DirectoryBranchChoice>),
}
pub struct DirectoryScopeBranch {
    pub branch: DirectoryBranch,
    pub current: bool,
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
    Full(Vec<DirectoryScopeRecord>),
    Summary(Vec<DirectoryChoiceRecord>),
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
struct DirectoryScopeRecord {
    directory_branch_id: String,
    repository_id: String,
    directory_id: String,
    path: String,
    name: String,
    parent_branch_id: Option<String>,
    head_tree_id: String,
    commit_id: String,
    entries: Vec<Entry>,
    file_branches: BTreeMap<String, String>,
    #[serialization(skip_serializing_if = "BTreeMap::is_empty")]
    file_contexts: BTreeMap<String, FileContext>,
    #[serialization(skip_serializing_if = "BTreeMap::is_empty")]
    inherited_files: BTreeMap<String, String>,
    #[serialization(skip_serializing_if = "BTreeMap::is_empty")]
    directory_refs: BTreeMap<String, DirectoryReference>,
    materialized: bool,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    current: bool,
    file_count: String,
}
impl From<DirectoryScopeBranch> for DirectoryScopeRecord {
    fn from(snapshot: DirectoryScopeBranch) -> Self {
        let DirectoryScopeBranch { branch, current } = snapshot;
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
        } = branch;
        let file_count = entries
            .iter()
            .filter(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
            .count();
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
            materialized,
            sequence,
            current,
            file_count: file_count.to_string(),
        }
    }
}
impl From<DirectoryScopesSnapshot> for DirectoryScopesResponse {
    fn from(snapshot: DirectoryScopesSnapshot) -> Self {
        let DirectoryScopesSnapshot {
            repository_id,
            branches,
        } = snapshot;
        let directory_branches = match branches {
            DirectoryScopeRows::Full(rows) => DirectoryScopeResponses::Full(
                rows.into_iter().map(DirectoryScopeRecord::from).collect(),
            ),
            DirectoryScopeRows::Summary(rows) => DirectoryScopeResponses::Summary(
                rows.into_iter().map(DirectoryChoiceRecord::from).collect(),
            ),
        };
        Self {
            repository_id,
            directory_branches,
        }
    }
}
