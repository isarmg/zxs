//! Page-owned lookup positions borrow records from the existing immutable snapshot.
use super::history::{FileHistory, FileNode};
use crate::{snapshot::Snapshot, ui::State};
use std::collections::BTreeMap;
use web_interface::prelude::*;

#[derive(PartialEq)]
pub(super) struct HistoryIndex {
    pub(super) snapshot: Snapshot<FileHistory>,
    nodes: BTreeMap<String, usize>,
    versions: BTreeMap<String, usize>,
}

impl HistoryIndex {
    pub(super) fn new(snapshot: Snapshot<FileHistory>) -> Self {
        let mut nodes = BTreeMap::new();
        for (position, node) in snapshot.nodes().iter().enumerate() {
            nodes.insert(node.id().to_owned(), position);
        }
        let mut versions = BTreeMap::new();
        for (position, node) in snapshot.nodes().iter().enumerate() {
            if let Some(id) = node.version_id()
                && node.body_note().is_some()
            {
                versions.entry(id.to_owned()).or_insert(position);
            }
        }
        Self {
            snapshot,
            nodes,
            versions,
        }
    }

    pub(super) fn node(&self, id: &str) -> Option<(usize, &FileNode)> {
        self.nodes
            .get(id)
            .map(|&position| (position, &self.snapshot.nodes()[position]))
    }

    pub(super) fn version(&self, id: &str) -> Option<&FileNode> {
        self.versions
            .get(id)
            .map(|&position| &self.snapshot.nodes()[position])
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
