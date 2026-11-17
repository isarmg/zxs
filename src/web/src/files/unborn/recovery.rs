//! Fencing and recovery for the original first commit, without assigning a new ID.
use super::{Model, Original, Pending, Phase, SaveStatus, invalid, storage};
use crate::api;
use crate::files::{FileOperation, actions};
use crate::ui::{ErrorContext, State, fail_in};
use std::sync::Arc;
use storage_contract::Principal;
use web_interface::{prelude::*, tasks::spawn_local};

pub(super) fn owns_phase(
    state: &State,
    epoch: u64,
    original: &Arc<Original>,
    phase: Phase,
) -> bool {
    let model = &state.files.unborn;
    state.session_epoch == epoch
        && model.owns(state)
        && state.principal.as_ref() == Some(&original.request.principal)
        && model
            .pending
            .as_ref()
            .is_some_and(|p| Arc::ptr_eq(&p.original, original) && p.phase == phase)
}

pub(super) fn complete_save(
    state: &mut State,
    epoch: u64,
    original: &Arc<Original>,
    result: Result<(), api::Error>,
) -> bool {
    if !owns_phase(state, epoch, original, Phase::Retaining) {
        return false;
    }
    let model = &mut state.files.unborn;
    match result {
        Ok(()) => {
            model.pending.as_mut().unwrap().phase = Phase::Pending;
            model.error = None;
            true
        }
        Err(error) => {
            model.pending = None;
            model.storage_ready = false;
            model.error = Some(error);
            false
        }
    }
}

pub(super) fn retry_record(state: &State) -> Option<Arc<Original>> {
    let model = &state.files.unborn;
    if !model.owns(state)
        || !model.storage_ready
        || state.files.mutation_pending
        || !state.management.storage.writable()
    {
        return None;
    }
    let pending = model.pending.as_ref()?;
    (pending.phase == Phase::Pending).then(|| Arc::clone(&pending.original))
}

#[derive(Clone, Copy)]
pub(super) enum Submission {
    First,
    OriginalRetry,
}
impl Submission {
    fn queued(self) -> Phase {
        match self {
            Self::First => Phase::Retaining,
            Self::OriginalRetry => Phase::RetryQueued,
        }
    }
}

pub(super) fn retry(state: RwSignal<State>) {
    let Some((epoch, original)) = state
        .try_update(|s| {
            let original = retry_record(s)?;
            s.files.unborn.pending.as_mut()?.phase = Phase::RetryQueued;
            Some((s.session_epoch, original))
        })
        .flatten()
    else {
        return;
    };
    dispatch(state, epoch, original, Submission::OriginalRetry);
}

pub(super) fn dispatch(
    state: RwSignal<State>,
    epoch: u64,
    original: Arc<Original>,
    submission: Submission,
) {
    spawn_local(async move {
        let current = Arc::clone(&original);
        let result = storage::while_held(&original.request.principal, move |slot| async move {
            send_locked(state, epoch, current, submission, slot).await;
            Ok(())
        })
        .await;
        if let Err(error) = result {
            state.try_update(|s| storage_failure(s, epoch, &original, submission, error));
        }
    });
}

fn storage_failure(
    state: &mut State,
    epoch: u64,
    original: &Arc<Original>,
    submission: Submission,
    error: api::Error,
) {
    if !owns_phase(state, epoch, original, submission.queued()) {
        return;
    }
    match submission {
        Submission::First => {
            complete_save(state, epoch, original, Err(error));
        }
        Submission::OriginalRetry => {
            state.files.unborn.pending.as_mut().unwrap().phase = Phase::Pending;
            state.files.unborn.storage_ready = false;
            state.files.unborn.error = Some(error);
        }
    }
}

