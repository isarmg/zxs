//! Root-directory summaries borrowed and encoded outside the writer mutex.
use super::*;
use crate::{DefaultDirectoryBranchRow, DefaultDirectoryBranchSnapshot, SelectedDirectoryBranch};

pub(super) fn read(
    view: storage_engine::PublishedReadView,
    repository_id: &str,
    control: &crate::HistoryReadControl,
) -> Result<DefaultDirectoryBranchSnapshot, ApiError> {
    file_manager::activity::checkpoint(control)?;
    let state = view.state();
    let repo = state
        .repositories
        .get(repository_id)
        .expect("invariant: admitted repository belongs to the captured publication");
    let selected_id = state.directory_selections.get(repository_id);
    let selected = selected_id.map(|id| {
        let branch = state
            .directory_branches
            .get(id)
            .expect("invariant: admitted directory selection references its branch");
        SelectedDirectoryBranch {
            directory_branch_id: id,
            name: &branch.name,
        }
    });
    let mut rows = Vec::new();
    for branch in state.directory_branches.values() {
        file_manager::activity::checkpoint(control)?;
        if branch.repository_id == repository_id && branch.directory_id == repository_id {
            rows.push(DefaultDirectoryBranchRow {
                branch,
                current: selected_id == Some(&branch.directory_branch_id),
            });
        }
    }
    file_manager::activity::checkpoint(control)?;
    // A fixed total-order comparator; sorting is a non-interruptible segment.
    rows.sort_by(|left, right| {
        (
            std::cmp::Reverse(left.branch.sequence),
            &left.branch.directory_branch_id,
        )
            .cmp(&(
                std::cmp::Reverse(right.branch.sequence),
                &right.branch.directory_branch_id,
            ))
    });
    file_manager::activity::checkpoint(control)?;
    // Borrowed encoding is one cooperative segment with a bounded byte output.
    let snapshot = DefaultDirectoryBranchSnapshot::encode(
        &repo.repository_id,
        &repo.main_branch_id,
        selected,
        &rows,
    )?;
    file_manager::activity::checkpoint(control)?;
    Ok(snapshot)
}
