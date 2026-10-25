//! One presentation graph for content, path changes and metadata revisions.
use super::*;

struct PendingSummary {
    remaining_rows: usize,
    summary: storage_engine::Result<storage_engine::FileNodeSummary>,
}

/// Consume the last row so parent/tag buffers do not live alongside the JSON
/// graph. Repeated or overlapping IDs retain the same facts for earlier rows.
fn take_summary(
    summaries: &mut BTreeMap<String, PendingSummary>,
    node: &str,
) -> Option<storage_engine::FileNodeSummary> {
    let pending = summaries.get_mut(node)?;
    if pending.remaining_rows > 1 {
        pending.remaining_rows -= 1;
        pending.summary.as_ref().ok().cloned()
    } else {
        summaries.remove(node)?.summary.ok()
    }
}

/// Metadata UUID order is unrelated to graph order. Resolve a path through the
/// explicit source chain and memoize it without recursive stack growth.
struct NodePaths<'a> {
    engine: &'a Engine,
    commits: &'a BTreeMap<&'a str, &'a storage_contract::Commit>,
    chosen: BTreeMap<&'a str, &'a str>,
    cache: BTreeMap<String, String>,
}
impl NodePaths<'_> {
    fn get(&mut self, node: &str) -> String {
        let state = self.engine.state();
        let mut current = node.to_owned();
        let mut trail = Vec::new();
        let mut seen = BTreeSet::new();
        let path = loop {
            if let Some(path) = self.cache.get(&current) {
                break path.clone();
            }
            if !seen.insert(current.clone()) {
                break String::new();
            }
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
                    .map(|entry| entry.path.clone())
                    .unwrap_or_else(|| version.creation_name.clone());
            }
            if let Some(metadata) = state.file_metadata_nodes.get(&current) {
                let source = if metadata.kind == "merge" {
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
                    .map(|body| body.creation_name.clone())
                    .unwrap_or_default();
            }
            break String::new();
        };
        for id in trail {
            self.cache.insert(id, path.clone());
        }
        path
    }
}

pub(super) fn unified_history(
    engine: &Engine,
    file: &str,
    history: &str,
    versions: &[Value],
    first_commits: &BTreeMap<&str, &storage_contract::Commit>,
    current_node: Option<&str>,
) -> Result<Vec<Value>, ApiError> {
    let state = engine.state();
    let mut paths = NodePaths {
        engine,
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
        .filter_map(|version| version["version_id"].as_str().map(|id| (id, version)))
        .collect::<BTreeMap<_, _>>();
    let path_nodes = state
        .file_path_nodes
        .values()
        .filter(|node| node.file_id == file && node.history_tree_id == history)
        .collect::<Vec<_>>();
    let visible = versions
        .iter()
        .filter_map(|version| version["version_id"].as_str())
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
    let mut summaries = engine
        .project_file_nodes(storage_engine::FileNodeProjectionRequest {
            file_id: file,
            node_ids: &node_ids,
        })
        .into_iter()
        .map(|(id, summary)| {
            let remaining_rows = visible[id.as_str()];
            (
                id,
                PendingSummary {
                    remaining_rows,
                    summary,
                },
            )
        })
        .collect();
    let mut nodes = versions
        .iter()
        .map(|version| {
            let mut node = version.clone();
            let id = version["version_id"].as_str().unwrap_or("");
            let commit = first_commits.get(id).copied();
            let summary = take_summary(&mut summaries, id).unwrap_or_default();
            node["node_id"] = id.into();
            node["kind"] = "content".into();
            node["tags"] = json!(summary.tags);
            node["parent_node_ids"] = json!(
                summary
                    .parent_node_ids
                    .into_iter()
                    .filter(|parent| visible.contains_key(parent.as_str()))
                    .collect::<Vec<_>>()
            );
            node["path"] = commit
                .and_then(|commit| {
                    commit
                        .entries
                        .iter()
                        .find(|entry| entry.file_id == file && entry.version_id == id)
                })
                .map(|entry| entry.path.clone())
                .unwrap_or_else(|| version["download_name"].as_str().unwrap_or("").to_owned())
                .into();
            node["previous_path"] = Value::Null;
            node["sequence"] = commit
                .map(|commit| commit.sequence.to_string())
                .unwrap_or_else(|| version["sequence"].as_str().unwrap_or("0").to_owned())
                .into();
            node["current"] = (current_node == Some(id)).into();
            node
        })
        .collect::<Vec<_>>();
    for path in path_nodes {
        let Some(body) = state.versions.get(&path.version_id) else {
            continue;
        };
        let metadata = version_metadata.get(path.version_id.as_str()).copied();
        let summary = take_summary(&mut summaries, &path.node_id).unwrap_or_default();
        nodes.push(json!({
            "node_id":path.node_id,"version_id":path.version_id,"file_id":file,
            "parent_node_ids":summary.parent_node_ids.into_iter().filter(|parent|visible.contains_key(parent.as_str())).collect::<Vec<_>>(),
            "kind":"rename","path":path.path,"previous_path":path.previous_path,
            "branch_id":path.branch_id,"created_at":utc_timestamp(path.created_at)?,"sequence":path.sequence.to_string(),
            "current":current_node==Some(path.node_id.as_str()),
            "note":state.file_path_notes.get(&path.node_id).map(String::as_str).unwrap_or(""),
            "size":body.logical_size.to_string(),"content_hash":body.content_hash,
            "content_state":state.content_states.get(&path.version_id).map(String::as_str).unwrap_or("LIVE"),
            "download_name":metadata.and_then(|version|version.get("download_name")).cloned().unwrap_or_else(||body.creation_name.clone().into()),
            "storage_name":body.storage_name,
            "operation_id":path.operation_id,"author":path.author,"message":""
            ,"tags":summary.tags
        }));
    }
    for revision in state
        .file_metadata_nodes
        .values()
        .filter(|node| node.file_id == file && node.history_tree_id == history)
    {
        let Some(body) = state.versions.get(&revision.version_id) else {
            continue;
        };
        let metadata = version_metadata.get(revision.version_id.as_str()).copied();
        let path = paths.get(&revision.node_id);
        let summary = take_summary(&mut summaries, &revision.node_id).unwrap_or_default();
        nodes.push(json!({"node_id":revision.node_id,"version_id":revision.version_id,"file_id":file,
            "kind":revision.kind,"parent_node_ids":summary.parent_node_ids.into_iter().filter(|parent|visible.contains_key(parent.as_str())).collect::<Vec<_>>(),
            "tags":revision.tags,"path":path,"previous_path":null,"branch_id":revision.branch_id,
            "created_at":utc_timestamp(revision.created_at)?,"sequence":revision.sequence.to_string(),"current":current_node==Some(revision.node_id.as_str()),
            "note":state.file_path_notes.get(&revision.node_id).map(String::as_str).unwrap_or(""),
            "size":body.logical_size.to_string(),"content_hash":body.content_hash,"content_state":state.content_states.get(&revision.version_id).map(String::as_str).unwrap_or("LIVE"),
            "download_name":metadata.and_then(|version|version.get("download_name")).cloned().unwrap_or_else(||body.creation_name.clone().into()),
            "storage_name":body.storage_name,"operation_id":revision.operation_id,"author":revision.author,"message":""}));
    }
    nodes.sort_by(|left, right| {
        let sequence = |node: &Value| {
            node["sequence"]
                .as_str()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0)
        };
        sequence(right)
            .cmp(&sequence(left))
            .then_with(|| left["node_id"].as_str().cmp(&right["node_id"].as_str()))
    });
    Ok(nodes)
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/history_nodes_tests.rs"]
mod tests;
