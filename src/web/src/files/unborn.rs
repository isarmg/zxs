//! Explicit first-history review over actual Unborn metadata. Browser recovery
//! retains only the exact original request; it never grants publication authority.
use super::{FileOperation, SaveStatus, actions};
use crate::{
    api,
    ui::{ErrorContext, State, current_session, fail_in},
};
use std::sync::Arc;
use storage_contract::Principal;
use storage_contract::file_draft::DraftLocation;
use web_interface::{prelude::*, tasks::spawn_local};
mod storage;
mod view;
mod wire;
use wire::{Command, Record, Snapshot, invalid};
pub(crate) use view::{view, recovery_view};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    epoch: u64,
    navigation: u64,
    location: DraftLocation,
}
impl Target {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active() || state.files.directory_preview.is_some()
            || state.files.file_preview.is_some()
        {
            return None;
        }
        let entry = state.files.selected.as_ref()?;
        if entry.read_only() { return None; }
        Some(Self {
            epoch: state.session_epoch,
            navigation: state.files.generation,
            location: entry.unborn()?.clone(),
        })
    }
    fn current(&self, state: &State) -> bool {
        Self::capture(state).as_ref() == Some(self)
    }
    fn endpoint(&self) -> String {
        format!(
            "/api/file-manager/files/{}/unborn-working?repository_id={}&path={}&entry_id={}",
            self.location.file, self.location.repository,
            api::segment(self.location.path.as_str()), self.location.entry,
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase { Pending, Sending, Settled }
#[derive(Clone)]
struct Pending {
    record: Arc<Record>,
    phase: Phase,
}
#[derive(Clone, Default)]
pub(crate) struct Model {
    owner: Option<(u64, Principal)>,
    storage_ready: bool,
    pending: Option<Pending>,
    generation: u64,
    reading: Option<Target>,
    review: Option<(Target, Snapshot)>,
    message: String,
    error: Option<api::Error>,
}
impl Model {
    pub(crate) fn clear_review(&mut self) {
        self.generation += 1;
        self.reading = None;
        self.review = None;
        self.message.clear();
    }
    fn owns(&self, state: &State) -> bool {
        state.session_active() && self.owner.as_ref().is_some_and(|(epoch, principal)| {
            *epoch == state.session_epoch && state.principal.as_ref() == Some(principal)
        })
    }
}
pub(crate) fn blocks_writes(state: &State) -> bool {
    let model = &state.files.unborn;
    model.owns(state) && (!model.storage_ready || model.pending.is_some())
}
fn can_review(state: &State) -> bool {
    state.files.unborn.owns(state) && state.files.unborn.storage_ready
        && actions::write_allowed(state) && Target::capture(state).is_some()
        && state.files.unborn.reading.is_none()
}
fn load(state: RwSignal<State>) {
    let target = state.with_untracked(|s| can_review(s).then(|| Target::capture(s)).flatten());
    let Some(target) = target else { return; };
    let generation = state.try_update(|s| {
        s.files.unborn.clear_review();
        s.files.unborn.error = None;
        s.files.unborn.reading = Some(target.clone());
        s.files.unborn.generation
    });
    let Some(generation) = generation else { return; };
    spawn_local(async move {
        let result = api::request("GET", &target.endpoint(), None).await
            .and_then(|value| Snapshot::parse(value, &target.location));
        if !current_session(state, target.epoch) { return; }
        if let Err(error) = &result && error.status == 401 {
            fail_in(state, target.epoch, ErrorContext::Read(target.endpoint()), error.clone());
            return;
        }
        state.update(|s| {
            if !target.current(s) || s.files.unborn.generation != generation { return; }
            s.files.unborn.reading = None;
            match result {
                Ok(snapshot) => s.files.unborn.review = Some((target, snapshot)),
                Err(error) => s.files.unborn.error = Some(error),
            }
        });
    });
}
fn can_submit(state: &State) -> bool {
    can_review(state) && state.files.unborn.review.as_ref().is_some_and(|(target, _)| {
        target.current(state)
            && storage_contract::validate_note(&state.files.unborn.message).is_ok()
    })
}
fn submit(state: RwSignal<State>) {
    let input = state.with_untracked(|s| {
        if !can_submit(s) { return None; }
        let (_, snapshot) = s.files.unborn.review.as_ref()?;
        Some((s.principal.clone()?, snapshot.clone(), s.files.unborn.message.clone()))
    });
    let Some((principal, snapshot, message)) = input else { return; };
    let Some(id) = crate::ui::file_operation_id(state) else { return; };
    let operation_id = id.parse().expect("invariant: UI generates typed operation IDs");
    let record = Record::new(principal, Command {
        operation_id, target: snapshot.target, expected: snapshot.expected, message,
    });
    let record = match record {
        Ok(record) => Arc::new(record),
        Err(error) => {
            state.update(|s| s.files.unborn.error = Some(error));
            return;
        }
    };
    if let Err(error) = storage::save(&record) {
        state.update(|s| {
            s.files.unborn.storage_ready = false;
            s.files.unborn.error = Some(error);
        });
        return;
    }
    state.update(|s| {
        s.files.unborn.pending = Some(Pending {
            record: Arc::clone(&record), phase: Phase::Pending,
        });
        s.files.unborn.error = None;
    });
    send(state, record, true);
}
fn retry_record(state: &State) -> Option<Arc<Record>> {
    let model = &state.files.unborn;
    if !model.owns(state) || state.files.mutation_pending
        || !state.management.storage.writable()
    {
        return None;
    }
    let pending = model.pending.as_ref()?;
    (pending.phase == Phase::Pending).then(|| Arc::clone(&pending.record))
}
fn retry(state: RwSignal<State>) {
    let Some(record) = state.with_untracked(retry_record) else { return; };
    send(state, record, false);
}
fn send(state: RwSignal<State>, record: Arc<Record>, first: bool) {
    if let Err(error) = storage::verify(&record) {
        state.update(|s| s.files.unborn.error = Some(error));
        return;
    }
    let admitted = state.try_update(|s| {
        if !s.files.unborn.owns(s) || s.principal.as_ref() != Some(&record.principal)
            || s.files.mutation_pending || !s.management.storage.writable()
        {
            return None;
        }
        let pending = s.files.unborn.pending.as_mut()?;
        if !Arc::ptr_eq(&pending.record, &record) || pending.phase != Phase::Pending {
            return None;
        }
        pending.phase = Phase::Sending;
        s.files.mutation_pending = true;
        Some(s.session_epoch)
    }).flatten();
    let Some(epoch) = admitted else { return; };
    spawn_local(async move {
        let result = match record.command.body() {
            Ok(body) => api::request("POST", &record.command.endpoint(), Some(body)).await,
            Err(error) => Err(error),
        };
        if !current_session(state, epoch) { return; }
        let owned = state.try_update(|s| {
            let Some(pending) = s.files.unborn.pending.as_mut() else { return false; };
            if !Arc::ptr_eq(&pending.record, &record) { return false; }
            pending.phase = Phase::Pending;
            s.files.mutation_pending = false;
            true
        }) == Some(true);
        if !owned { return; }
        let id = record.command.operation_id.to_string();
        match result {
            Ok(value) => match record.receipt_status(&value) {
                Ok(Some(SaveStatus::Published | SaveStatus::Degraded)) => {
                    state.update(|s| {
                        s.files.unborn.pending.as_mut().unwrap().phase = Phase::Settled;
                        s.files.unborn.clear_review();
                    });
                    cleanup(state);
                    actions::record_receipt(state, &id, value);
                }
                Ok(Some(SaveStatus::Unknown | SaveStatus::Rejected)) | Ok(None) => {
                    state.update(|s| s.files.unborn.error = Some(invalid(
                        "The original first commit is unconfirmed. Retain and retry this request.",
                    )));
                }
                Err(error) => state.update(|s| s.files.unborn.error = Some(error)),
            },
            Err(error) => {
                // A retry rejection says nothing about the first attempt. Only
                // the fresh request's definitive client error releases this fence.
                if first && !error.mutation_outcome_unknown() {
                    state.update(|s| {
                        s.files.unborn.pending.as_mut().unwrap().phase = Phase::Settled;
                        s.files.unborn.clear_review();
                    });
                    cleanup(state);
                    actions::record_operation(state, FileOperation::rejected(&id));
                }
                state.update(|s| s.files.unborn.error = Some(error.clone()));
                fail_in(state, epoch, ErrorContext::Operation(id), error);
            }
        }
    });
}
fn cleanup(state: RwSignal<State>) {
    let record = state.with_untracked(|s| {
        if !s.files.unborn.owns(s) { return None; }
        s.files.unborn.pending.as_ref().filter(|p| p.phase == Phase::Settled)
            .map(|p| Arc::clone(&p.record))
    });
    let Some(record) = record else { return; };
    let result = storage::remove(&record);
    state.update(|s| {
        match result {
            Ok(()) => {
                s.files.unborn.pending = None;
                s.files.unborn.storage_ready = true;
            }
            Err(error) => s.files.unborn.error = Some(error),
        }
    });
}
pub(crate) fn restore(state: RwSignal<State>) {
    let owner = state.with_untracked(|s| {
        if !s.session_active() || s.files.mutation_pending { return None; }
        Some((s.session_epoch, s.principal.clone()?))
    });
    let Some(owner) = owner else { return; };
    let result = storage::load(&owner.1);
    state.update(|s| {
        s.files.unborn.owner = Some(owner);
        s.files.unborn.clear_review();
        match result {
            Ok(record) => {
                s.files.unborn.storage_ready = true;
                s.files.unborn.error = None;
                s.files.unborn.pending = record.map(|record| Pending {
                    record: Arc::new(record), phase: Phase::Pending,
                });
            }
            Err(error) => {
                s.files.unborn.storage_ready = false;
                s.files.unborn.error = Some(error);
            }
        }
    });
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/unborn_tests.rs"]
mod tests;
