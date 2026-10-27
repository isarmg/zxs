//! File-history read projection. These observations do not grant later authorization.
use crate::{ApiError, BranchSnapshot, ContentDispositionSnapshot};
use data_serialization::Serialize;
use std::{collections::BTreeSet, sync::Arc};

pub struct ReadFileHistory {
    file_id: String,
    location: Option<FileHistoryLocation>,
}
struct FileHistoryLocation {
    pub repository_id: String,
    pub path: name_policy::LookupPath,
}
impl ReadFileHistory {
    pub fn file_id(&self) -> &str {
        &self.file_id
    }
    pub fn location(&self) -> Option<(&str, &name_policy::LookupPath)> {
        self.location
            .as_ref()
            .map(|location| (location.repository_id.as_str(), &location.path))
    }

    /// Parse external lookup context; admission and current path membership remain live checks.
    pub fn parse(
        file_id: String,
        repository_id: Option<String>,
        path: Option<String>,
    ) -> Result<Self, ApiError> {
        if file_id.is_empty() {
            return Err(ApiError::invalid("missing string field file_id"));
        }
        let location = match (repository_id, path) {
            (None, None) => None,
            (Some(repository_id), Some(path)) => Some(FileHistoryLocation {
                repository_id,
                path: name_policy::LookupPath::parse(&path).map_err(ApiError::invalid_source)?,
            }),
            (Some(_), None) | (None, Some(_)) => {
                return Err(ApiError::invalid(
                    "repository_id and path must be supplied together",
                ));
            }
        };
        Ok(Self { file_id, location })
    }
}
#[derive(Debug)]
pub struct FileHistorySnapshot {
    pub file_id: String,
    pub repository_id: String,
    pub path: Option<String>,
    pub note: String,
    pub current_node_id: Option<String>,
    pub selected_branch_id: Option<String>,
    pub versions: Vec<HistoryVersion>,
    pub nodes: Vec<HistoryNode>,
    pub branches: Vec<HistoryBranch>,
    pub merges: Vec<storage_contract::FileBranchMerge>,
    pub content: Option<ContentDispositionSnapshot>,
}
#[derive(Debug)]
pub struct HistoryBranch {
    pub branch: BranchSnapshot,
    pub current: bool,
}
/// Shared fields are owned once even when a content version also appears as a node.
#[derive(Debug, Serialize)]
pub struct HistoryVersionData {
    pub version_id: String,
    pub file_id: String,
    pub source_version_ids: Vec<String>,
    pub branch_id: Option<String>,
    pub created_at: String,
    pub message: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub size: u64,
    #[serialization(serialize_with = "serialize_content_state")]
    pub content_state: Option<storage_contract::ContentState>,
    pub content_hash: storage_contract::ContentHash,
    pub note: String,
    pub download_name: String,
    pub storage_name: String,
}
#[derive(Debug, Serialize)]
pub struct HistoryVersion {
    #[serialization(flatten, serialize_with = "serialize_version_data")]
    pub data: Arc<HistoryVersionData>,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub current: bool,
}
#[derive(Debug, Serialize)]
#[serialization(untagged)]
pub enum HistoryNode {
    Content {
        #[serialization(flatten)]
        version: HistoryVersion,
        node_id: String,
        kind: ContentNodeKind,
        tags: BTreeSet<String>,
        parent_node_ids: Vec<String>,
        path: String,
        previous_path: Option<String>,
    },
    Revision(Box<HistoryRevision>),
}
#[derive(Debug, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum ContentNodeKind {
    Content,
}
#[derive(Debug, Serialize)]
#[serialization(untagged)]
pub enum RevisionKind {
    Rename(RenameNodeKind),
    Metadata(storage_contract::FileMetadataKind),
}
#[derive(Debug, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum RenameNodeKind {
    Rename,
}
#[derive(Debug, Serialize)]
pub struct HistoryRevision {
    pub node_id: String,
    pub version_id: String,
    pub file_id: String,
    pub parent_node_ids: Vec<String>,
    pub kind: RevisionKind,
    pub path: String,
    pub previous_path: Option<String>,
    pub branch_id: String,
    pub created_at: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub current: bool,
    pub note: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub size: u64,
    pub content_hash: storage_contract::ContentHash,
    #[serialization(serialize_with = "serialize_content_state")]
    pub content_state: Option<storage_contract::ContentState>,
    pub download_name: String,
    pub storage_name: String,
    pub operation_id: storage_contract::OperationId,
    pub author: String,
    pub message: String,
    pub tags: BTreeSet<String>,
}
#[derive(Serialize)]
pub(crate) struct FileHistoryResponse {
    file_id: String,
    repository_id: String,
    path: Option<String>,
    note: String,
    node_id: Option<String>,
    current_node_id: Option<String>,
    selected_branch_id: Option<String>,
    versions: Vec<HistoryVersion>,
    nodes: Vec<HistoryNode>,
    branches: Vec<HistoryBranchResponse>,
    merges: Vec<HistoryMergeResponse>,
    #[serialization(flatten)]
    content: Option<crate::backend_branch::ContentDispositionResponse>,
}
#[derive(Serialize)]
struct HistoryBranchResponse {
    #[serialization(flatten)]
    branch: crate::backend_branch::BranchResponse,
    current: bool,
}
impl From<FileHistorySnapshot> for FileHistoryResponse {
    fn from(snapshot: FileHistorySnapshot) -> Self {
        let FileHistorySnapshot {
            file_id,
            repository_id,
            path,
            note,
            current_node_id,
            selected_branch_id,
            versions,
            nodes,
            branches,
            merges,
            content,
        } = snapshot;
        Self {
            file_id,
            repository_id,
            path,
            note,
            node_id: current_node_id.clone(),
            current_node_id,
            selected_branch_id,
            versions,
            nodes,
            branches: branches
                .into_iter()
                .map(|HistoryBranch { branch, current }| HistoryBranchResponse {
                    branch: branch.into(),
                    current,
                })
                .collect(),
            merges: merges.into_iter().map(HistoryMergeResponse::from).collect(),
            content: content.map(Into::into),
        }
    }
}

