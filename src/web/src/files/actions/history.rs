//! Bounded file-history reads (ADR 0009). Each slot has at most one in-flight
//! request; a new one aborts the previous. Late responses are accepted only
//! when session epoch, selected file, context, history generation and the
//! slot ticket all still match.
use crate::{
    api,
    files::{
        history::{
            BranchPage, FieldSlice, HistoryBinding, HistoryWindow, MergeInputsPage, MergePage,
            NodeField, ParentsPage,
            state::{AuxPanel, BranchReadState, ReadSlot, StaleReason},
        },
        model::file_context,
    },
    ui::{ErrorContext, State, accept_read_response, fail, fail_in},
};
use web_interface::tasks::spawn_local;
use web_interface::{i18n::t, prelude::*};

/// Node page size requested by the browser; the server may return fewer
/// records when its byte budget is reached.
pub(crate) const NODE_PAGE_LIMIT: usize = 50;
const BRANCH_PAGE_LIMIT: usize = 100;
const MERGE_PAGE_LIMIT: usize = 50;
const PARENT_PAGE_LIMIT: usize = 100;
const INPUT_PAGE_LIMIT: usize = 200;
const FIELD_MAX_BYTES: usize = 64 * 1024;

/// Where the main page starts: first page, a continuation, or anchored so
/// that the given node is the first node of the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MainRequest {
    First,
    /// Discover a new published root, rather than navigate the observed one.
    Latest,
    Cursor(String),
    Anchor(String),
}

/// Fields fixed at request time; a late response is accepted only if the
/// model still describes exactly this read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fence {
    epoch: u64,
    file_id: String,
    context: Option<(String, String)>,
    generation: u64,
    slot: ReadSlot,
    ticket: u64,
}
impl Fence {
    fn capture(state: &State, slot: ReadSlot) -> Option<Self> {
        let entry = state.files.selected.as_ref()?;
        if entry.file_id().is_empty() {
            return None;
        }
        Some(Self {
            epoch: state.session_epoch,
            file_id: entry.file_id().to_owned(),
            context: file_context(entry),
            generation: state.files.history.generation,
            slot,
            ticket: state.files.history.reads.ticket(slot),
        })
    }
    pub(crate) fn holds(&self, state: &State) -> bool {
        let history = &state.files.history;
        history.generation == self.generation
            && history.reads.ticket(self.slot) == self.ticket
            // A main response replaces exactly the Loading state it started.
            && (self.slot != ReadSlot::Main
                || history.state.loading_generation() == Some(self.generation))
            && state.files.selected.as_ref().is_some_and(|entry| {
                entry.file_id() == self.file_id && file_context(entry) == self.context
            })
    }
}

fn history_path(file: &str) -> String {
    format!("/api/file-manager/files/{}/history", api::segment(file))
}

pub(crate) fn main_endpoint(
    file: &str,
    context: Option<&(String, String)>,
    request: &MainRequest,
    root: Option<&str>,
) -> String {
    let mut endpoint = history_path(file);
    let mut first = true;
    let mut push = |endpoint: &mut String, key: &str, value: &str| {
        endpoint.push(if first { '?' } else { '&' });
        first = false;
        endpoint.push_str(key);
        endpoint.push('=');
        endpoint.push_str(&api::segment(value));
    };
    if let Some((repository, path)) = context {
        push(&mut endpoint, "repository_id", repository);
        push(&mut endpoint, "path", path);
    }
    push(&mut endpoint, "limit", &NODE_PAGE_LIMIT.to_string());
    match request {
        MainRequest::First | MainRequest::Latest => {}
        MainRequest::Cursor(cursor) => push(&mut endpoint, "cursor", cursor),
        MainRequest::Anchor(node) => push(&mut endpoint, "anchor_node_id", node),
    }
    if let Some(root) = root {
        push(&mut endpoint, "history_root", root);
    }
    endpoint
}

/// `HISTORY_CHANGED`: the resident page stays readable, every write entry
/// that depended on it closes, drafts stay as input awaiting re-review.
pub(crate) fn mark_stale(state: &mut State, reason: StaleReason) {
    state.files.history.mark_stale(reason);
    state.files.open_editors = 0;
    state.notice = t(
        "历史已变化，请刷新。",
        "The history changed. Refresh to continue.",
    );
}

