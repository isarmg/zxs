//! File-manager actions. Views call this facade; private modules separate reads,
//! mutations and publication recovery without owning UI state.
mod browse;
mod mutation;
mod operation;

pub(crate) use browse::{
    change_page, go_directory, go_root, load_disks, load_entries, open_disk, search, select_entry,
};
pub(crate) use mutation::{
    create_branch, directory_branch, namespace, rename_directory_branch, rename_file_branch,
    select_branch, select_default_branch, update_node_note, update_note,
};
pub(crate) use operation::{poll, record_operation, record_receipt};

use super::model::SaveStatus;
use crate::{api, ui::State};

fn disk_path(slot: &str) -> String {
    format!("/api/file-manager/disks/{}", api::segment(slot))
}
/// Shared by write actions and their controls. Check before generating IDs,
/// creating drafts, or scheduling any browser/network work.
pub(crate) fn write_allowed(state: &State) -> bool {
    state.session_active()
        && state.management.storage.writable()
        && state.files.writable
        && !state.files.loading
        && !pending(state)
}

pub(crate) fn pending(state: &State) -> bool {
    state.files.mutation_pending
        || state
            .files
            .operations
            .iter()
            .any(|op| op.status() == SaveStatus::Unknown)
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/actions/tests.rs"]
mod tests;
