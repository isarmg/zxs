//! Recover exact namespace and directory branch requests across sessions.
use super::{FileOperation, SaveStatus, actions};
use crate::{
    api,
    ui::{ErrorContext, State, current_session, fail, fail_in},
};
use json_codec::{Value, json};
use std::sync::Arc;
use storage_contract::{ErrorCode, OperationId};
use web_interface::{prelude::*, tasks::spawn_local};

mod storage;
mod view;
use storage::Record;
pub(crate) use view::view;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReceiptOrigin {
    Lookup,
    OriginalRequest,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Pending,
    Sending,
    Settled,
}
#[derive(Clone)]
struct Retained {
    record: Arc<Record>,
    phase: Phase,
}
#[derive(Clone, Default)]
pub(crate) struct Model {
    owner: Option<(u64, String)>,
    records: Vec<Retained>,
    error: Option<api::Error>,
}
impl Model {
    fn owns(&self, state: &State) -> bool {
        state.session_active()
            && self.owner.as_ref().is_some_and(|(epoch, principal)| {
                *epoch == state.session_epoch && *principal == state.principal
            })
    }
    fn ready(&self, state: &State) -> bool {
        self.owns(state) && self.records.is_empty() && self.error.is_none()
    }
    fn restore_records(&mut self, loaded: Result<Vec<Arc<Record>>, api::Error>) {
        self.error = loaded.and_then(|records| self.merge(records)).err();
    }
    fn merge(&mut self, records: Vec<Arc<Record>>) -> Result<(), api::Error> {
        let mut next = self.records.clone();
        for record in records {
            if let Some(previous) = next
                .iter()
                .find(|retained| retained.record.operation() == record.operation())
            {
                if previous.record.bytes != record.bytes {
                    return Err(failure(
                        "An original folder request changed; it was preserved.",
                    ));
                }
            } else {
                next.push(Retained {
                    record,
                    phase: Phase::Pending,
                });
            }
        }
        if next.len() > storage::MAX_RECORDS {
            return Err(limit(
                "Folder recovery exceeds 32 requests; nothing was discarded.",
            ));
        }
        next.sort_by_key(|retained| retained.record.operation());
        self.records = next;
        Ok(())
    }
}
fn failure(message: &str) -> api::Error {
    api::error(ErrorCode::InvalidRequest, message)
}
fn limit(message: &str) -> api::Error {
    api::error(ErrorCode::ResourceLimit, message)
}
fn owner(state: &State) -> Option<(u64, String)> {
    state
        .session_active()
        .then(|| (state.session_epoch, state.principal.clone()))
}
pub(crate) fn blocks_writes(state: &State) -> bool {
    let model = &state.files.mkdir;
    model.owns(state) && (!model.records.is_empty() || model.error.is_some())
}
pub(crate) fn default_completion_applies(state: &State, id: &str) -> bool {
    state
        .files
        .mkdir
        .records
        .iter()
        .find(|retained| retained.record.operation().to_string() == id)
        .is_none_or(|retained| retained.record.checkout_applies(state))
}
pub(crate) fn start(state: RwSignal<State>, name: &str) -> Option<String> {
    if !state
        .with_untracked(|s| actions::write_allowed(s) && s.files.loaded && s.files.mkdir.ready(s))
    {
        return None;
    }
    capture_and_retain(state, |s, operation| Record::capture(s, operation, name))
}

pub(super) fn start_relocation(
    state: RwSignal<State>,
    movement: storage_contract::file_relocation::ObjectMoveRequest,
) -> Option<String> {
    if !state
        .with_untracked(|s| actions::write_allowed(s) && s.files.loaded && s.files.mkdir.ready(s))
    {
        return None;
    }
    capture_and_retain(state, |s, operation| {
        Record::capture_relocation(s, operation, movement)
    })
}

pub(super) fn start_directory_branch(
    state: RwSignal<State>,
    source: storage_contract::directory_git::DirectoryBranchCreateSource,
    name: String,
) -> Option<String> {
    if !state.with_untracked(|s| {
        actions::write_allowed(s)
            && s.files.loaded
            && s.files.mkdir.ready(s)
            && s.files.directory_branches_supported
            && s.files.repository_id == source.repository_id
            && s.files.namespace_head == source.namespace_head
    }) {
        return None;
    }
    capture_and_retain(state, |s, operation| {
        Record::capture_directory_branch(s, operation, source, name)
    })
}

