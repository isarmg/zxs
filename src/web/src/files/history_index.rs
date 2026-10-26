//! Page-owned lookup positions borrow records from the existing immutable snapshot.
use super::model::{history_node_id, history_nodes_ref};
use crate::{snapshot::Snapshot, ui::State};
use json_codec::Value;
use std::collections::BTreeMap;
use web_interface::prelude::*;

#[derive(PartialEq)]
pub(super) struct HistoryIndex {
    pub(super) snapshot: Snapshot<Value>,
    nodes: BTreeMap<String, usize>,
    branches: BTreeMap<String, usize>,
    versions: BTreeMap<String, usize>,
}

impl HistoryIndex {
    pub(super) fn new(snapshot: Snapshot<Value>) -> Self {
        let mut nodes = BTreeMap::new();
        for (position, node) in history_nodes_ref(&snapshot).iter().enumerate() {
            // Keep the same first-match semantics as the previous linear lookup.
            nodes.entry(history_node_id(node)).or_insert(position);
        }
        let mut branches = BTreeMap::new();
        for (position, branch) in snapshot["branches"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            if let Some(id) = branch["branch_id"].as_str() {
                branches.entry(id.to_owned()).or_insert(position);
            }
        }
        let mut versions = BTreeMap::new();
        for (position, version) in snapshot["versions"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            if let Some(id) = version["version_id"].as_str() {
                versions.entry(id.to_owned()).or_insert(position);
            }
        }
        Self {
            snapshot,
            nodes,
            branches,
            versions,
        }
    }

    pub(super) fn node(&self, id: &str) -> Option<(usize, &Value)> {
        self.nodes
            .get(id)
            .map(|&position| (position, &history_nodes_ref(&self.snapshot)[position]))
    }

    pub(super) fn version(&self, id: &str) -> Option<&Value> {
        self.versions
            .get(id)
            .map(|&position| &self.snapshot["versions"][position])
    }

    pub(super) fn branch(&self, id: &str) -> Option<&Value> {
        self.branches
            .get(id)
            .map(|&position| &self.snapshot["branches"][position])
    }
}

#[derive(Clone, Copy)]
struct SharedHistory(Memo<HistoryIndex>);

pub(super) fn provide(state: RwSignal<State>) {
    let snapshot = Memo::new(move |_| state.with(|s| s.files.file_history.clone()));
    provide_context(SharedHistory(Memo::new(move |_| {
        HistoryIndex::new(snapshot.get())
    })));
}

pub(super) fn shared() -> Memo<HistoryIndex> {
    use_context::<SharedHistory>()
        .expect("invariant: file page provides its history index")
        .0
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/history_index/tests.rs"]
mod tests;
