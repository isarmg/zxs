//! Detached complete branch metadata reads over one admitted publication.
use super::*;
use crate::backend_branch::{BorrowedBranch, PendingUnion};
use storage_engine::PublishedReadView;

fn row<'a>(
    view: &'a PublishedReadView,
    branch: &'a storage_contract::Branch,
    control: &crate::HistoryReadControl,
) -> Result<BorrowedBranch<'a>, ApiError> {
    file_manager::activity::checkpoint(control)?;
    let state = view.state();
    let head = state
        .commits
        .get(&branch.head_commit_id)
        .expect("invariant: admitted branch head references an existing commit");
    let pending = PendingUnion::new(&head.pending_file_versions, &branch.pending_file_versions);
    let file = match branch.file_id.as_deref() {
        Some(file) => {
            let observe = || control.checkpoint().map_err(storage_engine::Error::from);
            // Always resolve the visible HEAD, even with pending or an explicit
            // node. This canonical early-stop walk can read multiple pages.
            let visible = view
                .commit_namespace(&branch.head_commit_id, &observe)
                .expect("invariant: admitted branch head references an existing commit")
                .find_file(file)
                .map_err(file_manager::activity::read_error)?;
            let visible = visible.as_ref().map(|entry| entry.version_id.as_str());
            let alias_of = state.file_branch_aliases.get(&branch.branch_id).cloned();
            Some(crate::FileBranchSnapshot {
                content: branch::capture_content(state, visible, pending.get(file)),
                node_id: branch
                    .file_node_id
                    .clone()
                    .or_else(|| visible.map(str::to_owned)),
                read_only: branch.state != storage_contract::BranchState::Active
                    || alias_of.is_some(),
                alias_of,
            })
        }
        None => None,
    };
    Ok(BorrowedBranch {
        branch,
        pending,
        file,
    })
}

pub(super) fn single(
    view: PublishedReadView,
    id: &str,
    control: &crate::HistoryReadControl,
) -> Result<crate::BranchMetadataSnapshot, ApiError> {
    let branch = view
        .state()
        .branches
        .get(id)
        .expect("invariant: admitted branch belongs to the captured publication");
    let row = row(&view, branch, control)?;
    file_manager::activity::checkpoint(control)?;
    let encoded = crate::BranchMetadataSnapshot::encode_borrowed(row)?;
    file_manager::activity::checkpoint(control)?;
    Ok(encoded)
}

pub(super) fn list(
    view: PublishedReadView,
    repository_id: &str,
    control: &crate::HistoryReadControl,
) -> Result<crate::BranchMetadataSnapshot, ApiError> {
    file_manager::activity::checkpoint(control)?;
    let repo = view
        .state()
        .repositories
        .get(repository_id)
        .expect("invariant: admitted repository belongs to the captured publication");
    let mut rows = Vec::new();
    for branch in view.state().branches.values() {
        file_manager::activity::checkpoint(control)?;
        if branch.history_tree_id == repo.history_tree_id {
            // Preserve BTreeMap order and include migrated rows. Resolve every
            // namespace before encoding so its error still precedes the cap.
            rows.push(row(&view, branch, control)?);
        }
    }
    file_manager::activity::checkpoint(control)?;
    let encoded = crate::BranchMetadataSnapshot::encode_borrowed_list(rows)?;
    file_manager::activity::checkpoint(control)?;
    Ok(encoded)
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/branch_metadata_tests.rs"]
mod tests;
