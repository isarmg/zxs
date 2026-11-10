//! Shared publication state for inline review panels. Unknown outcomes remain
//! tied to their original operation, and definitive conflicts preserve drafts.
use super::{
    actions,
    model::{FileOperation, Model, SaveStatus, publication_notice},
};
use crate::{
    api,
    ui::{State, current_session, fail},
};
use json_codec::Value;
use std::{collections::BTreeMap, sync::Arc};
mod recovery;
pub(crate) use recovery::view as recovery_view;
use web_interface::prelude::*;
use web_interface::tasks::spawn_local;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    Merge,
    Tags,
    Backup,
    Placement,
}
#[derive(Clone, Default)]
pub(crate) struct Publication {
    pub operation_id: Option<String>,
    pub error: String,
}
pub(crate) type Publications = BTreeMap<Kind, Publication>;

pub(crate) fn error(state: RwSignal<State>, kind: Kind, message: String) {
    state.update(|s| s.files.publications.entry(kind).or_default().error = message);
}
pub(crate) fn reset(state: RwSignal<State>, kind: Kind) {
    state.update(|s| {
        s.files.publications.remove(&kind);
    });
}
pub(crate) fn error_text(state: &State, kind: Kind) -> String {
    state
        .files
        .publications
        .get(&kind)
        .map(|publication| publication.error.clone())
        .unwrap_or_default()
}
pub(crate) fn settle(model: &mut Model, id: &str, status: SaveStatus) {
    let kinds = model
        .publications
        .iter()
        .filter(|(_, publication)| publication.operation_id.as_deref() == Some(id))
        .map(|(kind, _)| *kind)
        .collect::<Vec<_>>();
    for kind in kinds {
        match status {
            SaveStatus::Published | SaveStatus::Degraded => {
                match kind {
                    Kind::Merge => model.merge = None,
                    Kind::Backup => {
                        model.backup = None;
                        model.recursive_backup = None;
                    }
                    Kind::Placement => model.placement = None,
                    Kind::Tags => {
                        let owns_draft = model.tag_submission.as_ref().is_none_or(|submission| {
                            model.tag_edit.as_ref().is_some_and(|draft| {
                                submission
                                    .request
                                    .owner
                                    .as_ref()
                                    .is_some_and(|owner| draft.owned_by(owner))
                            })
                        });
                        if owns_draft {
                            model.tag_edit = None;
                            model.checked_nodes.clear();
                        }
                    }
                }
                model.publications.remove(&kind);
                if kind == Kind::Tags {
                    model.tag_submission = None;
                }
            }
            SaveStatus::Rejected => {
                if let Some(publication) = model.publications.get_mut(&kind) {
                    publication.operation_id = None;
                    publication.error = publication_notice(status);
                }
                if kind == Kind::Tags {
                    model.tag_submission = None;
                }
            }
            SaveStatus::Unknown => {}
        }
    }
}
/// Immutable data from the first send. Credentials are obtained by the API
/// for each attempt; neither tokens nor CSRF material are retained here.
struct FrozenRequest {
    id: String,
    method: &'static str,
    endpoint: String,
    body: Value,
    epoch: u64,
    owner: Option<super::tags::TagOwner>,
}
#[derive(Clone)]
pub(crate) struct TagSubmission {
    request: Arc<FrozenRequest>,
    generation: u64,
    in_flight: bool,
    uncertain: bool,
}
struct Attempt {
    request: Arc<FrozenRequest>,
    generation: u64,
    was_uncertain: bool,
}

