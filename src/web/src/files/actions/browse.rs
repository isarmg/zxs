use super::{disk_path, load_file_history};
use crate::{
    api,
    files::{
        model::{Disk, Navigation},
        page::{DirectoryKey, Entry, Page},
    },
    ui::{ErrorContext, State, accept_read_response, current_session, fail, fail_in},
};
use web_interface::tasks::spawn_local;
use web_interface::{i18n::t, prelude::*};

fn begin_disk_load(state: &mut State) -> Option<(u64, u64)> {
    if !state.session_active() || state.files.loading_disks {
        return None;
    }
    state.files.loading_disks = true;
    Some((state.session_epoch, state.files.disks_generation))
}

fn complete_disk_load(
    state: &mut State,
    epoch: u64,
    generation: u64,
    result: Result<Vec<Disk>, api::Error>,
) -> Result<bool, api::Error> {
    if !state.session_active() || state.session_epoch != epoch {
        return Ok(false);
    }
    state.files.loading_disks = false;
    if state.files.disks_generation != generation
        && !matches!(&result, Err(error) if error.status == 401)
    {
        state.files.disks_loaded = false;
        return Ok(state.management.storage.readable());
    }
    match result {
        Ok(disks) => {
            state.files.disks = disks;
            state.files.disks_loaded = true;
            Ok(false)
        }
        Err(error) => {
            state.files.disks_loaded = false;
            Err(error)
        }
    }
}

pub(crate) fn load_disks(state: RwSignal<State>) {
    let Some((epoch, generation)) = state.try_update(begin_disk_load).flatten() else {
        return;
    };
    spawn_local(async move {
        let result = api::request("GET", "/api/file-manager/disks", None)
            .await
            .and_then(super::super::model::parse_disks);
        match state.try_update(|s| complete_disk_load(s, epoch, generation, result)) {
            Some(Ok(true)) => load_disks(state),
            Some(Err(error)) => fail_in(
                state,
                epoch,
                ErrorContext::Read("/api/file-manager/disks".into()),
                error,
            ),
            Some(Ok(false)) | None => {}
        }
    });
}

pub(crate) fn open_disk(state: RwSignal<State>, slot: String) {
    open_disk_directory(state, slot, String::new());
}

pub(crate) fn open_disk_directory(state: RwSignal<State>, slot: String, directory: String) {
    state.update(|s| {
        s.notices.leave();
        s.files.navigate(Navigation::Disk(slot));
        s.files.directory = directory;
        s.change_page(crate::ui::Page::Files);
    });
    load_entries(state);
}
pub(crate) fn go_root(state: RwSignal<State>) {
    state.update(|s| {
        s.notices.leave();
        s.files.navigate(Navigation::Root);
        s.change_page(crate::ui::Page::Files);
    });
    load_disks(state);
}
pub(crate) fn go_directory(state: RwSignal<State>, directory: String) {
    navigate_directory(state, directory, crate::ui::Page::Files);
}

pub(crate) fn go_directory_details(state: RwSignal<State>, directory: String) {
    navigate_directory(state, directory, crate::ui::Page::Details);
}

fn navigate_directory(state: RwSignal<State>, directory: String, page: crate::ui::Page) {
    state.update(|s| {
        s.notices.leave();
        s.files.navigate(Navigation::Directory(directory));
        s.change_page(page);
    });
    load_entries(state);
}

pub(crate) fn refresh_directory(state: RwSignal<State>) {
    if !state.with_untracked(|s| {
        s.session_active() && !s.files.slot.is_empty() && !s.files.loading && !super::pending(s)
    }) {
        return;
    }
    state.update(|s| {
        s.notices.leave();
        s.files.reset_page();
        s.files.clear_selection();
        s.files.batch = None;
        s.files.deletion.cancel_review();
        s.files.entries = Default::default();
        s.files.loaded = false;
    });
    load_entries(state);
}

pub(crate) fn load_entries(state: RwSignal<State>) {
    request_page(state, false);
}

pub(crate) fn reload_entries(state: RwSignal<State>) {
    state.update(|s| s.files.reset_page());
    request_page(state, false);
}

pub(crate) fn edit_search(state: RwSignal<State>, query: String) {
    state.update(|s| s.files.query_draft = query);
}

pub(crate) fn search(state: RwSignal<State>, query: String) {
    state.update(|s| {
        s.notices.leave();
        s.files.cancel_listing();
        s.files.reset_page();
        s.files.query_draft.clone_from(&query);
        s.files.query = query;
        s.files.entries = Default::default();
        s.files.loaded = false;
        s.files.clear_selection();
    });
    request_page(state, false);
}

