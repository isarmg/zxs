//! One presentation graph for content, path changes and metadata revisions.
use super::*;

use crate::backend_file_history::{
    ContentNodeKind, HistoryNode, HistoryRevision, HistoryVersion, RenameNodeKind, RevisionKind,
};

struct PendingSummary {
    remaining_rows: usize,
    summary: storage_engine::FileNodeSummary,
}

/// Consume the last row so parent/tag buffers do not live alongside the JSON
/// graph. Repeated or overlapping IDs retain the same facts for earlier rows.
fn take_summary(
    summaries: &mut BTreeMap<String, PendingSummary>,
    node: &str,
) -> storage_engine::FileNodeSummary {
    let pending = summaries
        .get_mut(node)
        .expect("invariant: projected row has a summary");
    if pending.remaining_rows > 1 {
        pending.remaining_rows -= 1;
        pending.summary.clone()
    } else {
        summaries
            .remove(node)
            .expect("invariant: last projected row owns its summary")
            .summary
    }
}

/// Metadata UUID order is unrelated to graph order. Resolve a path through the
/// explicit source chain and memoize it without recursive stack growth.
struct NodePaths<'a> {
    state: &'a storage_contract::State,
    commits: &'a BTreeMap<&'a str, &'a storage_contract::Commit>,
    chosen: BTreeMap<&'a str, &'a str>,
    cache: BTreeMap<String, String>,
}
impl NodePaths<'_> {
    fn get(&mut self, node: &str) -> String {
        let state = self.state;
        let mut current = node.to_owned();
        let mut trail = Vec::new();
        let mut seen = BTreeSet::new();
        let path = loop {
            if let Some(path) = self.cache.get(&current) {
                break path.clone();
            }
            assert!(
                seen.insert(current.clone()),
                "invariant: admitted file history source chains are acyclic"
            );
            trail.push(current.clone());
            if let Some(path) = state.file_path_nodes.get(&current) {
                break path.path.clone();
            }
            if let Some(version) = state.versions.get(&current) {
                break self
                    .commits
                    .get(current.as_str())
                    .and_then(|commit| {
                        commit.entries.iter().find(|entry| {
                            entry.file_id == version.file_id && entry.version_id == current
                        })
                    })
                    .map(|entry| entry.path().to_owned())
                    .unwrap_or_else(|| version.creation_name.clone());
            }
            if let Some(metadata) = state.file_metadata_nodes.get(&current) {
                let source = if metadata.kind == storage_contract::FileMetadataKind::Merge {
                    self.chosen.get(current.as_str()).copied()
                } else {
                    metadata.parent_node_ids.first().map(String::as_str)
                };
                if let Some(source) = source {
                    current = source.into();
                    continue;
                }
                break state
                    .versions
                    .get(&metadata.version_id)
                    .expect("invariant: metadata node body version exists")
                    .creation_name
                    .clone();
            }
            unreachable!("invariant: admitted file history references an existing node");
        };
        for id in trail {
            self.cache.insert(id, path.clone());
        }
        path
    }
}