fn can_start(state: &State, kind: Kind) -> bool {
    actions::write_allowed(state)
        && (matches!(kind, Kind::Backup | Kind::Placement)
            || !super::model::selected_entry_read_only(&state.files))
        && !state
            .files
            .publications
            .get(&kind)
            .is_some_and(|p| p.operation_id.is_some())
        && (kind != Kind::Tags
            || state
                .files
                .tag_edit
                .as_ref()
                .is_some_and(|draft| !draft.requires_review()))
}
fn begin_submission(
    state: &mut State,
    kind: Kind,
    id: String,
    method: &'static str,
    endpoint: String,
    mut body: Value,
) -> Attempt {
    body["operation_id"] = id.clone().into();
    let owner = (kind == Kind::Tags).then(|| {
        state
            .files
            .tag_edit
            .as_ref()
            .expect("invariant: admitted tag submission has its draft")
            .owner()
    });
    let request = Arc::new(FrozenRequest {
        id: id.clone(),
        method,
        endpoint,
        body,
        epoch: state.session_epoch,
        owner,
    });
    let publication = state.files.publications.entry(kind).or_default();
    publication.operation_id = Some(id);
    publication.error.clear();
    state.files.mutation_pending = true;
    if kind == Kind::Tags {
        state.files.tag_submission = Some(TagSubmission {
            request: Arc::clone(&request),
            generation: 0,
            in_flight: true,
            uncertain: false,
        });
    }
    Attempt {
        request,
        generation: 0,
        was_uncertain: false,
    }
}
pub(crate) fn has_tag_replay(state: &State) -> bool {
    state
        .files
        .tag_submission
        .as_ref()
        .is_some_and(|submission| {
            submission.uncertain
                && submission.request.epoch == state.session_epoch
                && state
                    .files
                    .publications
                    .get(&Kind::Tags)
                    .and_then(|p| p.operation_id.as_deref())
                    == Some(submission.request.id.as_str())
        })
}
pub(crate) fn receipt_kind_matches(state: &State, id: &str, value: &Value) -> bool {
    !state
        .files
        .tag_submission
        .as_ref()
        .is_some_and(|submission| submission.request.id == id)
        || value["operation_kind"].as_str() == Some("file.tags.update")
}
pub(crate) fn tag_attempt_in_flight(state: &State, id: &str) -> bool {
    state
        .files
        .tag_submission
        .as_ref()
        .is_some_and(|s| s.request.id == id && s.in_flight)
}
pub(crate) fn can_retry_tags(state: &State) -> bool {
    has_tag_replay(state)
        && state.session_active()
        && state.management.storage.writable()
        && !state.files.mutation_pending
        && state
            .files
            .tag_submission
            .as_ref()
            .is_some_and(|submission| {
                !submission.in_flight
                    && !state
                        .files
                        .polling_operations
                        .iter()
                        .any(|id| id == &submission.request.id)
                    && state.files.operations.iter().any(|operation| {
                        operation.id() == submission.request.id
                            && operation.status() == SaveStatus::Unknown
                    })
            })
}
fn begin_retry(state: &mut State) -> Option<Attempt> {
    if !can_retry_tags(state) {
        return None;
    }
    let submission = state
        .files
        .tag_submission
        .as_mut()
        .expect("invariant: retry admission found the frozen request");
    submission.generation = submission
        .generation
        .checked_add(1)
        .expect("invariant: a browser session cannot send 2^64 tag attempts");
    submission.in_flight = true;
    state.files.mutation_pending = true;
    Some(Attempt {
        request: Arc::clone(&submission.request),
        generation: submission.generation,
        was_uncertain: true,
    })
}
fn current_attempt(state: &State, kind: Kind, attempt: &Attempt) -> bool {
    state.session_active()
        && state.session_epoch == attempt.request.epoch
        && state
            .files
            .publications
            .get(&kind)
            .and_then(|p| p.operation_id.as_deref())
            == Some(attempt.request.id.as_str())
        && (kind != Kind::Tags
            || state
                .files
                .tag_submission
                .as_ref()
                .is_some_and(|submission| {
                    Arc::ptr_eq(&submission.request, &attempt.request)
                        && submission.generation == attempt.generation
                        && submission.in_flight
                }))
}
fn complete_error(state: &mut State, kind: Kind, attempt: &Attempt, error: &api::Error) -> bool {
    let unknown = attempt.was_uncertain || error.mutation_outcome_unknown();
    state.files.mutation_pending = false;
    if let Some(publication) = state.files.publications.get_mut(&kind) {
        publication.error = error.to_string();
        if !unknown {
            publication.operation_id = None;
        }
    }
    if kind == Kind::Tags {
        if unknown {
            let submission = state
                .files
                .tag_submission
                .as_mut()
                .expect("invariant: current tag attempt retains its frozen request");
            submission.in_flight = false;
            submission.uncertain = true;
        } else {
            state.files.tag_submission = None;
        }
        if !unknown
            && error.status == 409
            && let Some(owner) = &attempt.request.owner
            && owner.current(&state.files)
        {
            let draft = state
                .files
                .tag_edit
                .as_mut()
                .expect("invariant: matching tag owner has its draft");
            let instance = draft.instance();
            draft.require_review();
            state
                .files
                .history_aux
                .cancel_review(super::history::reads::ReviewOwner {
                    kind: Kind::Tags,
                    instance,
                });
            return true;
        }
    }
    false
}
/// Return whether the still-current tag scope needs a fresh history window.
fn complete(
    state: RwSignal<State>,
    kind: Kind,
    attempt: &Attempt,
    result: Result<Value, api::Error>,
) -> bool {
    if !current_session(state, attempt.request.epoch)
        || !state.with_untracked(|s| current_attempt(s, kind, attempt))
    {
        return false;
    }
    match result {
        Ok(value) => {
            state.update(|s| {
                s.files.mutation_pending = false;
                if kind == Kind::Tags {
                    s.files
                        .tag_submission
                        .as_mut()
                        .expect("invariant: current tag response has its submission")
                        .in_flight = false;
                }
            });
            let status = actions::record_receipt(state, &attempt.request.id, value);
            if kind == Kind::Tags && status == SaveStatus::Unknown {
                state.update(|s| {
                    s.files
                        .tag_submission
                        .as_mut()
                        .expect("invariant: nonterminal receipt retains its request")
                        .uncertain = true
                });
            }
            false
        }
        Err(error) => {
            if attempt.was_uncertain || error.mutation_outcome_unknown() {
                actions::record_operation(state, FileOperation::unknown(&attempt.request.id));
            }
            let refresh = state
                .try_update(|s| complete_error(s, kind, attempt, &error))
                .unwrap_or(false);
            fail(state, error);
            refresh
        }
    }
}
fn send(state: RwSignal<State>, kind: Kind, attempt: Attempt) {
    spawn_local(async move {
        let request = &attempt.request;
        let result = api::request(
            request.method,
            &request.endpoint,
            Some(request.body.clone()),
        )
        .await;
        if complete(state, kind, &attempt, result) {
            actions::load_file_history(state);
        }
    });
}
pub(crate) fn retry_tags(state: RwSignal<State>) {
    if let Some(attempt) = state.try_update(begin_retry).flatten() {
        send(state, Kind::Tags, attempt);
    }
}
pub(crate) fn submit(
    state: RwSignal<State>,
    kind: Kind,
    method: &'static str,
    endpoint: String,
    body: Value,
) {
    if !state.with_untracked(|s| can_start(s, kind)) {
        return;
    }
    let Some(id) = crate::ui::file_operation_id(state) else {
        return;
    };
    if let Some(attempt) =
        state.try_update(|s| begin_submission(s, kind, id, method, endpoint, body))
    {
        send(state, kind, attempt);
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/review/tests.rs"]
pub(crate) mod tests;
