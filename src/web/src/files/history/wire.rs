//! Projection from an already admitted shared response; this module does not decode wire JSON.
use super::{FileBranch, FileNode, NodeKind, Parent, ParentEndpoint, ParentSet};
use std::collections::BTreeMap;
use storage_contract::history_window::{
    HistoryBranchSummary, HistoryContentSource, HistoryEndpoint, HistoryGraphEdge,
    HistoryNodeKind, HistoryNodeSummary, HistoryParentSet,
};

pub(super) fn nodes(
    nodes: Vec<HistoryNodeSummary>,
    edges: Vec<HistoryGraphEdge>,
) -> Vec<FileNode> {
    let mut parents = BTreeMap::new();
    for HistoryGraphEdge { node_id, parent_node_id, endpoint } in edges {
        parents.entry(node_id).or_insert_with(Vec::new).push(Parent {
            id: parent_node_id.to_string(),
            endpoint: match endpoint {
                HistoryEndpoint::InWindow => ParentEndpoint::InWindow,
                HistoryEndpoint::OutsideWindow => ParentEndpoint::OutsideWindow,
            },
        });
    }
    nodes.into_iter().map(|node| {
        let HistoryNodeSummary {
            node_id, version_id, kind, sequence, current: _, branch_id: _,
            path, previous_path, note, tags, size: _, content_hash: _,
            content_state, download_name: _, storage_name: _, parents: parent_set,
            fields,
        } = node;
        let kind = match kind {
            HistoryNodeKind::Content => NodeKind::Content,
            HistoryNodeKind::Rename => NodeKind::Rename,
            HistoryNodeKind::Tags => NodeKind::Tags,
            HistoryNodeKind::Merge => NodeKind::Merge,
        };
        let mut node_parents = parents.remove(&node_id).unwrap_or_default();
        node_parents.sort_by(|a, b| a.id.cmp(&b.id));
        FileNode {
            id: node_id.to_string(),
            version: Some(version_id.to_string()),
            sequence,
            kind,
            body_note: (kind == NodeKind::Content).then(|| note.clone()),
            note,
            tags,
            path: Some(path),
            previous_path,
            parents: node_parents,
            parent_set: match parent_set {
                HistoryParentSet::Complete => ParentSet::Complete,
                HistoryParentSet::More { cursor } => ParentSet::More {
                    cursor: cursor.as_str().to_owned(),
                },
            },
            materialized: true,
            inherited: false,
            content_state,
            fields,
        }
    }).collect()
}

pub(super) fn branch(branch: HistoryBranchSummary) -> FileBranch {
    let HistoryBranchSummary {
        branch_id, name, head_commit_id: _, fork_commit_id: _, node_id, state: _,
        branch_kind: _, generation: _, current: _, read_only, alias_of: _, content,
    } = branch;
    let pending = content.version_id.is_none()
        || matches!(content.source, HistoryContentSource::Inherited { .. });
    FileBranch {
        id: branch_id.to_string(),
        name,
        head: node_id.map(|id| id.to_string()).or_else(|| content.version_id.map(|id| id.to_string())),
        pending,
        writable: !read_only,
    }
}