fn new_cancellation(state: RwSignal<State>, slot: ReadSlot) -> Option<api::ReadCancellation> {
    match api::ReadCancellation::new() {
        Ok(value) => Some(value),
        Err(error) => {
            if slot == ReadSlot::Branches {
                state.update(|s| {
                    s.files.history.branch_read = BranchReadState::Failed(error.clone())
                });
            }
            fail(state, error);
            None
        }
    }
}

/// Synchronous part of a main read. Returns the fence and endpoint when a
/// request was admitted; the caller performs the network work.
pub(crate) fn begin_main(
    state: RwSignal<State>,
    request: &MainRequest,
    cancellation: Option<api::ReadCancellation>,
) -> Option<(Fence, String)> {
    if !state.with_untracked(State::session_active) {
        return None;
    }
    if !navigation_allowed(state) {
        return None;
    }
    state.try_update(|s| begin_main_request(s, request, cancellation))?
}

/// Every explicit page change, including First, preserves an active editor.
pub(crate) fn navigation_allowed(state: RwSignal<State>) -> bool {
    if state.with_untracked(|s| s.files.open_editors > 0) {
        state.update(|s| {
            s.notice = t(
                "请先完成或取消当前编辑，再切换历史页。",
                "Finish or cancel the current edit before changing history pages.",
            );
        });
        return false;
    }
    true
}

/// A navigation click must reach the edit guard before the input's ordinary
/// blur-to-cancel behavior runs. Keep focus on the editor for that click.
pub(crate) fn preserve_editor_focus(state: RwSignal<State>, event: &web_sys::MouseEvent) {
    if state.with_untracked(|s| s.files.open_editors > 0) {
        event.prevent_default();
    }
}

fn begin_main_request(
    s: &mut State,
    request: &MainRequest,
    cancellation: Option<api::ReadCancellation>,
) -> Option<(Fence, String)> {
    if s.files
        .selected
        .as_ref()
        .is_none_or(|entry| entry.file_id().is_empty())
    {
        return None;
    }
    let root = match request {
        MainRequest::First => s.files.history.root().map(|root| root.as_str().to_owned()),
        MainRequest::Latest => None,
        MainRequest::Cursor(_) | MainRequest::Anchor(_) => {
            Some(s.files.history.root()?.as_str().to_owned())
        }
    };
    s.files.history.begin_main(cancellation);
    let fence = Fence::capture(s, ReadSlot::Main)?;
    let endpoint = main_endpoint(
        &fence.file_id,
        fence.context.as_ref(),
        request,
        root.as_deref(),
    );
    Some((fence, endpoint))
}

fn request_main(state: RwSignal<State>, request: &MainRequest) {
    // Session and selection gates run before any browser resource is created.
    if !state.with_untracked(|s| {
        s.session_active()
            && s.files
                .selected
                .as_ref()
                .is_some_and(|entry| !entry.file_id().is_empty())
    }) {
        return;
    }
    let Some(cancellation) = new_cancellation(state, ReadSlot::Main) else {
        return;
    };
    let Some((fence, endpoint)) = begin_main(state, request, Some(cancellation.clone())) else {
        return;
    };
    spawn_local(async move {
        let result = api::cancellable_get(&endpoint, cancellation)
            .await
            .and_then(|value| HistoryWindow::parse(value, &fence.file_id));
        complete_main(state, &fence, &endpoint, result);
    });
}

/// Install or reject a main-page response. Separated from the spawn so the
/// fence logic is testable without a browser.
pub(crate) fn complete_main(
    state: RwSignal<State>,
    fence: &Fence,
    endpoint: &str,
    result: Result<HistoryWindow, api::Error>,
) {
    if !accept_read_response(state, fence.epoch, &result)
        || !state.with_untracked(|s| fence.holds(s))
    {
        return;
    }
    match result {
        Ok(window) => state.update(|s| {
            let entry = s
                .files
                .selected
                .as_ref()
                .expect("invariant: accepted history still has its selected entry");
            let window = window.for_entry(entry);
            s.files.history.install(window);
        }),
        Err(error) if error.code == storage_contract::ErrorCode::HistoryChanged => {
            // The continuation lost its root; the previous page stays
            // readable as stale until the user refreshes on the new root.
            state.update(|s| mark_stale(s, StaleReason::RootChanged));
        }
        Err(error) => {
            // A failed refresh must not re-enable controls for stale heads.
            state.update(|s| s.files.history.fail(error.clone()));
            fail_in(
                state,
                fence.epoch,
                ErrorContext::Read(endpoint.to_owned()),
                error,
            );
        }
    }
}

