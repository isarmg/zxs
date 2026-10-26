//! Pure same-state capture; sorting and wire serialization need no admission lock.
use crate::HistoryGraphSnapshot;
use std::sync::Arc;
use storage_contract::State;

/// Caller has authorized this history and its repository in this same State.
pub(super) fn capture(state: &State, history: &str) -> HistoryGraphSnapshot {
    HistoryGraphSnapshot {
        history: state.histories[history].clone(),
        commits: state
            .commits
            .shared_values()
            .filter(|commit| commit.history_tree_id == history)
            .map(Arc::clone)
            .collect(),
        branches: state
            .branches
            .values()
            .filter(|branch| branch.history_tree_id == history)
            .cloned()
            .collect(),
        merge_links: state
            .merge_links
            .values()
            .filter(|link| link.history_tree_id == history)
            .cloned()
            .collect(),
        boundary_marks: state
            .history_boundaries
            .values()
            .filter(|mark| mark.history_tree_id == history)
            .cloned()
            .collect(),
    }
}