pub(super) fn start_directory_checkout(state: RwSignal<State>, branch: &str) {
    if !state.with_untracked(|s| {
        actions::default_write_allowed(s)
            && s.files.loaded
            && s.files.mkdir.ready(s)
            && s.files.directory_branches_supported
            && s.files.directory_preview.as_ref().is_some_and(|preview| {
                preview.at_directory(&s.files.directory)
                    && preview.branch().is_some_and(|choice| choice.id() == branch)
            })
    }) {
        return;
    }
    capture_and_retain(state, Record::capture_directory_checkout);
}

fn capture_and_retain(
    state: RwSignal<State>,
    capture: impl FnOnce(&State, OperationId) -> Result<Record, api::Error>,
) -> Option<String> {
    let operation = crate::ui::file_operation_id(state)?
        .parse()
        .expect("invariant: UI generated a canonical operation ID");
    match state.with_untracked(|s| capture(s, operation)) {
        Ok(record) => retain(state, Arc::new(record)),
        Err(error) => {
            fail(state, error);
            None
        }
    }
}

fn retain(state: RwSignal<State>, record: Arc<Record>) -> Option<String> {
    let id = record.operation().to_string();
    if let Err(error) = storage::save(&record) {
        state.update(|s| {
            s.files.mkdir.error = Some(error);
            s.notices.clear_operation_notice(&id);
        });
        return None;
    }
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| {
        if record.is_directory_checkout() {
            s.files.default_operation = Some(id.clone());
        }
        s.files.mkdir.records.push(Retained {
            record: Arc::clone(&record),
            phase: Phase::Pending,
        });
    });
    send(state, epoch, record, Submission::First);
    Some(id)
}

