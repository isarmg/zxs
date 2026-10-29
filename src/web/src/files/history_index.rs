//! Page-owned lookup positions borrow records from the resident window. Only
//! the resident page is indexed; a replaced page drops its index with it.
use super::history::{FileBranch, HistoryNodeSummary, HistoryWindow};
use crate::{snapshot::Snapshot, ui::State};
use std::collections::BTreeMap;
use web_interface::prelude::*;

#[derive(PartialEq)]
pub(super) struct HistoryIndex {
    /// The resident window, current or stale; none while unloaded/loading.
    window: Option<Snapshot<HistoryWindow>>,
    nodes: BTreeMap<String, usize>,
}

impl HistoryIndex {
    pub(super) fn new(window: Option<Snapshot<HistoryWindow>>) -> Self {
        let nodes = window
            .as_deref()
            .map(|window| {
                window
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(position, node)| (node.node_id.clone(), position))
                    .collect()
            })
            .unwrap_or_default();
        Self { window, nodes }
    }

    pub(super) fn node(&self, id: &str) -> Option<(usize, &HistoryNodeSummary)> {
        let window = self.window.as_deref()?;
        self.nodes
            .get(id)
            .map(|&position| (position, &window.nodes[position]))
    }

    /// Node identities of the resident page.
    #[cfg(test)]
    pub(super) fn node_ids(&self) -> impl Iterator<Item = &str> {
        self.nodes.keys().map(String::as_str)
    }
}

/// Branch lookups come from the resident branch page, never from the window.
pub(super) fn branch<'a>(state: &'a State, id: &str) -> Option<&'a FileBranch> {
    state.files.history.branch_page()?.branch(id)
}

#[derive(Clone, Copy)]
struct SharedHistory(Memo<HistoryIndex>);

fn resident(state: &State) -> Option<Snapshot<HistoryWindow>> {
    state.files.history.window().cloned()
}

pub(super) fn provide(state: RwSignal<State>) {
    // Snapshot equality is allocation identity: unrelated State updates and
    // auxiliary pages do not rebuild the node index.
    let window = Memo::new(move |_| state.with(resident));
    provide_context(SharedHistory(Memo::new(move |_| {
        HistoryIndex::new(window.get())
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