pub(crate) fn preview_directory_branch(state: RwSignal<State>, id: String) {
    let branch = state.with_untracked(|s| {
        if !s.session_active() || s.files.loading || super::pending(s) {
            return None;
        }
        s.files
            .directory_branches
            .iter()
            .find(|branch| branch.id() == id)
            .cloned()
    });
    let Some(branch) = branch else {
        return;
    };
    state.update(|s| {
        s.notices.leave();
        s.files.directory_preview = Some(crate::files::model::DirectoryPreview {
            source: crate::files::model::DirectoryPreviewSource::Branch(branch),
            path: s.files.directory.clone(),
        });
        s.files.cancel_listing();
        s.files.reset_page();
        s.files.clear_selection();
        s.files.entries = Default::default();
        s.files.loaded = false;
        s.files.writable = false;
    });
    load_entries(state);
}
fn begin_directory_snapshot(
    state: &mut State,
    path: String,
    branch_id: String,
    tree_id: String,
) -> bool {
    if !state.session_active() || state.files.loading || super::pending(state) {
        return false;
    }
    state.notices.leave();
    state.files.navigate(Navigation::Directory(path.clone()));
    state.files.directory_preview = Some(crate::files::model::DirectoryPreview {
        source: crate::files::model::DirectoryPreviewSource::Snapshot { branch_id, tree_id },
        path,
    });
    state.change_page(crate::ui::Page::Files);
    true
}
pub(crate) fn preview_directory_snapshot(
    state: RwSignal<State>,
    path: String,
    branch_id: String,
    tree_id: String,
) {
    if state.try_update(|state| begin_directory_snapshot(state, path, branch_id, tree_id))
        == Some(true)
    {
        load_entries(state);
    }
}
pub(crate) fn return_to_default_file(state: RwSignal<State>) {
    if !state.with_untracked(|s| s.session_active() && !super::pending(s)) {
        return;
    }
    state.update(|s| s.files.file_preview = None);
    load_file_history(state);
}

pub(crate) fn return_to_default(state: RwSignal<State>) {
    if !state.with_untracked(|s| s.session_active() && !super::pending(s)) {
        return;
    }
    state.update(|s| {
        s.files.directory_preview = None;
        s.files.file_preview = None;
        s.files.clear_selection();
        s.files.reset_page();
    });
    load_entries(state);
}

pub(crate) fn open_preview_origin(state: RwSignal<State>) {
    let root = state.with_untracked(|state| {
        if !state.session_active() || super::pending(state) {
            return None;
        }
        state
            .files
            .directory_preview
            .as_ref()
            .map(|preview| preview.path.clone())
    });
    let Some(root) = root else {
        return;
    };
    state.update(|state| {
        state.files.directory_preview = None;
        state.files.file_preview = None;
    });
    go_directory_details(state, root);
}

pub(crate) fn change_page(state: RwSignal<State>, next: bool) {
    let cursor = state.with_untracked(|s| {
        if s.files.loading || super::pending(s) {
            return None;
        }
        if next {
            s.files.next_cursor.clone()
        } else {
            s.files.previous_cursor.clone()
        }
    });
    let Some(cursor) = cursor else {
        return;
    };
    state.update(|s| {
        s.notices.leave();
        s.files.cursor = Some(cursor);
        s.files.next_cursor = None;
        s.files.previous_cursor = None;
        s.files.clear_selection();
        s.files.checked_entries.clear();
        s.files.entries = Default::default();
        s.files.loaded = false;
    });
    request_page(state, false);
}

fn page_endpoint(slot: &str, directory: &str, query: &str, cursor: Option<&str>) -> String {
    let mut path = format!(
        "{}/entries?path={}&query={}&limit={}",
        disk_path(slot),
        api::segment(directory),
        api::segment(query),
        super::super::page::PAGE_SIZE
    );
    if let Some(cursor) = cursor {
        path.push_str(&format!("&cursor={}", api::segment(cursor)));
    }
    path
}