#[derive(Clone, Copy)]
enum Submission {
    First,
    OriginalRetry,
}
fn retry_record(state: &State, epoch: u64, operation: OperationId) -> Option<Arc<Record>> {
    let model = &state.files.mkdir;
    if !model.owns(state)
        || epoch != state.session_epoch
        || !state.management.storage.writable()
        || model.error.is_some()
        || state.files.mutation_pending
        || state
            .files
            .polling_operations
            .contains(&operation.to_string())
    {
        return None;
    }
    model
        .records
        .iter()
        .find(|retained| {
            retained.record.operation() == operation && retained.phase == Phase::Pending
        })
        .map(|retained| Arc::clone(&retained.record))
}
fn retry(state: RwSignal<State>, epoch: u64, operation: OperationId) {
    let Some(record) = state.with_untracked(|s| retry_record(s, epoch, operation)) else {
        return;
    };
    if let Err(error) = storage::verify(&record) {
        state.update(|s| s.files.mkdir.error = Some(error));
        return;
    }
    send(state, epoch, record, Submission::OriginalRetry);
}
fn send(state: RwSignal<State>, epoch: u64, record: Arc<Record>, submission: Submission) {
    let id = record.operation().to_string();
    let admitted = state
        .try_update(|s| {
            if !s.files.mkdir.owns(s)
                || s.session_epoch != epoch
                || s.principal != record.principal()
                || s.files.mutation_pending
            {
                return false;
            }
            let Some(retained) = s.files.mkdir.records.iter_mut().find(|retained| {
                Arc::ptr_eq(&retained.record, &record) && retained.phase == Phase::Pending
            }) else {
                return false;
            };
            retained.phase = Phase::Sending;
            s.files.mutation_pending = true;
            true
        })
        .unwrap_or(false);
    if !admitted {
        return;
    }
    actions::record_operation(state, FileOperation::unknown(&id));
    spawn_local(async move {
        if !current_session(state, epoch) {
            return;
        }
        let (method, endpoint, body) = record.request();
        let result = api::request(method, &endpoint, Some(body)).await;
        if !current_session(state, epoch) {
            return;
        }
        state.update(|s| {
            if let Some(retained) = s
                .files
                .mkdir
                .records
                .iter_mut()
                .find(|retained| Arc::ptr_eq(&retained.record, &record))
            {
                retained.phase = Phase::Pending;
            }
        });
        match result {
            Ok(value) => {
                let operation = if record.matches_receipt(&value, ReceiptOrigin::OriginalRequest) {
                    FileOperation::from_receipt(&id, &value)
                } else {
                    FileOperation::unknown(&id)
                };
                actions::record_operation(state, operation);
            }
            Err(error) => match submission {
                Submission::First => actions::record_failure(state, epoch, &id, error),
                Submission::OriginalRetry => {
                    // Rejection of a retry is not evidence about the first request.
                    actions::record_operation(state, FileOperation::unknown(&id));
                    fail_in(state, epoch, ErrorContext::Operation(id.clone()), error);
                }
            },
        }
        state.update(|s| s.files.mutation_pending = false);
    });
}
pub(crate) fn poll_allowed(state: &State, id: &str) -> bool {
    let model = &state.files.mkdir;
    model
        .records
        .iter()
        .find(|retained| retained.record.operation().to_string() == id)
        .is_none_or(|retained| {
            model.owns(state) && retained.phase == Phase::Pending && retained.record.can_lookup()
        })
}
pub(crate) fn receipt_matches(state: &State, id: &str, receipt: &Value) -> bool {
    let model = &state.files.mkdir;
    model
        .records
        .iter()
        .find(|retained| retained.record.operation().to_string() == id)
        .is_none_or(|retained| {
            model.owns(state)
                && retained
                    .record
                    .matches_receipt(receipt, ReceiptOrigin::Lookup)
        })
}
pub(crate) fn settle(model: &mut Model, id: &str, status: SaveStatus) {
    if status != SaveStatus::Unknown
        && let Some(retained) = model
            .records
            .iter_mut()
            .find(|retained| retained.record.operation().to_string() == id)
    {
        retained.phase = Phase::Settled;
    }
}
pub(crate) fn finish(state: RwSignal<State>, id: &str) {
    let record = state.with_untracked(|s| {
        let model = &s.files.mkdir;
        if !model.owns(s) {
            return None;
        }
        model
            .records
            .iter()
            .find(|retained| {
                retained.record.operation().to_string() == id && retained.phase == Phase::Settled
            })
            .map(|retained| Arc::clone(&retained.record))
    });
    let Some(record) = record else {
        return;
    };
    let result = storage::remove(&record);
    if state.try_update(|s| complete_cleanup(&mut s.files.mkdir, &record, result)) == Some(true) {
        // Removing one key cannot clear an unrelated scan or binding error.
        // Reuse the bounded original-record read before admitting new writes.
        restore(state);
    }
}
fn complete_cleanup(
    model: &mut Model,
    record: &Arc<Record>,
    result: Result<(), api::Error>,
) -> bool {
    match result {
        Ok(()) => {
            model
                .records
                .retain(|retained| !Arc::ptr_eq(&retained.record, record));
            model.error.is_some()
        }
        Err(error) => {
            model.error = Some(error);
            false
        }
    }
}
/// Authentication hydrates before new writes. Reloading local hints and polling
/// never repeat a mutation or trust a stored completion claim.
pub(crate) fn restore(state: RwSignal<State>) {
    let Some(current) = state.with_untracked(owner) else {
        return;
    };
    if state.with_untracked(|s| {
        s.files.mkdir.owner.as_ref() == Some(&current) && s.files.mutation_pending
    }) {
        return;
    }
    let loaded = storage::load(&current.1);
    let operations = state
        .try_update(|s| {
            if owner(s).as_ref() != Some(&current) {
                return Vec::new();
            }
            let changed = s.files.mkdir.owner.as_ref() != Some(&current);
            if changed {
                adopt_owner(s, &current);
            }
            let model = &mut s.files.mkdir;
            model.restore_records(loaded);
            let operations = model
                .records
                .iter()
                .filter(|retained| retained.phase == Phase::Pending)
                .map(|retained| retained.record.operation().to_string())
                .collect::<Vec<_>>();
            // A previous generic observation cannot authenticate newly loaded
            // browser bytes. Poll again with this exact original request.
            s.files
                .operations
                .retain(|operation| !operations.iter().any(|id| id == operation.id()));
            operations
        })
        .unwrap_or_default();
    for id in operations {
        state.update(|s| {
            if s.files.mkdir.records.iter().any(|retained| {
                retained.record.operation().to_string() == id
                    && retained.record.is_directory_checkout()
            }) {
                s.files.default_operation = Some(id.clone());
            }
        });
        actions::record_operation(state, FileOperation::unknown(&id));
    }
    actions::poll(state);
}
fn adopt_owner(state: &mut State, current: &(u64, String)) {
    let model = &mut state.files.mkdir;
    let old_ids: Vec<_> = model
        .records
        .iter()
        .map(|retained| retained.record.operation().to_string())
        .collect();
    if state
        .files
        .default_operation
        .as_ref()
        .is_some_and(|id| old_ids.contains(id))
    {
        state.files.default_operation = None;
    }
    if model
        .records
        .iter()
        .any(|retained| retained.phase == Phase::Sending)
    {
        state.files.mutation_pending = false;
    }
    state
        .files
        .operations
        .retain(|operation| !old_ids.iter().any(|id| id == operation.id()));
    state
        .files
        .polling_operations
        .retain(|id| !old_ids.contains(id));
    if model
        .owner
        .as_ref()
        .is_none_or(|(_, principal)| principal != &current.1)
    {
        model.records.clear();
    } else {
        for retained in &mut model.records {
            retained.phase = Phase::Pending;
        }
    }
    model.owner = Some(current.clone());
    model.error = None;
}
