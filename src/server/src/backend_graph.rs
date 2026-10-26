//! Owned metadata and shared immutable commit records for one history read.
use data_serialization::Serialize;
use std::{collections::BTreeMap, sync::Arc};
use storage_contract::{Branch, Commit, HistoryBoundary, HistoryTree, MergeLink};

#[derive(Debug)]
pub struct HistoryGraphSnapshot {
    pub history: HistoryTree,
    pub commits: Vec<Arc<Commit>>,
    pub branches: Vec<Branch>,
    pub merge_links: Vec<MergeLink>,
    pub boundary_marks: Vec<HistoryBoundary>,
}
#[derive(Serialize)]
pub(crate) struct HistoryGraphResponse {
    history: HistoryRecord,
    commits: Vec<crate::commit_wire::CommitResponse>,
    branches: Vec<BranchRecord>,
    merge_links: Vec<MergeLinkRecord>,
    boundary_marks: Vec<BoundaryRecord>,
}
impl From<HistoryGraphSnapshot> for HistoryGraphResponse {
    fn from(snapshot: HistoryGraphSnapshot) -> Self {
        let HistoryGraphSnapshot {
            history,
            commits,
            branches,
            merge_links,
            boundary_marks,
        } = snapshot;
        Self {
            history: history.into(),
            commits: commits
                .into_iter()
                .map(crate::commit_wire::CommitResponse::from)
                .collect(),
            branches: branches.into_iter().map(BranchRecord::from).collect(),
            merge_links: merge_links.into_iter().map(MergeLinkRecord::from).collect(),
            boundary_marks: boundary_marks
                .into_iter()
                .map(BoundaryRecord::from)
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct HistoryRecord {
    history_tree_id: String,
    repository_id: String,
    root_commit_id: String,
    main_branch_id: Option<String>,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    state: storage_contract::HistoryState,
    provenance: Option<String>,
}
impl From<HistoryTree> for HistoryRecord {
    fn from(record: HistoryTree) -> Self {
        let HistoryTree {
            history_tree_id,
            repository_id,
            root_commit_id,
            main_branch_id,
            generation,
            state,
            provenance,
        } = record;
        Self {
            history_tree_id,
            repository_id,
            root_commit_id,
            main_branch_id,
            generation,
            state,
            provenance,
        }
    }
}

#[derive(Serialize)]
struct BranchRecord {
    branch_id: String,
    history_tree_id: String,
    name: String,
    head_commit_id: String,
    fork_commit_id: String,
    branch_kind: storage_contract::BranchKind,
    #[serialization(skip_serializing_if = "Option::is_none")]
    file_id: Option<String>,
    #[serialization(skip_serializing_if = "Option::is_none")]
    file_node_id: Option<String>,
    #[serialization(skip_serializing_if = "BTreeMap::is_empty")]
    pending_file_versions: BTreeMap<String, String>,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    state: storage_contract::BranchState,
}
impl From<Branch> for BranchRecord {
    fn from(record: Branch) -> Self {
        let Branch {
            branch_id,
            history_tree_id,
            name,
            head_commit_id,
            fork_commit_id,
            branch_kind,
            file_id,
            file_node_id,
            pending_file_versions,
            generation,
            state,
        } = record;
        Self {
            branch_id,
            history_tree_id,
            name,
            head_commit_id,
            fork_commit_id,
            branch_kind,
            file_id,
            file_node_id,
            pending_file_versions,
            generation,
            state,
        }
    }
}

#[derive(Serialize)]
struct MergeLinkRecord {
    merge_link_id: String,
    history_tree_id: String,
    source_branch_id: String,
    source_commit_id: String,
    target_branch_id: String,
    target_commit_id: String,
    author: String,
    operation_id: storage_contract::OperationId,
}
impl From<MergeLink> for MergeLinkRecord {
    fn from(record: MergeLink) -> Self {
        let MergeLink {
            merge_link_id,
            history_tree_id,
            source_branch_id,
            source_commit_id,
            target_branch_id,
            target_commit_id,
            author,
            operation_id,
        } = record;
        Self {
            merge_link_id,
            history_tree_id,
            source_branch_id,
            source_commit_id,
            target_branch_id,
            target_commit_id,
            author,
            operation_id,
        }
    }
}

#[derive(Serialize)]
struct BoundaryRecord {
    boundary_id: String,
    history_tree_id: String,
    local_commit_id: String,
    local_branch_id: Option<String>,
    relation: String,
    external_history_tree_id: String,
    external_commit_id: String,
    source_merge_link_id: Option<String>,
}
impl From<HistoryBoundary> for BoundaryRecord {
    fn from(record: HistoryBoundary) -> Self {
        let HistoryBoundary {
            boundary_id,
            history_tree_id,
            local_commit_id,
            local_branch_id,
            relation,
            external_history_tree_id,
            external_commit_id,
            source_merge_link_id,
        } = record;
        Self {
            boundary_id,
            history_tree_id,
            local_commit_id,
            local_branch_id,
            relation,
            external_history_tree_id,
            external_commit_id,
            source_merge_link_id,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_graph.rs"]
mod tests;
