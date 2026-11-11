//! Deleted files remain discoverable outside the visible SMB namespace. Reads
//! belong to a session/generation; undo belongs to the global operation ledger.
mod batch;
mod batch_restore;
mod directory;
mod frontier;
mod listing;
mod recovery;
mod request;
mod restore;
mod summary;
mod view;
mod wire;
use crate::files::actions;
use crate::{
    api,
    ui::{State, accept_read_response, current_session, fail},
};
pub(crate) use directory::{
    available as directory_available, entry_available as directory_entry_available,
    start as start_directory,
};
pub(crate) use recovery::{
    blocks_writes, finish as finish_recovery, restore as restore_recovery, view as recovery_view,
};
pub(crate) use request::{available, in_flight, receipt_matches, settle, start};
use std::sync::Arc;
pub(crate) use view::view;
use web_interface::{prelude::*, tasks::spawn_local};
use wire::{Page, Summary};

#[derive(Clone)]
enum Inspection {
    Complete(Arc<storage_contract::FileDeletionNode>),
    Paged(summary::Model),
}
impl Inspection {
    fn complete(&self) -> Option<&Arc<storage_contract::FileDeletionNode>> {
        match self {
            Self::Complete(node) => Some(node),
            Self::Paged(_) => None,
        }
    }
    fn paged(&self) -> Option<&summary::Model> {
        match self {
            Self::Complete(_) => None,
            Self::Paged(model) => Some(model),
        }
    }
    fn paged_mut(&mut self) -> Option<&mut summary::Model> {
        match self {
            Self::Complete(_) => None,
            Self::Paged(model) => Some(model),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Model {
    directory: directory::Model,
    review_generation: u64,
    review: Option<request::Review>,
    batch_review: Option<batch::Review>,
    restore_review: Option<restore::Review>,
    attempt: Option<request::Attempt>,
    recovery: recovery::Queue,
    open: bool,
    generation: u64,
    loading: bool,
    page: Option<Arc<Page>>,
    selected: Option<Summary>,
    node: Option<Inspection>,
    frontier_page: usize,
}
impl Model {
    pub(crate) fn cancel_review(&mut self) {
        self.review_generation += 1;
        self.review = None;
        self.batch_review = None;
        self.restore_review = None;
        self.directory.cancel_review();
    }

    fn accepts(&self, generation: u64) -> bool {
        self.open && self.generation == generation
    }
    fn begin(&mut self) -> u64 {
        self.cancel_review();
        self.generation += 1;
        self.loading = true;
        self.node = None;
        self.frontier_page = 0;
        self.generation
    }
    fn close(&mut self) {
        self.cancel_review();
        self.directory.close();
        self.open = false;
        self.generation += 1;
        self.loading = false;
        self.page = None;
        self.selected = None;
        self.node = None;
    }
}

fn load(state: RwSignal<State>, read: listing::Read) {
    let endpoint = state.with_untracked(|s| read.endpoint(s));
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

fn select(state: RwSignal<State>, selection: listing::Selection) {
    let Some((epoch, generation, summary)) = state
        .try_update(|s| selection.begin(s))
        .expect("invariant: application state outlives its file view")
    else {
        return;
    };
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
            Ok(node) => state.update(|s| {
                s.files.deletion.node = node.map(|node| Inspection::Complete(Arc::new(node)));
            }),
            Err(error) if summary::needs_pages(error.code) => {
                summary::load(state, summary, epoch, generation);
            }
            Err(error) => fail(state, error),
        }
    });
}

pub(crate) fn outcome_endpoint(state: &State, id: &str) -> Option<String> {
    let compact = recovery::compact_outcome(state, id)
        || state
            .files
            .deletion
            .attempt
            .as_ref()
            .is_some_and(|attempt| {
                attempt.epoch == state.session_epoch
                    && attempt.principal == state.principal
                    && attempt.operation == id
                    && matches!(&attempt.intent, request::Intent::UndoSummary(_))
            });
    compact.then(|| format!("/api/file-deletion-undos/{}", api::segment(id)))
}

fn undo_allowed(state: &State) -> bool {
    state.session_active()
        && state.management.storage.writable()
        && !actions::pending(state)
        && state.files.deletion.open
        && !state.files.deletion.loading
        && state
            .files
            .deletion
            .node
            .as_ref()
            .and_then(Inspection::complete)
            .is_some()
}
fn undo(state: RwSignal<State>, file_id: &str, deletion_node_id: &str) {
    let Some(node) = state.with_untracked(|s| {
        undo_allowed(s)
            .then(|| {
                s.files
                    .deletion
                    .node
                    .as_ref()
                    .and_then(Inspection::complete)
                    .cloned()
            })
            .flatten()
            .filter(|node| node.file_id == file_id && node.node_id == deletion_node_id)
    }) else {
        return;
    };
    request::submit_undo(state, node);
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/deletion/tests.rs"]
mod tests;
