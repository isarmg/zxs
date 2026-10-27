//! Call-local container descriptors selected from State-bound memberships.
use crate::{
    Branch, FileBranchMerge, FileContext, FileMetadataNode, FilePathNode, FileVersion, State,
    business::file_nodes::NodeProjection, state_projection::StateProjection,
};
use json_codec::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
struct HistoryRows<'a> {
    versions: Vec<&'a FileVersion>,
    pending: Vec<(&'a Branch, &'a String)>,
    paths: Vec<&'a FilePathNode>,
    metadata: Vec<&'a FileMetadataNode>,
    merges: Vec<&'a FileBranchMerge>,
}

pub(super) struct EncodedHistory {
    pub(super) versions: Vec<Value>,
    pub(super) inherited: Vec<Value>,
    pub(super) paths: Vec<Value>,
    pub(super) metadata: Vec<Value>,
    pub(super) merges: Value,
}

impl HistoryRows<'_> {
    fn encode(&mut self, state: &State, nodes: &mut NodeProjection<'_>) -> EncodedHistory {
        self.versions.sort_by(|left, right| {
            left.storage_name
                .cmp(&right.storage_name)
                .then_with(|| left.version_id.cmp(&right.version_id))
        });
        let versions = self.versions.iter().map(|version| json!({
            "version_id": version.version_id,
            "parent_node_ids": nodes.parents(&version.version_id),
            "tags": nodes.tags(&version.version_id),
            "storage_name": version.storage_name,
            "created_at": version.created_at,
            "size": version.logical_size.to_string(),
            "content_hash": version.content_hash,
            "note": state.version_notes.get(&version.version_id).map(String::as_str).unwrap_or(""),
            "content_state": state.content_states.get(&version.version_id),
        })).collect();
        let inherited = self
            .pending
            .iter()
            .map(|(branch, source)| {
                json!({
                    "branch_id": branch.branch_id,
                    "name": branch.name,
                    "state": branch.state,
                    "source_node_id": branch.file_node_id.as_ref().unwrap_or(source),
                    "source_version_id": source,
                    "version_id": source,
                    "own_version_id": null,
                    "inherited": true,
                    "materialized": false,
                })
            })
            .collect();
        self.paths.sort_by(|left, right| {
            left.sequence
                .cmp(&right.sequence)
                .then_with(|| left.node_id.cmp(&right.node_id))
        });
        let paths = self.paths.iter().map(|node| json!({
            "node": node,
            "tags": nodes.tags(&node.node_id),
            "note": state.file_path_notes.get(&node.node_id).map(String::as_str).unwrap_or(""),
        })).collect();
        let metadata = self.metadata.iter().map(|node| json!({
            "node": node,
            "note": state.file_path_notes.get(&node.node_id).map(String::as_str).unwrap_or(""),
        })).collect();
        EncodedHistory {
            versions,
            inherited,
            paths,
            metadata,
            merges: json!(self.merges),
        }
    }
}

pub(super) struct FileDescription<'a> {
    histories: BTreeMap<&'a str, HistoryRows<'a>>,
    pub(super) contexts: BTreeMap<&'a String, &'a FileContext>,
    pub(super) aliases: BTreeMap<&'a String, &'a String>,
}

impl<'a> FileDescription<'a> {
    pub(super) fn new(view: StateProjection<'a>, file: &str) -> Self {
        let state = view.state();
        let mut result = Self {
            histories: BTreeMap::new(),
            contexts: BTreeMap::new(),
            aliases: BTreeMap::new(),
        };
        for key in view.owners().versions(file) {
            let version = &state.versions[key];
            result
                .histories
                .entry(&version.history_tree_id)
                .or_default()
                .versions
                .push(version);
        }
        if let Some(rows) = view.owners().file_rows(file) {
            for key in rows.pending_branches.keys() {
                let branch = &state.branches[key];
                let source = branch
                    .pending_file_versions
                    .get(file)
                    .expect("invariant: indexed pending branch contains the file");
                result
                    .histories
                    .entry(&branch.history_tree_id)
                    .or_default()
                    .pending
                    .push((branch, source));
            }
            for key in rows.path_nodes.keys() {
                let node = &state.file_path_nodes[key];
                result
                    .histories
                    .entry(&node.history_tree_id)
                    .or_default()
                    .paths
                    .push(node);
            }
            for key in rows.metadata_nodes.keys() {
                let node = &state.file_metadata_nodes[key];
                result
                    .histories
                    .entry(&node.history_tree_id)
                    .or_default()
                    .metadata
                    .push(node);
            }
            for key in rows.merges.keys() {
                let event = &state.file_branch_merges[key];
                result
                    .histories
                    .entry(&event.history_tree_id)
                    .or_default()
                    .merges
                    .push(event);
            }
            for key in rows.contexts.keys() {
                let (key, context) = state
                    .file_contexts
                    .get_key_value(key)
                    .expect("invariant: indexed context exists");
                result.contexts.insert(key, context);
            }
            for key in rows.aliases.keys() {
                let (key, target) = state
                    .file_branch_aliases
                    .get_key_value(key)
                    .expect("invariant: indexed alias source exists");
                result.aliases.insert(key, target);
            }
        }
        result
    }

    pub(super) fn encode_history(
        &mut self,
        history: &str,
        state: &State,
        nodes: &mut NodeProjection<'_>,
    ) -> EncodedHistory {
        match self.histories.get_mut(history) {
            Some(rows) => rows.encode(state, nodes),
            None => HistoryRows::default().encode(state, nodes),
        }
    }
}