fn request_page(state: RwSignal<State>, restarted: bool) {
    if !state.with_untracked(|s| s.session_active() && !s.files.slot.is_empty()) {
        return;
    }
    let cancellation = match api::ReadCancellation::new() {
        Ok(value) => value,
        Err(error) => {
            fail(state, error);
            return;
        }
    };
    let Some((epoch, slot, directory, query, cursor, selected, generation)) =
        state.try_update(|s| {
            s.files.cancel_listing();
            s.files.loading = true;
            s.files.listing_failed = false;
            s.files.listing_cancellation = Some(cancellation.clone());
            (
                s.session_epoch,
                s.files.slot.clone(),
                s.files.directory.clone(),
                s.files.query.clone(),
                s.files.cursor.clone(),
                s.files.selected.clone(),
                s.files.generation,
            )
        })
    else {
        return;
    };
    let preview = state.with_untracked(|s| s.files.directory_preview.clone());
    let directory_key = DirectoryKey::navigation(&directory);
    spawn_local(async move {
        let mut endpoint = page_endpoint(&slot, &directory, &query, cursor.as_deref());
        if let Some(preview) = &preview {
            endpoint.push_str(&format!(
                "&directory_branch_id={}&expected_tree_id={}",
                api::segment(preview.branch_id()),
                api::segment(preview.tree_id())
            ));
        }
        if let Some(selected) = &selected {
            endpoint.push_str(&format!(
                "&selected_path={}&selected_entry_id={}",
                api::segment(selected.path()),
                api::segment(selected.entry_id())
            ));
        }
        let result = api::cancellable_get(&endpoint, cancellation)
            .await
            .and_then(|value| {
                let page = Page::parse(
                    value,
                    &slot,
                    &directory_key,
                    super::super::page::PAGE_SIZE,
                    selected.as_ref(),
                )?;
                if page.preview() != preview.is_some() {
                    return Err(crate::files::page::DecodeError::Relation(
                        "listing preview differs from the requested view",
                    )
                    .into());
                }
                Ok(page)
            });
        if !accept_read_response(state, epoch, &result)
            || !current_listing(state, epoch, &slot, &directory, generation)
        {
            return;
        }
        if !restarted
            && matches!(&result, Err(error) if error.code == storage_contract::ErrorCode::ListingChanged)
        {
            state.update(|s| {
                s.files.reset_page();
                s.files.entries = Default::default();
                s.files.loaded = false;
                let preview_changed = s.files.directory_preview.take().is_some();
                if preview_changed { s.files.clear_selection(); }
                s.set_file_notice(if preview_changed { t("浏览分支已改变，已返回默认视图；请重新选择分支。", "The browsed branch changed. Returned to the default view; choose the branch again.") } else { t(
                    "目录已改变，已返回第一页。",
                    "The folder changed. Returned to the first page.",
                ) });
            });
            request_page(state, true);
            return;
        }
        let refreshed = result.is_ok();
        match result {
            Ok(page) => state.update(|s| apply_page(s, page, selected.as_ref())),
            Err(error) => {
                state.update(|s| {
                    s.files.listing_failed = true;
                    s.files.writable = false;
                });
                fail_in(state, epoch, ErrorContext::Read(endpoint), error);
            }
        }
        state.update(|s| {
            s.files.loading = false;
            s.files.listing_cancellation = None;
        });
        if refreshed
            && state.with_untracked(|s| {
                s.files
                    .selected
                    .as_ref()
                    .is_some_and(super::super::model::file_kind)
            })
        {
            load_file_history(state);
        }
    });
}

fn apply_page(state: &mut State, page: Page, requested: Option<&Entry>) {
    let page = page.into_parts();
    state.files.entries = page.entries.into();
    state.files.next_cursor = page.next;
    state.files.previous_cursor = page.previous;
    state.files.total_entries = page.total;
    state.files.directory_branches = page.branches;
    (
        state.files.repository_id,
        state.files.repository_path,
        state.files.namespace_head,
    ) = page
        .context
        .map(|context| {
            (
                context.repository_id().to_owned(),
                context.repository_path().to_owned(),
                context.base_head().to_owned(),
            )
        })
        .unwrap_or_default();
    state.files.default_branch_scope = page.default_branch_scope;
    state.files.directory_branches_supported = page.branches_supported;
    state.files.directory_file_count = page.directory_file_count;
    state.files.writable = page.writable;
    state.files.loaded = true;
    if requested.map(super::super::model::entry_key)
        == state
            .files
            .selected
            .as_ref()
            .map(super::super::model::entry_key)
    {
        state.files.reconcile_selection(page.selected);
        if requested.is_some()
            && state.page == crate::ui::Page::Details
            && state.files.selected.is_none()
        {
            state.change_page(crate::ui::Page::Files);
        }
    } else {
        state.files.prune_checked_entries();
    }
}

fn current_listing(
    state: RwSignal<State>,
    epoch: u64,
    slot: &str,
    directory: &str,
    generation: u64,
) -> bool {
    current_session(state, epoch)
        && state.with_untracked(|s| {
            s.files.slot == slot
                && s.files.directory == directory
                && s.files.generation == generation
        })
}
pub(crate) fn select_entry(state: RwSignal<State>, entry: Entry) {
    state.update(|s| {
        s.notices.leave();
        s.files.clear_selection();
        s.files.selected = Some(entry.clone());
        s.change_page(crate::ui::Page::Details);
    });
    if super::super::model::file_kind(&entry) {
        load_file_history(state);
    }
}
#[cfg(test)]
#[path = "../../../../../test/web/unit/files/actions/browse/tests.rs"]
mod tests;
