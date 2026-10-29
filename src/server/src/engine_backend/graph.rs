//! Pure same-state capture; sorting and wire serialization need no admission lock.
use crate::HistoryGraphSnapshot;
use storage_engine::Engine;

/// Caller has authorized this history and its repository in this same State.
pub(super) fn capture(engine: &Engine, history: &str) -> HistoryGraphSnapshot {
    let state = engine.state();
    HistoryGraphSnapshot {
        history: state.histories[history].clone(),
        commits: engine.history_commit_records(history),
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