pub(super) fn unified_history(
    view: &storage_engine::MetadataView,
    file: &str,
    history: &str,
    versions: &[HistoryVersion],
    first_commits: &BTreeMap<&str, &storage_contract::Commit>,
    current_node: Option<&str>,
) -> Result<Vec<HistoryNode>, ApiError> {
    let state = view.state();
    let mut paths = NodePaths {
        state,
        commits: first_commits,
        chosen: state
            .file_branch_merges
            .values()
            .filter(|merge| !merge.reused_node)
            .map(|merge| (merge.result_node_id.as_str(), merge.chosen_node_id.as_str()))
            .collect(),
        cache: BTreeMap::new(),
    };
    let version_metadata = versions
        .iter()
        .map(|version| (version.data.version_id.as_str(), version))
        .collect::<BTreeMap<_, _>>();
    let path_nodes = state
        .file_path_nodes
        .values()
        .filter(|node| node.file_id == file && node.history_tree_id == history)
        .collect::<Vec<_>>();
    let visible = versions
        .iter()
        .map(|version| version.data.version_id.as_str())
        .chain(path_nodes.iter().map(|node| node.node_id.as_str()))
        .chain(
            state
                .file_metadata_nodes
                .values()
                .filter(|node| node.file_id == file && node.history_tree_id == history)
                .map(|node| node.node_id.as_str()),
        )
        .fold(BTreeMap::<_, usize>::new(), |mut nodes, id| {
            *nodes.entry(id).or_default() += 1;
            nodes
        });
    let node_ids = visible.keys().copied().collect::<Vec<_>>();
    let mut summaries = view
        .project_file_nodes(storage_engine::FileNodeProjectionRequest {
            file_id: file,
            node_ids: &node_ids,
        })
        .into_iter()
        .map(|(id, summary)| {
            let remaining_rows = visible[id.as_str()];
            Ok((
                id,
                PendingSummary {
                    remaining_rows,
                    summary: summary.map_err(error)?,
                },
            ))
        })
        .collect::<Result<_, ApiError>>()?;
    let mut nodes = versions
        .iter()
        .map(|version| {
            let id = version.data.version_id.as_str();
            let commit = first_commits.get(id).copied();
            let summary = take_summary(&mut summaries, id);
            let path = commit
                .and_then(|commit| {
                    commit
                        .entries
                        .iter()
                        .find(|entry| entry.file_id == file && entry.version_id == id)
                })
                .map(|entry| entry.path().to_owned())
                .unwrap_or_else(|| version.data.download_name.clone());
            let sequence = commit.map_or(version.sequence, |commit| commit.sequence);
            (
                sequence,
                id,
                HistoryNode::Content {
                    version: HistoryVersion {
                        data: Arc::clone(&version.data),
                        sequence,
                        current: current_node == Some(id),
                    },
                    node_id: id.into(),
                    kind: ContentNodeKind::Content,
                    tags: summary.tags,
                    parent_node_ids: summary
                        .parent_node_ids
                        .into_iter()
                        .filter(|parent| visible.contains_key(parent.as_str()))
                        .collect(),
                    path,
                    previous_path: None,
                },
            )
        })
        .collect::<Vec<_>>();
    for path in path_nodes {
        let body = &state.versions[&path.version_id];
        let metadata = version_metadata.get(path.version_id.as_str()).copied();
        let summary = take_summary(&mut summaries, &path.node_id);
        nodes.push((
            path.sequence,
            path.node_id.as_str(),
            HistoryNode::Revision(Box::new(HistoryRevision {
                node_id: path.node_id.clone(),
                version_id: path.version_id.clone(),
                file_id: file.into(),
                parent_node_ids: summary
                    .parent_node_ids
                    .into_iter()
                    .filter(|parent| visible.contains_key(parent.as_str()))
                    .collect(),
                kind: RevisionKind::Rename(RenameNodeKind::Rename),
                path: path.path.clone(),
                previous_path: Some(path.previous_path.clone()),
                branch_id: path.branch_id.clone(),
                created_at: utc_timestamp(path.created_at)?,
                sequence: path.sequence,
                current: current_node == Some(path.node_id.as_str()),
                note: state
                    .file_path_notes
                    .get(&path.node_id)
                    .cloned()
                    .unwrap_or_default(),
                size: body.logical_size,
                content_hash: body.content_hash,
                content_state: state.content_states.get(&path.version_id).copied(),
                download_name: metadata
                    .map_or(body.creation_name.as_str(), |version| {
                        version.data.download_name.as_str()
                    })
                    .into(),
                storage_name: body.storage_name.clone(),
                operation_id: path.operation_id,
                author: path.author.clone(),
                message: String::new(),
                tags: summary.tags,
            })),
        ));
    }
    for revision in state
        .file_metadata_nodes
        .values()
        .filter(|node| node.file_id == file && node.history_tree_id == history)
    {
        let body = &state.versions[&revision.version_id];
        let metadata = version_metadata.get(revision.version_id.as_str()).copied();
        let path = paths.get(&revision.node_id);
        let summary = take_summary(&mut summaries, &revision.node_id);
        nodes.push((
            revision.sequence,
            revision.node_id.as_str(),
            HistoryNode::Revision(Box::new(HistoryRevision {
                node_id: revision.node_id.clone(),
                version_id: revision.version_id.clone(),
                file_id: file.into(),
                kind: RevisionKind::Metadata(revision.kind),
                parent_node_ids: summary
                    .parent_node_ids
                    .into_iter()
                    .filter(|parent| visible.contains_key(parent.as_str()))
                    .collect(),
                tags: revision.tags.clone(),
                path,
                previous_path: None,
                branch_id: revision.branch_id.clone(),
                created_at: utc_timestamp(revision.created_at)?,
                sequence: revision.sequence,
                current: current_node == Some(revision.node_id.as_str()),
                note: state
                    .file_path_notes
                    .get(&revision.node_id)
                    .cloned()
                    .unwrap_or_default(),
                size: body.logical_size,
                content_hash: body.content_hash,
                content_state: state.content_states.get(&revision.version_id).copied(),
                download_name: metadata
                    .map_or(body.creation_name.as_str(), |version| {
                        version.data.download_name.as_str()
                    })
                    .into(),
                storage_name: body.storage_name.clone(),
                operation_id: revision.operation_id,
                author: revision.author.clone(),
                message: String::new(),
            })),
        ));
    }
    nodes.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    Ok(nodes.into_iter().map(|(_, _, node)| node).collect())
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/history_nodes_tests.rs"]
mod tests;