async fn send_locked(
    state: RwSignal<State>,
    epoch: u64,
    original: Arc<Original>,
    submission: Submission,
    slot: storage::Slot,
) {
    // Check at lock grant, before changing either account's retained material.
    if state.try_with_untracked(|s| owns_phase(s, epoch, &original, submission.queued()))
        != Some(true)
    {
        return;
    }
    let retained = match submission {
        Submission::First => slot.save(&original),
        Submission::OriginalRetry => slot.verify(&original),
    };
    if let Err(error) = retained {
        state.try_update(|s| storage_failure(s, epoch, &original, submission, error));
        return;
    }
    let admitted = state.try_update(|s| {
        if !owns_phase(s, epoch, &original, submission.queued()) {
            return false;
        }
        match submission {
            Submission::First => {
                complete_save(s, epoch, &original, Ok(()));
            }
            Submission::OriginalRetry => {
                s.files.unborn.pending.as_mut().unwrap().phase = Phase::Pending;
            }
        }
        if !s.files.unborn.storage_ready
            || !s.management.storage.writable()
            || s.files.mutation_pending
        {
            return false;
        }
        s.files.unborn.pending.as_mut().unwrap().phase = Phase::Sending;
        s.files.mutation_pending = true;
        true
    });
    if admitted != Some(true) {
        return;
    }
    let record = &original.request;
    let result = match record.command.body() {
        Ok(body) => api::request("POST", &record.command.endpoint(), Some(body)).await,
        Err(error) => Err(error),
    };
    if !finish_io(state, epoch, &original, Phase::Sending) {
        return;
    }
    let id = record.command.operation_id.to_string();
    match result {
        Ok(value) => {
            if settle_receipt(state, &original, &value) {
                finish_locked(state, epoch, &original, &slot);
                actions::record_receipt(state, &id, value);
            }
        }
        Err(error) => {
            // The first submission owns the slot continuously from its empty
            // check through this response. Another tab cannot already have
            // dispatched this original. A later retry has no such proof.
            if matches!(submission, Submission::First) && !error.mutation_outcome_unknown() {
                match slot.remove(&original) {
                    Ok(()) => {
                        state.update(|s| {
                            s.files.unborn.pending = None;
                            s.files.unborn.clear_review();
                        });
                        actions::record_operation(state, FileOperation::rejected(&id));
                    }
                    Err(storage_error) => {
                        // Once this lease is released another tab may retry the
                        // retained intent. Do not claim durable terminal proof.
                        state.update(|s| {
                            s.files.unborn.storage_ready = false;
                            s.files.unborn.error = Some(storage_error);
                        });
                    }
                }
            }
            fail_in(state, epoch, ErrorContext::Operation(id), error);
        }
    }
}

fn finish_io(state: RwSignal<State>, epoch: u64, original: &Arc<Original>, phase: Phase) -> bool {
    state.try_update(|s| {
        if !owns_phase(s, epoch, original, phase) {
            return false;
        }
        s.files.unborn.pending.as_mut().unwrap().phase = Phase::Pending;
        if phase == Phase::Sending {
            s.files.mutation_pending = false;
        }
        true
    }) == Some(true)
}

fn settle_receipt(state: RwSignal<State>, original: &Original, value: &json_codec::Value) -> bool {
    match original.request.receipt_status(value) {
        Ok(Some(SaveStatus::Published | SaveStatus::Degraded | SaveStatus::Rejected)) => {
            state.update(|s| {
                s.files.unborn.pending.as_mut().unwrap().phase = Phase::Settled;
                s.files.unborn.clear_review();
            });
            true
        }
        Ok(Some(SaveStatus::Unknown) | None) => {
            state.update(|s| s.files.unborn.error = Some(invalid(
                "No complete receipt matches the original first commit. Its request was retained.",
            )));
            false
        }
        Err(error) => {
            state.update(|s| s.files.unborn.error = Some(error));
            false
        }
    }
}

fn finish_locked(
    state: RwSignal<State>,
    epoch: u64,
    original: &Arc<Original>,
    slot: &storage::Slot,
) {
    if state.try_update(|s| {
        if !owns_phase(s, epoch, original, Phase::Settled) {
            return false;
        }
        s.files.unborn.pending.as_mut().unwrap().phase = Phase::Removing;
        true
    }) == Some(true)
    {
        let result = slot.remove(original);
        state.try_update(|s| complete_cleanup(s, epoch, original, result));
    }
}

pub(super) fn cleanup(state: RwSignal<State>) {
    let Some((epoch, original)) = state
        .try_update(|s| {
            if !s.files.unborn.owns(s) {
                return None;
            }
            let pending = s.files.unborn.pending.as_mut()?;
            if pending.phase != Phase::Settled {
                return None;
            }
            pending.phase = Phase::Removing;
            Some((s.session_epoch, Arc::clone(&pending.original)))
        })
        .flatten()
    else {
        return;
    };
    spawn_local(async move {
        let current = Arc::clone(&original);
        let result = storage::while_held(&original.request.principal, move |slot| async move {
            if state.try_with_untracked(|s| owns_phase(s, epoch, &current, Phase::Removing))
                == Some(true)
            {
                slot.remove(&current)
            } else {
                Ok(())
            }
        })
        .await;
        state.try_update(|s| complete_cleanup(s, epoch, &original, result));
    });
}

