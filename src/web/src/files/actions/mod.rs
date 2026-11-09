//! File-manager actions. Views call this facade; private modules separate reads,
//! mutations and publication recovery without owning UI state.
mod browse;
mod history;
mod mutation;
mod operation;

pub(crate) use history::{
    anchor_history_node, close_history_field, close_history_relation, history_aux_loading,
    load_file_history, load_history_field, load_history_merges, open_history_merge_inputs,
    open_history_merge_parents, open_history_parents, open_history_version_sources,
    page_history_branches, page_history_nodes, review_history_merge, review_history_tags,
};

pub(crate) use browse::{
    change_page, edit_search, go_directory, go_root, load_disks, load_entries, open_disk,
    preview_directory_branch, preview_directory_snapshot, return_to_default,
    return_to_default_file, search, select_entry,
};
pub(crate) use mutation::{
    commit_directory, create_branch, directory_branch, initialize_directory_git, namespace,
    record_failure, rename_directory_branch, rename_file_branch, select_branch,
    select_default_branch, set_default_file_branch, stage_directory_child, stage_directory_owned,
    update_node_note, update_note,
};
pub(crate) use operation::{poll, record_operation, record_receipt};

use super::model::SaveStatus;
use crate::{api, ui::State};

fn disk_path(slot: &str) -> String {
    format!("/api/file-manager/disks/{}", api::segment(slot))
}

pub(crate) fn history_write_allowed(state: &State) -> bool {
    write_allowed(state)
        && !state.files.history_loading
        && state.files.file_history.ready()
        && !super::model::selected_entry_read_only(&state.files)
}

pub(crate) fn history_navigation_ready(state: &State) -> bool {
    state.session_active()
        && state.files.file_history.ready()
        && !state.files.history_loading
        && state.files.history_editors == 0
        && !pending(state)
}
/// Shared by write actions and their controls. Check before generating IDs,
/// creating drafts, or scheduling any browser/network work.
pub(crate) fn write_allowed(state: &State) -> bool {
    state.files.directory_preview.is_none()
        && state.files.file_preview.is_none()
        && default_write_allowed(state)
        && state.files.writable
}

pub(crate) fn default_write_allowed(state: &State) -> bool {
    state.session_active()
        && state.management.storage.writable()
        && (state.files.writable || state.files.directory_preview.is_some())
        && state
            .files
            .directory_preview
            .as_ref()
            .is_none_or(|preview| preview.branch().is_some())
        && !state.files.loading
        && !pending(state)
}

pub(crate) fn pending(state: &State) -> bool {
    state.files.mutation_pending
        || super::deletion::blocks_writes(state)
        || super::mkdir::blocks_writes(state)
        || state
            .files
            .operations
            .iter()
            .any(|op| op.status() == SaveStatus::Unknown)
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/actions/tests.rs"]
mod tests;
