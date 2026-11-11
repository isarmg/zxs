//! Detail-only irreversible admission with account-scoped original-request recovery.
use super::{
    actions,
    history::DecodeError,
    model::{FileOperation, SaveStatus},
};
use crate::{
    api,
    ui::{ErrorContext, State, current_session, fail_in},
};
use json_codec::{Value, json};
use std::collections::BTreeMap;
use storage_contract::{
    Principal,
    history_window::{HistoryFileId, HistoryRoot, HistoryVersionId},
    node_retirement::{self as wire, Cleanup, Preview, Selection},
};
use web_interface::{i18n::t, prelude::*, tasks::spawn_local};
mod action;
mod recovery;
mod view;
pub(crate) use recovery::view as recovery_view;
pub(crate) use view::view;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    epoch: u64,
    file: String,
    entry: String,
    path: String,
    slot: String,
    root: String,
}
impl Target {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active()
            || !state.files.file_history.ready()
            || state.files.history_loading
        {
            return None;
        }
        let entry = state.files.selected.as_ref()?;
        if entry.kind() != storage_contract::NamespaceEntryKind::File {
            return None;
        }
        Some(Self {
            epoch: state.session_epoch,
            file: entry.file_id().into(),
            entry: entry.entry_id().into(),
            path: entry.path().into(),
            slot: state.files.slot.clone(),
            root: state.files.file_history.root()?.into(),
        })
    }
}
#[derive(Clone)]
struct Attempt {
    target: Target,
    principal: Principal,
    selection: Selection,
    operation: String,
    in_flight: bool,
    uncertain: bool,
    restored: bool,
    resume_only: bool,
    retry_confirmed: bool,
    cleanup: Option<Cleanup>,
    separation: Option<wire::Separation>,
}
impl Attempt {
    fn owned_by(&self, state: &State) -> bool {
        state.session_active()
            && state.session_epoch == self.target.epoch
            && state.principal.as_ref() == Some(&self.principal)
    }
    fn retry_allowed(&self, state: &State) -> bool {
        self.owned_by(state)
            && state.management.storage.writable()
            && !state.files.mutation_pending
            && !self.in_flight
            && status(state, self) != SaveStatus::Rejected
            && (!self.resume_only || published(status(state, self)))
            && (!self.restored || published(status(state, self)) || self.retry_confirmed)
    }
    fn dismiss_allowed(&self, state: &State) -> bool {
        self.owned_by(state)
            && !self.in_flight
            && ((!self.resume_only && status(state, self) == SaveStatus::Rejected)
                || published(status(state, self))
                    && self
                        .cleanup
                        .as_ref()
                        .is_some_and(Cleanup::all_required_bindings_absent))
    }
}
#[derive(Clone, Default)]
pub(crate) struct Model {
    target: Option<Target>,
    selected: BTreeMap<String, String>,
    preview: Option<Preview>,
    confirmed: bool,
    generation: u64,
    loading: bool,
    attempt: Option<Attempt>,
    error: Option<api::Error>,
    recovery: recovery::Queue,
}
impl Model {
    fn retarget(&mut self, target: Option<Target>) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.selected.clear();
        self.preview = None;
        self.confirmed = false;
        self.generation += 1;
        self.loading = false;
        if self.attempt.is_none() {
            self.error = None;
        }
    }
    fn selection(&self) -> Option<Selection> {
        let target = self.target.as_ref()?;
        if self.selected.is_empty() {
            return None;
        }
        let versions = self
            .selected
            .keys()
            .map(|id| {
                HistoryVersionId::parse(id)
                    .expect("invariant: history ingress validated version IDs")
            })
            .collect();
        Some(
            Selection::new(
                HistoryFileId::parse(&target.file)
                    .expect("invariant: selected entry has a validated file ID"),
                HistoryRoot::parse(&target.root)
                    .expect("invariant: admitted history root is a digest"),
                versions,
            )
            .expect("invariant: bounded checked-version map is nonempty and unique"),
        )
    }
}
fn status(state: &State, attempt: &Attempt) -> SaveStatus {
    state
        .files
        .operations
        .iter()
        .find(|op| op.id() == attempt.operation)
        .map_or(SaveStatus::Unknown, FileOperation::status)
}
fn published(status: SaveStatus) -> bool {
    matches!(status, SaveStatus::Published | SaveStatus::Degraded)
}
pub(crate) fn in_flight(state: &State, id: &str) -> bool {
    state
        .files
        .node_retirement
        .attempt
        .as_ref()
        .is_some_and(|attempt| attempt.operation == id && attempt.in_flight)
}
fn matches_request(
    file: &HistoryFileId,
    root: HistoryRoot,
    versions: &[HistoryVersionId],
    receipt: &Value,
) -> bool {
    receipt["operation_kind"].as_str() == Some("file.nodes.retire")
        && receipt["result"]["retirement"]["request"]["origin_file_id"].as_str()
            == Some(file.as_str())
        && receipt["result"]["retirement"]["request"]["history_root"].as_str()
            == Some(root.to_string().as_str())
        && receipt["result"]["retirement"]["request"]["version_ids"] == json!(versions)
}
pub(crate) fn receipt_matches(state: &State, id: &str, receipt: &Value) -> bool {
    recovery::receipt_matches(state, id, receipt)
        && state
            .files
            .node_retirement
            .attempt
            .as_ref()
            .filter(|attempt| attempt.operation == id)
            .is_none_or(|attempt| {
                attempt.owned_by(state)
                    && matches_request(
                        attempt.selection.file_id(),
                        attempt.selection.history_root(),
                        attempt.selection.version_ids(),
                        receipt,
                    )
            })
}
fn malformed(message: &'static str) -> api::Error {
    DecodeError::Relation(message).into()
}
fn admission_ready(state: &State) -> bool {
    if !state
        .files
        .selected
        .as_ref()
        .is_some_and(super::page::Entry::retained_history)
    {
        return actions::history_write_allowed(state);
    }
    state.session_active()
        && state.management.storage.writable()
        && !actions::pending(state)
        && !state.files.history_loading
        && state.files.file_history.ready()
        && state.files.directory_preview.is_none()
        && state.files.file_preview.is_none()
        && state.files.file_history.identity().is_some_and(|identity| {
            identity.context.path.is_none() && identity.context.current_node_id.is_none()
        })
}
fn checkbox(state: RwSignal<State>, target: &Target, version: &str, label: &str, checked: bool) {
    state.update(|state| {
        if !admission_ready(state) || Target::capture(state).as_ref() != Some(target) {
            return;
        }
        if !state.files.node_retirement.recovery.ready_for_new(state) {
            return;
        }
        let model = &mut state.files.node_retirement;
        if model.loading || model.attempt.is_some() || model.target.as_ref() != Some(target) {
            return;
        }
        if checked {
            if model.selected.len() == storage_contract::shared_history::MAX_SHARED_HISTORY_NODES {
                return;
            }
            model.selected.insert(version.into(), label.into());
        } else {
            model.selected.remove(version);
        }
        model.preview = None;
        model.confirmed = false;
        model.error = None;
    });
}