pub(super) fn lookup(state: RwSignal<State>) {
    let Some((epoch, original)) = state
        .try_update(|s| {
            if !s.files.unborn.owns(s) {
                return None;
            }
            let p = s.files.unborn.pending.as_mut()?;
            if p.phase != Phase::Pending {
                return None;
            }
            p.phase = Phase::LookingUp;
            Some((s.session_epoch, Arc::clone(&p.original)))
        })
        .flatten()
    else {
        return;
    };
    spawn_local(async move {
        if state.try_with_untracked(|s| owns_phase(s, epoch, &original, Phase::LookingUp))
            != Some(true)
        {
            return;
        }
        let id = original.request.command.operation_id.to_string();
        let path = format!("/api/operations/{id}/outcome");
        let result = api::request("GET", &path, None).await;
        if !finish_io(state, epoch, &original, Phase::LookingUp) {
            return;
        }
        match result {
            Ok(value) => {
                if settle_receipt(state, &original, &value) {
                    cleanup(state);
                    actions::record_receipt(state, &id, value);
                }
            }
            Err(error) => {
                state.update(|s| s.files.unborn.error = Some(error.clone()));
                fail_in(state, epoch, ErrorContext::Read(path), error);
            }
        }
    });
}

pub(super) fn complete_cleanup(
    state: &mut State,
    epoch: u64,
    original: &Arc<Original>,
    result: Result<(), api::Error>,
) {
    if !owns_phase(state, epoch, original, Phase::Removing) {
        return;
    }
    let model = &mut state.files.unborn;
    match result {
        Ok(()) => {
            model.pending = None;
            model.storage_ready = true;
            model.error = None;
        }
        Err(error) => {
            model.pending.as_mut().unwrap().phase = Phase::Settled;
            model.error = Some(error);
        }
    }
}

pub(crate) fn restore(state: RwSignal<State>) {
    let owner = state.with_untracked(|s| {
        if !s.session_active() || s.files.mutation_pending {
            return None;
        }
        Some((s.session_epoch, s.principal.clone()?))
    });
    let Some(owner) = owner else {
        return;
    };
    if state.with_untracked(|s| {
        s.files.unborn.owner.as_ref() == Some(&owner)
            && s.files
                .unborn
                .pending
                .as_ref()
                .is_some_and(|p| p.phase != Phase::Pending && p.phase != Phase::Settled)
    }) {
        return;
    }
    let result = storage::load(&owner.1);
    state.update(|s| restore_loaded(&mut s.files.unborn, owner, result));
    lookup(state);
}
pub(super) fn restore_loaded(
    model: &mut Model,
    owner: (u64, Principal),
    result: Result<Option<Original>, api::Error>,
) {
    if model.owner.as_ref() != Some(&owner) {
        match model.owner.as_ref() {
            Some((_, principal)) if principal == &owner.1 => {
                if let Some(pending) = &mut model.pending {
                    match pending.phase {
                        // A prior session cannot start the POST after the lock
                        // completes. Adopt only what the current read actually finds.
                        Phase::Retaining => model.pending = None,
                        Phase::Sending | Phase::LookingUp | Phase::RetryQueued => {
                            pending.phase = Phase::Pending
                        }
                        Phase::Removing => pending.phase = Phase::Settled,
                        Phase::Pending | Phase::Settled => {}
                    }
                }
            }
            _ => model.pending = None,
        }
    }
    model.owner = Some(owner);
    model.clear_review();
    match result {
        Ok(record) => {
            if let Some(pending) = &model.pending {
                let same = record.as_ref() == Some(pending.original.as_ref());
                let other_id = record.as_ref().is_some_and(|other| {
                    other.request.command.operation_id
                        != pending.original.request.command.operation_id
                });
                if !same && !(pending.phase == Phase::Settled && (record.is_none() || other_id)) {
                    model.storage_ready = false;
                    model.error = Some(invalid(
                        "The retained original changed or is missing. Its in-memory request was preserved.",
                    ));
                    return;
                }
                if same {
                    model.storage_ready = true;
                    model.error = None;
                    return;
                }
            }
            // A confirmed original can yield to an independently admitted new
            // operation on read. Never delete that replacement or dispatch a POST.
            model.pending = record.map(|original| Pending {
                original: Arc::new(original),
                phase: Phase::Pending,
            });
            model.storage_ready = true;
            model.error = None;
        }
        Err(error) => {
            model.storage_ready = false;
            model.error = Some(error);
        }
    }
}
