//! Pure metadata projection after retaining one generation under admission.
use crate::HistoryGraphSnapshot;
use std::sync::Arc;
use storage_contract::State;

/// Recheck the actual history owner and split scope, not a global publication
/// counter. Ordinary writes may advance while the old projection is produced.
pub(super) fn authorize_current(
    state: &State,
    captured: &storage_contract::HistoryTree,
) -> Result<(), crate::ApiError> {
    super::repo_for_history_in_state(state, &captured.history_tree_id)?;
    let current = &state.histories[&captured.history_tree_id];
    if current.repository_id != captured.repository_id || current.generation != captured.generation
    {
        return Err(crate::ApiError::conflict(
            storage_contract::ErrorCode::TreeMoved,
            "history scope changed during the read",
        ));
    }
    Ok(())
}

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

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/detached_history_tests.rs"]
pub(super) mod tests;