/// First page of the selected file, replacing whatever is resident.
pub(crate) fn load_first_page(state: RwSignal<State>) {
    request_main(state, &MainRequest::First);
}
/// Continue at a cursor from the resident window.
pub(crate) fn history_page(state: RwSignal<State>, cursor: String) {
    request_main(state, &MainRequest::Cursor(cursor));
}
/// Replace the window so that `node_id` is the first node of the page.
pub(crate) fn history_anchor(state: RwSignal<State>, node_id: String) {
    request_main(state, &MainRequest::Anchor(node_id));
}
/// A refresh discovers the latest root. It is not navigation within the
/// resident root, even when a metadata-only write kept the selection intact.
pub(crate) fn refresh(state: RwSignal<State>) {
    request_main(state, &MainRequest::Latest);
}

pub(crate) struct Aux {
    pub(crate) fence: Fence,
    pub(crate) binding: HistoryBinding,
    pub(crate) endpoint: String,
}

/// Synchronous part of an auxiliary read bound to the resident root. Without
/// a current window there is nothing to bind to and no request is made.
pub(crate) fn begin_aux(
    state: RwSignal<State>,
    slot: ReadSlot,
    cancellation: Option<api::ReadCancellation>,
    endpoint: impl FnOnce(&HistoryBinding) -> String,
) -> Option<Aux> {
    if !state.with_untracked(State::session_active) {
        return None;
    }
    state.try_update(|s| {
        let binding = s.files.history.loaded()?.binding.clone();
        let mut endpoint = endpoint(&binding);
        endpoint.push_str(if endpoint.contains('?') { "&" } else { "?" });
        endpoint.push_str("history_root=");
        endpoint.push_str(binding.root.as_str());
        s.files.history.reads.begin_with(slot, cancellation);
        if slot == ReadSlot::Branches {
            s.files.history.branch_read = BranchReadState::Loading;
        }
        let fence = Fence::capture(s, slot)?;
        Some(Aux {
            fence,
            binding,
            endpoint,
        })
    })?
}

/// Install or reject an auxiliary response under the same fences.
pub(crate) fn complete_aux<T>(
    state: RwSignal<State>,
    aux: &Aux,
    result: Result<T, api::Error>,
    install: impl FnOnce(&mut State, T),
) {
    if !accept_read_response(state, aux.fence.epoch, &result)
        || !state.with_untracked(|s| aux.fence.holds(s))
    {
        return;
    }
    state.update(|s| s.files.history.reads.finish(aux.fence.slot));
    match result {
        Ok(value) => state.update(|s| {
            install(s, value);
            if aux.fence.slot == ReadSlot::Branches {
                s.files.history.branch_read = BranchReadState::Loaded;
            }
        }),
        Err(error) if error.code == storage_contract::ErrorCode::HistoryChanged => {
            state.update(|s| mark_stale(s, StaleReason::RootChanged));
        }
        Err(error) => {
            if aux.fence.slot == ReadSlot::Branches {
                state.update(|s| {
                    s.files.history.branch_read = BranchReadState::Failed(error.clone())
                });
            }
            fail_in(
                state,
                aux.fence.epoch,
                ErrorContext::Read(aux.endpoint.clone()),
                error,
            );
        }
    }
}

fn request_aux<T: 'static>(
    state: RwSignal<State>,
    slot: ReadSlot,
    endpoint: impl FnOnce(&HistoryBinding) -> String,
    parse: impl FnOnce(json_codec::Value, &HistoryBinding) -> Result<T, api::Error> + 'static,
    install: impl FnOnce(&mut State, T) + 'static,
) {
    if !state.with_untracked(|s| s.session_active() && s.files.history.loaded().is_some()) {
        return;
    }
    let Some(cancellation) = new_cancellation(state, slot) else {
        return;
    };
    let Some(aux) = begin_aux(state, slot, Some(cancellation.clone()), endpoint) else {
        return;
    };
    spawn_local(async move {
        let result = api::cancellable_get(&aux.endpoint, cancellation)
            .await
            .and_then(|value| parse(value, &aux.binding));
        complete_aux(state, &aux, result, install);
    });
}

