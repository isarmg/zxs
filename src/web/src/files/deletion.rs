//! Deleted files remain discoverable outside the visible SMB namespace. Reads
//! belong to a session/generation; undo belongs to the global operation ledger.
mod view;
mod wire;
use super::actions;
use crate::{
    api,
    ui::{State, accept_read_response, current_session, fail},
};
use json_codec::json;
use std::sync::Arc;
pub(crate) use view::view;
use web_interface::{prelude::*, tasks::spawn_local};
use wire::{Page, Summary};

#[derive(Clone, Default)]
pub(crate) struct Model {
    open: bool,
    generation: u64,
    loading: bool,
    page: Option<Arc<Page>>,
    selected: Option<Summary>,
    node: Option<Arc<storage_contract::FileDeletionNode>>,
    frontier_page: usize,
}
impl Model {
    fn accepts(&self, generation: u64) -> bool {
        self.open && self.generation == generation
    }
    fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.loading = true;
        self.node = None;
        self.frontier_page = 0;
        self.generation
    }
    fn close(&mut self) {
        self.open = false;
        self.generation += 1;
        self.loading = false;
        self.page = None;
        self.selected = None;
        self.node = None;
    }
}

fn load(state: RwSignal<State>, next: bool) {
    let endpoint = state.with_untracked(|s| {
        if !s.session_active() || s.files.deletion.loading {
            return None;
        }
        if next {
            let page = s.files.deletion.page.as_ref()?;
            let after = page.next_after.as_ref()?;
            Some(format!(
                "/api/file-deletions?after={}&snapshot={}",
                api::segment(after.as_str()),
                api::segment(&page.snapshot)
            ))
        } else {
            Some("/api/file-deletions".into())
        }
    });
    let Some(endpoint) = endpoint else {
        return;
    };
    let epoch = state.with_untracked(|s| s.session_epoch);
    let generation = state
        .try_update(|s| {
            let m = &mut s.files.deletion;
            m.open = true;
            m.page = None;
            m.selected = None;
            m.begin()
        })
        .expect("invariant: application state outlives its file view");
    spawn_local(async move {
        let result = api::validated_request("GET", &endpoint, None, wire::page).await;
        if !accept_read_response(state, epoch, &result)
            || !state.with_untracked(|s| s.files.deletion.accepts(generation))
        {
            return;
        }
        state.update(|s| s.files.deletion.loading = false);
        match result {
            Ok(page) => state.update(|s| s.files.deletion.page = Some(Arc::new(page))),
            Err(error) => fail(state, error),
        }
    });
}

fn select(state: RwSignal<State>, summary: Summary) {
    if !state.with_untracked(|s| {
        s.session_active() && s.files.deletion.open && !s.files.deletion.loading
    }) {
        return;
    }
    let epoch = state.with_untracked(|s| s.session_epoch);
    let generation = state
        .try_update(|s| {
            s.files.deletion.selected = Some(summary.clone());
            s.files.deletion.begin()
        })
        .expect("invariant: application state outlives its file view");
    spawn_local(async move {
        let endpoint = format!(
            "/api/files/{}/deletion",
            api::segment(summary.file_id.as_str())
        );
        let result =
            api::validated_request("GET", &endpoint, None, |value| wire::node(value, &summary))
                .await;
        if !accept_read_response(state, epoch, &result)
            || !state.with_untracked(|s| s.files.deletion.accepts(generation))
        {
            return;
        }
        state.update(|s| s.files.deletion.loading = false);
        match result {
            Ok(node) => state.update(|s| s.files.deletion.node = node.map(Arc::new)),
            Err(error) => fail(state, error),
        }
    });
}

fn undo_allowed(state: &State) -> bool {
    state.session_active()
        && state.management.storage.writable()
        && !actions::pending(state)
        && state.files.deletion.open
        && !state.files.deletion.loading
        && state.files.deletion.node.is_some()
}
fn undo(state: RwSignal<State>) {
    let Some(node) = state.with_untracked(|s| {
        undo_allowed(s)
            .then(|| s.files.deletion.node.clone())
            .flatten()
    }) else {
        return;
    };
    let Some(id) = crate::ui::file_operation_id(state) else {
        return;
    };
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| s.files.mutation_pending = true);
    spawn_local(async move {
        let endpoint = format!("/api/files/{}/deletion/undo", api::segment(&node.file_id));
        let result = api::request(
            "POST",
            &endpoint,
            Some(json!({"deletion_node_id":node.node_id,"operation_id":id})),
        )
        .await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Ok(value) => {
                actions::record_receipt(state, &id, value);
            }
            Err(error) => actions::record_failure(state, epoch, &id, error),
        }
        state.update(|s| s.files.mutation_pending = false);
        // An unknown outcome stays in the global ledger even if this view closes.
        // Refresh is read-only; a new undo ID remains blocked until it settles.
        if state.with_untracked(|s| s.files.deletion.open) {
            load(state, false);
        }
    });
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/deletion/tests.rs"]
mod tests;
