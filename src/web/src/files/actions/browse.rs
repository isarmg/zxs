//! Disk/folder navigation and generation-guarded metadata reads.
use super::disk_path;
use crate::{
    api,
    files::model::Disk,
    ui::{State, accept_read_response, current_session, fail, list, text},
};
use json_codec::Value;
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
            Some(Err(error)) => fail(state, error),
            Some(Ok(false)) | None => {}
        }
    });
}

pub(crate) fn open_disk(state: RwSignal<State>, slot: String) {
    state.update(|s| {
        s.files.cancel_listing();
        s.files.reset_page();
        s.page = crate::ui::Page::Files;
        s.files.slot = slot;
        s.files.writable = false;
        s.files.directory.clear();
        s.files.query.clear();
        s.files.directory_branches.clear();
        s.files.directory_file_count = 0;
        s.files.directory_branches_supported = false;
        s.files.repository_id.clear();
        s.files.repository_path.clear();
        s.files.default_branch_scope = false;
        s.files.clear_selection();
        s.files.checked_entries.clear();
        s.files.batch = None;
        s.files.entries = Default::default();
        s.files.loaded = false;
    });
    load_entries(state);
}
pub(crate) fn go_root(state: RwSignal<State>) {
    state.update(|s| {
        s.files.cancel_listing();
        s.files.reset_page();
        s.page = crate::ui::Page::Files;
        s.files.directory_branches.clear();
        s.files.directory_file_count = 0;
        s.files.directory_branches_supported = false;
        s.files.repository_id.clear();
        s.files.repository_path.clear();
        s.files.default_branch_scope = false;
        s.files.slot.clear();
        s.files.writable = false;
        s.files.loading = false;
        s.files.directory.clear();
        s.files.entries = Default::default();
        s.files.clear_selection();
        s.files.checked_entries.clear();
        s.files.batch = None;
    });
    load_disks(state);
}
pub(crate) fn go_directory(state: RwSignal<State>, directory: String) {
    state.update(|s| {
        s.files.cancel_listing();
        s.files.reset_page();
        s.page = crate::ui::Page::Files;
        s.files.directory = directory;
        s.files.writable = false;
        s.files.entries = Default::default();
        s.files.query.clear();
        s.files.directory_branches.clear();
        s.files.directory_file_count = 0;
        s.files.directory_branches_supported = false;
        s.files.repository_id.clear();
        s.files.repository_path.clear();
        s.files.default_branch_scope = false;
        s.files.clear_selection();
        s.files.checked_entries.clear();
        s.files.batch = None;
        s.files.loaded = false;
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
    spawn_local(async move {
        let mut endpoint = page_endpoint(&slot, &directory, &query, cursor.as_deref());
        if let Some(selected) = &selected {
            endpoint.push_str(&format!(
                "&selected_path={}&selected_entry_id={}",
                api::segment(&text(selected, &["path"])),
                api::segment(&text(selected, &["entry_id"]))
            ));
        }
        let result = api::cancellable_get(&endpoint, cancellation).await;
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
        let result = result.and_then(|value| {
            let page = super::super::page::Page::parse(&value)?;
            let selected_entry = super::super::page::selected(&value, selected.as_ref())?;
            Ok((value, page, selected_entry))
        });
        let refreshed = result.is_ok();
        match result {
            Ok((value, page, selected_entry)) => state.update(|s| {
                s.files.entries = list(&value, "entries").into();
                s.files.next_cursor = page.next;
                s.files.previous_cursor = page.previous;
                s.files.total_entries = page.total;
                s.files.directory_branches = list(&value, "directory_branches");
                s.files.repository_id = text(&value["context"], &["repository_id"]);
                s.files.repository_path = text(&value["context"], &["repository_path"]);
                s.files.default_branch_scope =
                    value["default_branch_scope"].as_bool().unwrap_or(false);
                s.files.directory_branches_supported = value["directory_branches_supported"]
                    .as_bool()
                    .unwrap_or(false);
                s.files.directory_file_count =
                    crate::format::parse_u64(&value["directory_file_count"]).unwrap_or(0);
                s.files.writable = value["writable"].as_bool().unwrap_or(false);
                s.files.loaded = true;
                if selected.as_ref().map(super::super::model::entry_key)
                    == s.files
                        .selected
                        .as_ref()
                        .map(super::super::model::entry_key)
                {
                    s.files.reconcile_selection(selected_entry);
                    if s.page == crate::ui::Page::Details && s.files.selected.is_none() {
                        s.page = crate::ui::Page::Files;
                    }
                }
                let keys = s
                    .files
                    .entries
                    .iter()
                    .map(super::super::model::entry_key)
                    .collect::<std::collections::BTreeSet<_>>();
                s.files.checked_entries.retain(|key| keys.contains(key));
            }),
            Err(error) => {
                state.update(|s| {
                    s.files.listing_failed = true;
                    s.files.writable = false;
                });
                fail(state, error);
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
pub(crate) fn select_entry(state: RwSignal<State>, entry: Value) {
    state.update(|s| {
        s.files.clear_selection();
        s.files.selected = Some(entry.clone());
        s.page = crate::ui::Page::Details;
    });
    if entry["kind"] == "file" {
        load_file_history(state);
    }
}
pub(crate) fn load_file_history(state: RwSignal<State>) {
    if !state.with_untracked(State::session_active) {
        return;
    }
    let epoch = state.with_untracked(|s| s.session_epoch);
    let (id, context) = state.with_untracked(|s| {
        s.files
            .selected
            .as_ref()
            .map(|entry| {
                (
                    text(entry, &["file_id"]),
                    crate::files::model::file_context(&s.files, entry),
                )
            })
            .unwrap_or_default()
    });
    if id.is_empty() {
        return;
    }
    let generation = state.with_untracked(|s| s.files.history_generation + 1);
    state.update(|s| {
        s.files.history_loading = true;
        s.files.history_generation = generation;
    });
    spawn_local(async move {
        let result = api::request(
            "GET",
            &format!(
                "/api/file-manager/files/{}/history{}",
                api::segment(&id),
                context
                    .as_ref()
                    .map(|(repository, path)| format!(
                        "?repository_id={}&path={}",
                        api::segment(repository),
                        api::segment(path)
                    ))
                    .unwrap_or_default()
            ),
            None,
        )
        .await;
        if !accept_read_response(state, epoch, &result)
            || !state.with_untracked(|s| {
                s.files.history_generation == generation
                    && s.files
                        .selected
                        .as_ref()
                        .is_some_and(|entry| text(entry, &["file_id"]) == id)
            })
        {
            return;
        }
        match result {
            Ok(value) => state.update(|s| {
                s.files.file_history = match s.files.selected.as_ref() {
                    Some(entry) => crate::files::model::history_for_entry(value, entry),
                    None => value,
                }
                .into();
            }),
            Err(error) => {
                // A failed refresh must not re-enable controls for stale heads.
                state.update(|s| s.files.file_history = Value::Null.into());
                fail(state, error);
            }
        }
        state.update(|s| s.files.history_loading = false);
    });
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/actions/browse/tests.rs"]
mod tests;