fn with_cursor(mut endpoint: String, cursor: Option<&str>) -> String {
    if let Some(cursor) = cursor {
        endpoint.push_str(&format!("&cursor={}", api::segment(cursor)));
    }
    endpoint
}

/// One branch page for the selector, bound to the resident root.
pub(crate) fn load_history_branches(state: RwSignal<State>, cursor: Option<String>) {
    if !state.with_untracked(|s| s.session_active() && s.files.history.loaded().is_some())
        || !navigation_allowed(state)
    {
        return;
    }
    state.update(|s| s.files.history.branch_cursor = cursor.clone());
    request_aux(
        state,
        ReadSlot::Branches,
        |binding| {
            with_cursor(
                format!(
                    "{}/branches?limit={BRANCH_PAGE_LIMIT}",
                    history_path(&binding.file_id)
                ),
                cursor.as_deref(),
            )
        },
        BranchPage::parse,
        |s, page| s.files.history.branches = Some(page),
    );
}

/// One provenance page, replacing the previous one.
pub(crate) fn load_history_merges(state: RwSignal<State>, cursor: Option<String>) {
    request_aux(
        state,
        ReadSlot::Merges,
        |binding| {
            with_cursor(
                format!(
                    "{}/merges?limit={MERGE_PAGE_LIMIT}",
                    history_path(&binding.file_id)
                ),
                cursor.as_deref(),
            )
        },
        MergePage::parse,
        |s, page| s.files.history.merges = Some(page),
    );
}

/// Open (or page) the complete-parents panel of one node.
pub(crate) fn open_parents_panel(state: RwSignal<State>, node_id: String, cursor: Option<String>) {
    let node = node_id.clone();
    let installed = node_id.clone();
    request_aux(
        state,
        ReadSlot::Panel,
        move |binding| {
            with_cursor(
                format!(
                    "{}/nodes/{}/parents?limit={PARENT_PAGE_LIMIT}",
                    history_path(&binding.file_id),
                    api::segment(&node)
                ),
                cursor.as_deref(),
            )
        },
        move |value, binding| ParentsPage::parse(value, binding, &node_id),
        move |s, page| {
            s.files.history.panel = Some(AuxPanel::Parents {
                node_id: installed,
                page,
            });
        },
    );
}

/// Open (or page) the merge-input panel of one provenance record.
pub(crate) fn open_merge_sources_panel(state: RwSignal<State>, merge_id: String, offset: usize) {
    let merge = merge_id.clone();
    let installed = merge_id.clone();
    request_aux(
        state,
        ReadSlot::Panel,
        move |binding| {
            format!(
                "{}/merges/{}/inputs?offset={offset}&limit={INPUT_PAGE_LIMIT}",
                history_path(&binding.file_id),
                api::segment(&merge)
            )
        },
        move |value, binding| MergeInputsPage::parse(value, binding, &merge_id),
        move |s, page| {
            s.files.history.panel = Some(AuxPanel::MergeSources {
                merge_id: installed,
                page,
            });
        },
    );
}

/// Read one fragment of an externalized node text into the panel.
pub(crate) fn load_node_field(
    state: RwSignal<State>,
    node_id: String,
    field: NodeField,
    offset: usize,
) {
    let node = node_id.clone();
    let installed = node_id.clone();
    request_aux(
        state,
        ReadSlot::Panel,
        move |binding| {
            format!(
                "{}/nodes/{}/fields/{}?offset={offset}&max_bytes={FIELD_MAX_BYTES}",
                history_path(&binding.file_id),
                api::segment(&node),
                field.as_str()
            )
        },
        move |value, binding| FieldSlice::parse(value, binding, &node_id, field),
        move |s, slice| {
            s.files.history.panel = Some(AuxPanel::Field {
                node_id: installed,
                field,
                slice,
            });
        },
    );
}

/// Close the auxiliary panel and abort its read; its page is freed.
pub(crate) fn close_panel(state: RwSignal<State>) {
    state.update(|s| {
        s.files.history.reads.abort(ReadSlot::Panel);
        s.files.history.panel = None;
    });
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/actions/history/tests.rs"]
mod tests;
