//! Capture public branch metadata without constructing or modifying JSON.
use crate::{BranchSnapshot, ContentDispositionSnapshot, FileBranchSnapshot};
use storage_contract::State;
use storage_contract::{Branch, BranchState};
use storage_engine::Engine;

pub(super) fn capture(engine: &Engine, branch: &Branch) -> storage_engine::Result<BranchSnapshot> {
    capture_owned(engine, branch.clone())
}
pub(super) fn capture_owned(
    engine: &Engine,
    branch: Branch,
) -> storage_engine::Result<BranchSnapshot> {
    let state = engine.state();
    let Branch {
        branch_id,
        history_tree_id,
        name,
        head_commit_id,
        fork_commit_id,
        branch_kind,
        file_id,
        file_node_id,
        pending_file_versions,
        generation,
        state: branch_state,
    } = branch;
    let snapshot = state
        .commits
        .get(&head_commit_id)
        .expect("invariant: admitted branch head references an existing commit");
    let mut pending = snapshot.pending_file_versions.clone();
    pending.extend(pending_file_versions);
    let file = match file_id.as_ref() {
        Some(file) => {
            // A file branch's head holds its one file entry; this early-stop
            // walk reads a single page.
            let visible = engine
                .commit_namespace(&head_commit_id)
                .expect("invariant: admitted branch head references an existing commit")
                .find_file(file)?;
            let visible = visible.as_ref().map(|entry| entry.version_id.as_str());
            let content = capture_content(state, visible, pending.get(file).map(String::as_str));
            let alias_of = state.file_branch_aliases.get(&branch_id).cloned();
            Some(FileBranchSnapshot {
                node_id: file_node_id.clone().or_else(|| visible.map(str::to_owned)),
                content,
                read_only: branch_state != BranchState::Active || alias_of.is_some(),
                alias_of,
            })
        }
        None => None,
    };
    Ok(BranchSnapshot {
        branch_id,
        history_tree_id,
        name,
        head_commit_id,
        fork_commit_id,
        branch_kind,
        file_id,
        file_node_id,
        pending_file_versions: pending,
        generation,
        state: branch_state,
        file,
    })
}

pub(super) fn capture_content(
    state: &State,
    visible: Option<&str>,
    source: Option<&str>,
) -> ContentDispositionSnapshot {
    // An explicit empty source suppresses visible fallback, as on the wire.
    let selected = source.or(visible).filter(|version| !version.is_empty());
    ContentDispositionSnapshot {
        version_id: selected.map(str::to_owned),
        source_version_id: source.map(str::to_owned),
        stored_state: selected
            .and_then(|version| state.content_states.get(version))
            .copied(),
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/branch_tests.rs"]
mod tests;
