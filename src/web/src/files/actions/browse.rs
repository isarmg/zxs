//! Disk/folder navigation and generation-guarded metadata reads.
use super::disk_path;
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

/// Return true only when recovery invalidated this read and storage is readable.
/// Old-session completions cannot write data, release a newer read, or reload.
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
    // A current-session 401 revokes authentication, not just this disk snapshot.
    if state.files.disks_generation != generation
        && !matches!(&result, Err(error) if error.status == 401)
    {
        state.files.disks_loaded = false;
        // Storage may have become unavailable again while this read was pending.
        return Ok(state.management.storage.readable());
    }
    match result {
        Ok(disks) => {
            state.files.disks = disks;
            state.files.disks_loaded = true;
            Ok(false)
        }
        Err(error) => {
            // Keep cached rows, but retain the demand for a fresh snapshot.
            // Healthy management polling will retry this current-generation read.
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
            .and_then(|value| super::super::model::parse_disks(&value));
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
    state.update(|s| {
        s.files.navigate(Navigation::Disk(slot));
        s.page = crate::ui::Page::Files;
    });
    load_entries(state);
}
pub(crate) fn go_root(state: RwSignal<State>) {
    state.update(|s| {
        s.files.navigate(Navigation::Root);
        s.page = crate::ui::Page::Files;
    });
    load_disks(state);
}
pub(crate) fn go_directory(state: RwSignal<State>, directory: String) {
    state.update(|s| {
        s.files.navigate(Navigation::Directory(directory));
        s.page = crate::ui::Page::Files;
    });
    load_entries(state);
}
/// Refresh keeps the current cursor; stale snapshots explicitly restart at page one.
pub(crate) fn load_entries(state: RwSignal<State>) {
    request_page(state, false);
}

pub(crate) fn reload_entries(state: RwSignal<State>) {
    state.update(|s| s.files.reset_page());
    request_page(state, false);
}

pub(crate) fn search(state: RwSignal<State>, query: String) {
    state.update(|s| {
        s.files.cancel_listing();
        s.files.reset_page();
        s.files.query = query;
        s.files.entries = Default::default();
        s.files.loaded = false;
        s.files.clear_selection();
    });
    if !state.with_untracked(|s| s.session_active() && !s.files.slot.is_empty()) {
        return;
    }
    let (epoch, slot, directory, generation) = state.with_untracked(|s| {
        (
            s.session_epoch,
            s.files.slot.clone(),
            s.files.directory.clone(),
            s.files.generation,
        )
    });
    // Coalesce typing before doing directory work. Navigation cancels the timer
    // immediately, and the same generation fence covers an already queued tick.
    let delay = gloo_timers::callback::Timeout::new(250, move || {
        if current_listing(state, epoch, &slot, &directory, generation) {
            request_page(state, false);
        }
    });
    state.update(|s| {
        s.files.loading = true;
        s.files.search_delay = Some(std::sync::Arc::new(send_wrapper::SendWrapper::new(delay)));
    });
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
    let directory_key = DirectoryKey::navigation(&directory);
    spawn_local(async move {
        let mut endpoint = page_endpoint(&slot, &directory, &query, cursor.as_deref());
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
                Page::parse(
                    value,
                    &slot,
                    &directory_key,
                    super::super::page::PAGE_SIZE,
                    selected.as_ref(),
                )
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
                s.notice = t(
                    "目录已改变，已返回第一页。",
                    "The folder changed. Returned to the first page.",
                );
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
            super::history::refresh(state);
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
    (state.files.repository_id, state.files.repository_path) = page
        .context
        .map(|context| {
            (
                context.repository_id().to_owned(),
                context.repository_path().to_owned(),
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
        if state.page == crate::ui::Page::Details && state.files.selected.is_none() {
            state.page = crate::ui::Page::Files;
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
        s.files.clear_selection();
        s.files.selected = Some(entry.clone());
        s.page = crate::ui::Page::Details;
    });
    if super::super::model::file_kind(&entry) {
        super::history::load_first_page(state);
    }
}
#[cfg(test)]
#[path = "../../../../../test/web/unit/files/actions/browse/tests.rs"]
mod tests;
