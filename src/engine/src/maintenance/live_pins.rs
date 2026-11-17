//! Additional live roots of a normal content purge, after its existing HEAD,
//! lease, workspace and Git checks. An authenticated receipt retry never
//! selects or reauthorizes a new version here.
use crate::{BranchState, Result, State};
use std::collections::BTreeSet;

pub(super) fn selected(
    state: &State,
    archived: &BTreeSet<String>,
    bodies: &BTreeSet<&str>,
) -> Result<bool> {
    let has_body = |id: &str| bodies.contains(id);
    let has_node = |node: &str| {
        crate::node_retirement_engine::node_version(state, node).is_some_and(has_body)
    };
    let active = |branch: &crate::Branch| {
        branch.state == BranchState::Active && !archived.contains(&branch.branch_id)
    };
    for branch in state.branches.values().filter(|branch| active(branch)) {
        if branch.file_node_id.as_deref().is_some_and(has_node)
            || branch.pending_file_versions.values().any(|id| has_body(id))
            || state.commits.get(&branch.head_commit_id).is_some_and(|head| {
                head.pending_file_versions.values().any(|id| has_body(id))
            })
        {
            return Ok(true);
        }
    }
    if state.file_contexts.values().any(|context| {
        state.branches.get(&context.branch_id).is_some_and(active)
            && context.node_id.as_deref().is_some_and(has_node)
    }) || state.directory_branches.values().any(|branch| {
        branch.file_contexts.values().any(|context| context.node_id.as_deref().is_some_and(has_node))
    }) {
        return Ok(true);
    }
    // Direct directory HEAD entries were checked already. Their frozen child
    // refs and exact retained selections may reach bodies no mutable HEAD uses.
    let roots = state.directory_branches.values()
        .flat_map(|branch| branch.directory_refs.values().map(|reference| reference.tree_id.as_str()))
        .chain(state.directory_tree_selections.values().map(String::as_str));
    crate::node_retirement_engine::any_frozen_body(state, roots, has_body)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/maintenance/live_pins_tests.rs"]
mod tests;
