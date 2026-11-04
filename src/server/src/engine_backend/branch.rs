//! Capture public branch metadata without constructing or modifying JSON.
use crate::{BranchSnapshot, ContentDispositionSnapshot, FileBranchSnapshot};
use storage_contract::State;
use storage_contract::{Branch, BranchState};

pub(super) fn capture(state: &State, branch: &Branch) -> BranchSnapshot {
    capture_owned(state, branch.clone())
}
pub(super) fn capture_current(
    engine: &storage_engine::Engine,
    branch: &Branch,
) -> Result<BranchSnapshot, crate::ApiError> {
    // The repository branch catalog still reports migrated records as history;
    // direct branch admission rejects them before calling this projection.
    if branch.state == BranchState::Migrated {
        return Ok(capture(engine.state(), branch));
    }
    let namespace = engine
        .namespace_tree(&branch.branch_id)
        .map_err(super::error)?;
    let commit = &engine.state().commits[&branch.head_commit_id];
    let pending = super::wire::PendingSources::new(
        engine,
        &branch.branch_id,
        commit,
        super::wire::PendingScope::Current(namespace),
    )
    .pending_versions();
    Ok(capture_entries(
        engine.state(),
        branch.clone(),
        namespace.entries(),
        pending,
    ))
}

pub(super) fn capture_owned(state: &State, mut branch: Branch) -> BranchSnapshot {
    let snapshot = state
        .commits
        .get(&branch.head_commit_id)
        .expect("invariant: admitted branch head references an existing commit");
    let mut pending = snapshot.pending_file_versions.clone();
    pending.extend(std::mem::take(&mut branch.pending_file_versions));
    capture_entries(state, branch, &snapshot.entries, pending)
}

fn capture_entries(
    state: &State,
    branch: Branch,
    entries: &[storage_contract::Entry],
    pending: std::collections::BTreeMap<String, String>,
) -> BranchSnapshot {
    let Branch {
        branch_id,
        history_tree_id,
        name,
        head_commit_id,
        fork_commit_id,
        branch_kind,
        file_id,
        file_node_id,
        pending_file_versions: _,
        generation,
        state: branch_state,
    } = branch;
    let file = file_id.as_ref().map(|file| {
        let visible = entries
            .iter()
            .find(|entry| &entry.file_id == file)
            .map(|entry| entry.version_id.as_str());
        let content = capture_content(state, visible, pending.get(file).map(String::as_str));
        let alias_of = state.file_branch_aliases.get(&branch_id).cloned();
        FileBranchSnapshot {
            node_id: file_node_id.clone().or_else(|| visible.map(str::to_owned)),
            content,
            read_only: branch_state != BranchState::Active || alias_of.is_some(),
            alias_of,
        }
    });
    BranchSnapshot {
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
    }
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