fn serialize_content_state<S: data_serialization::Serializer>(
    state: &Option<storage_contract::ContentState>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(
        state
            .as_ref()
            .map(storage_contract::ContentState::as_str)
            .unwrap_or("LIVE"),
    )
}

#[derive(Serialize)]
struct HistoryMergeResponse {
    merge_id: String,
    history_tree_id: String,
    file_id: String,
    input_branch_ids: Vec<String>,
    input_node_ids: Vec<String>,
    chosen_node_id: String,
    result_node_id: String,
    surviving_branch_id: String,
    reused_node: bool,
    parent_node_ids: Vec<String>,
    operation_id: storage_contract::OperationId,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
}
impl From<storage_contract::FileBranchMerge> for HistoryMergeResponse {
    fn from(merge: storage_contract::FileBranchMerge) -> Self {
        let storage_contract::FileBranchMerge {
            merge_id,
            history_tree_id,
            file_id,
            input_branch_ids,
            input_node_ids,
            chosen_node_id,
            result_node_id,
            surviving_branch_id,
            reused_node,
            parent_node_ids,
            operation_id,
            sequence,
        } = merge;
        Self {
            merge_id,
            history_tree_id,
            file_id,
            input_branch_ids,
            input_node_ids,
            chosen_node_id,
            result_node_id,
            surviving_branch_id,
            reused_node,
            parent_node_ids,
            operation_id,
            sequence,
        }
    }
}

fn serialize_version_data<S: data_serialization::Serializer>(
    data: &Arc<HistoryVersionData>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    data.as_ref().serialize(serializer)
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_file_history.rs"]
mod tests;
