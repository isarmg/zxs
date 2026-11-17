//! Every local decision uses its exact account, epoch and in-memory original,
//! rechecked *after* waiting for its same-origin Web Lock. Holding the lock
//! through a first POST is what distinguishes a proven fresh rejection from
//! another tab's retry after an unknown result.
use crate::{
    api,
    files::{
        actions,
        directory_snapshot::{
            Command, Pending, Phase, Record,
            storage::{self, Original, Slot},
            wire, worktree,
        },
    },
    ui::{ErrorContext, State, fail_in},
};
use std::sync::Arc;
use web_interface::{prelude::*, tasks::spawn_local};
type Session = RwSignal<State>;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Action {
    First,
    Retry,
    Read,
    Clear,
}
impl Action {
    fn phase(self) -> Phase {
        match self {
            Self::First => Phase::Retaining,
            Self::Retry | Self::Read => Phase::Sending,
            Self::Clear => Phase::Cleaning,
        }
    }
}
fn owns(s: &State, epoch: u64, original: &Arc<Original>, phase: Phase) -> bool {
    let m = &s.files.directory_snapshot;
    s.session_epoch == epoch
        && m.owns(s)
        && s.principal.as_ref() == Some(&original.principal)
        && m.pending
            .as_ref()
            .is_some_and(|p| p.phase == phase && Arc::ptr_eq(&p.record, original))
}
fn query_available(s: &State, original: &Original) -> bool {
    // A directory publication receipt and historical Named creation read are
    // read-only. Preserve the existing availability gate for other families.
    match original.command {
        Command::Commit { .. } | Command::CreateWorktree { .. } => s.management.storage.readable(),
        _ => s.management.storage.writable(),
    }
}
pub(in crate::files::directory_snapshot) fn retry_record(s: &State) -> Option<Arc<Original>> {
    let m = &s.files.directory_snapshot;
    if !m.owns(s) || s.files.mutation_pending {
        return None;
    }
    let pending = m.pending.as_ref()?;
    (pending.phase == Phase::Pending
        && s.principal.as_ref() == Some(&pending.record.principal)
        && query_available(s, &pending.record))
    .then(|| Arc::clone(&pending.record))
}
pub(in crate::files::directory_snapshot) fn retry_post_record(s: &State) -> Option<Arc<Original>> {
    (s.management.storage.writable() && s.files.directory_snapshot.storage_ready)
        .then(|| retry_record(s))
        .flatten()
        .filter(|r| r.command.can_post())
}
pub(in crate::files::directory_snapshot) fn retain_and_send(
    state: Session,
    record: Result<Record, api::Error>,
) {
    let original = match record.and_then(Original::fresh) {
        Ok(r) => Arc::new(r),
        Err(e) => {
            state.update(|s| s.files.directory_snapshot.error = Some(e));
            return;
        }
    };
    let epoch = state
        .try_update(|s| {
            let m = &s.files.directory_snapshot;
            if !m.owns(s)
                || !m.storage_ready
                || m.pending.is_some()
                || s.files.mutation_pending
                || !s.management.storage.writable()
                || s.principal.as_ref() != Some(&original.principal)
                || !original.command.can_post()
            {
                return None;
            }
            let m = &mut s.files.directory_snapshot;
            m.pending = Some(Pending {
                record: Arc::clone(&original),
                phase: Phase::Retaining,
            });
            m.error = None;
            m.publication = None;
            m.observed = None;
            m.observe_on_restore = None;
            s.files.mutation_pending = true;
            Some(s.session_epoch)
        })
        .flatten();
    if let Some(epoch) = epoch {
        dispatch(state, epoch, original, Action::First);
    }
}
fn existing(state: Session, action: Action) {
    let queued = state
        .try_update(|s| {
            let original = match action {
                Action::Retry => retry_post_record(s)?,
                Action::Read => {
                    let r = retry_record(s)?;
                    r.command.outcome_endpoint()?;
                    r
                }
                Action::Clear => {
                    let m = &s.files.directory_snapshot;
                    if !m.owns(s) || s.files.mutation_pending {
                        return None;
                    }
                    let p = m.pending.as_ref()?;
                    if p.phase != Phase::Settled
                        || s.principal.as_ref() != Some(&p.record.principal)
                    {
                        return None;
                    }
                    Arc::clone(&p.record)
                }
                Action::First => return None,
            };
            s.files.directory_snapshot.observe_on_restore = None;
            s.files.directory_snapshot.pending.as_mut()?.phase = action.phase();
            s.files.mutation_pending = true;
            Some((s.session_epoch, original))
        })
        .flatten();
    if let Some((epoch, original)) = queued {
        dispatch(state, epoch, original, action);
    }
}
pub(in crate::files::directory_snapshot) fn retry(state: Session) {
    existing(state, Action::Retry);
}
pub(in crate::files::directory_snapshot) fn lookup(state: Session) {
    existing(state, Action::Read);
}
pub(in crate::files::directory_snapshot) fn observe_restored(
    s: &State,
) -> Option<(u64, storage_contract::OperationId)> {
    let expected = s.files.directory_snapshot.observe_on_restore?;
    let r = retry_record(s)?;
    (matches!(&r.command, Command::Commit { .. }) && expected == r.command.operation())
        .then_some((s.session_epoch, expected))
}
pub(in crate::files::directory_snapshot) fn cleanup(state: Session) {
    existing(state, Action::Clear);
}

fn dispatch(state: Session, epoch: u64, original: Arc<Original>, action: Action) {
    spawn_local(async move {
        let held = Arc::clone(&original);
        let result = storage::while_held(&original.principal, move |slot| async move {
            run(state, epoch, &held, action, &slot).await
        })
        .await;
        if let Err(error) = result {
            let _ = state.try_update(|s| fail_local(s, epoch, &original, action, error));
        }
    });
}
fn fail_local(
    s: &mut State,
    epoch: u64,
    original: &Arc<Original>,
    action: Action,
    error: api::Error,
) {
    let phase = if owns(s, epoch, original, action.phase()) {
        action.phase()
    } else if action == Action::First && owns(s, epoch, original, Phase::Sending) {
        Phase::Sending
    } else {
        return;
    };
    let m = &mut s.files.directory_snapshot;
    match phase {
        // No successful retain was observed. A quota/readback error can still
        // leave bytes; require a real rescan, never clear the storage key here.
        Phase::Retaining => {
            m.pending = None;
            m.storage_ready = false;
        }
        Phase::Sending => {
            if let Some(p) = m.pending.as_mut() {
                p.phase = Phase::Pending;
            }
        }
        Phase::Cleaning => {
            if let Some(p) = m.pending.as_mut() {
                p.phase = Phase::Settled;
            }
        }
        Phase::Pending | Phase::Settled => return,
    }
    m.error = Some(error);
    s.files.mutation_pending = false;
}
fn ended(s: &mut State, epoch: u64, original: &Arc<Original>, phase: Phase) -> bool {
    if !owns(s, epoch, original, phase) {
        return false;
    }
    let Some(p) = s.files.directory_snapshot.pending.as_mut() else {
        return false;
    };
    p.phase = Phase::Pending;
    s.files.mutation_pending = false;
    true
}

async fn run(
    state: Session,
    epoch: u64,
    original: &Arc<Original>,
    action: Action,
    slot: &Slot,
) -> Result<(), api::Error> {
    if state.try_with_untracked(|s| owns(s, epoch, original, action.phase())) != Some(true) {
        return Ok(());
    }
    if action == Action::Clear {
        return clear_locked(state, epoch, original, Phase::Cleaning, slot);
    }
    let ready = state.with_untracked(|s| {
        if action == Action::Read {
            query_available(s, original)
        } else {
            s.management.storage.writable()
        }
    });
    if !ready {
        return Err(wire::invalid(
            "storage availability changed while waiting for the original",
        ));
    }
    if action == Action::First {
        slot.save(original)?;
        if state.try_update(|s| {
            if !owns(s, epoch, original, Phase::Retaining) {
                return false;
            }
            let m = &mut s.files.directory_snapshot;
            let Some(p) = m.pending.as_mut() else {
                return false;
            };
            p.phase = Phase::Sending;
            m.retained_count = 1;
            if matches!(original.command, Command::Commit { .. }) {
                m.kept = None;
            }
            true
        }) != Some(true)
        {
            return Ok(());
        }
    } else if action == Action::Retry {
        slot.verify(original)?;
    }
    if action == Action::Read {
        // The query is protected by the real server and only uses the exact
        // admitted in-memory original. If local text went missing or changed,
        // a read may still establish its actual outcome. Never POST those
        // changed bytes or let them pass terminal compare/remove.
        let endpoint = original
            .command
            .outcome_endpoint()
            .ok_or_else(|| wire::invalid("no read outcome for this original"))?;
        let read = api::request("GET", &endpoint, None)
            .await
            .and_then(|v| original.lookup_result(v));
        if state.try_update(|s| ended(s, epoch, original, Phase::Sending)) != Some(true) {
            return Ok(());
        }
        match read {
            Ok(wire::LookupResult::Pending) => {
                state.update(|s| s.files.directory_snapshot.error = None)
            }
            Ok(wire::LookupResult::DirectoryObserved(observation)) => {
                state.update(|s| observe(s, epoch, original, observation));
            }
            Ok(wire::LookupResult::Complete(p)) => {
                finish(state, epoch, original, slot, Ok(p), false)
            }
            Err(e) => finish(state, epoch, original, slot, Err(e), false),
        }
        return Ok(());
    }
    let body = original.command.body()?;
    let response = api::request("POST", &original.command.endpoint(), Some(body)).await;
    if state.try_with_untracked(|s| owns(s, epoch, original, Phase::Sending)) != Some(true) {
        return Ok(());
    }
    let (result, rejected_fresh) = match response {
        Ok(value) if matches!(original.command, Command::Initialize { .. }) => (
            super::verify_initialization(original, value)
                .await
                .and_then(|v| original.result(v)),
            false,
        ),
        Ok(value) => (original.result(value), false),
        Err(error) => {
            let rejected = action == Action::First && !error.mutation_outcome_unknown();
            (Err(error), rejected)
        }
    };
    if state.try_update(|s| ended(s, epoch, original, Phase::Sending)) == Some(true) {
        finish(state, epoch, original, slot, result, rejected_fresh);
    }
    Ok(())
}

fn observe(
    s: &mut State,
    epoch: u64,
    original: &Arc<Original>,
    observation: wire::DirectoryObservation,
) {
    if !owns(s, epoch, original, Phase::Pending) {
        return;
    }
    let m = &mut s.files.directory_snapshot;
    m.observed = Some((Arc::clone(original), observation));
    m.error = None;
    // Intentionally no settled transition, bookmark, cleanup, synthetic
    // write or success notice. Only the direct full original POST can compare
    // this browser record with the permanent request.
}

fn clear_locked(
    state: Session,
    epoch: u64,
    original: &Arc<Original>,
    phase: Phase,
    slot: &Slot,
) -> Result<(), api::Error> {
    if state.try_with_untracked(|s| owns(s, epoch, original, phase)) != Some(true) {
        return Ok(());
    }
    slot.remove(original)?;
    if state.try_update(|s| {
        if !owns(s, epoch, original, phase) {
            return false;
        }
        s.files.mutation_pending = false;
        let m = &mut s.files.directory_snapshot;
        m.pending = None;
        m.observed = None;
        m.storage_ready = true;
        true
    }) == Some(true)
    {
        super::restore(state);
        actions::refresh_directory(state);
    }
    Ok(())
}
fn finish(
    state: Session,
    epoch: u64,
    original: &Arc<Original>,
    slot: &Slot,
    result: Result<wire::Publication, api::Error>,
    rejected_fresh: bool,
) {
    if state.try_with_untracked(|s| owns(s, epoch, original, Phase::Pending)) != Some(true) {
        return;
    }
    match result {
        Ok(publication) => {
            if let Err(error) = worktree::remember_publication(original, &publication) {
                state.update(|s| {
                    let m = &mut s.files.directory_snapshot;
                    m.publication = Some(publication);
                    m.error = Some(error);
                });
                return;
            }
            state.update(|s| {
                worktree::accept_publication(s, original, &publication);
                let m = &mut s.files.directory_snapshot;
                let Some(p) = m.pending.as_mut() else {
                    return;
                };
                p.phase = Phase::Settled;
                m.publication = Some(publication);
                m.observed = None;
                m.error = None;
                m.clear_review();
            });
            if let Err(error) = clear_locked(state, epoch, original, Phase::Settled, slot) {
                state.update(|s| s.files.directory_snapshot.error = Some(error));
            }
        }
        Err(error) => {
            if rejected_fresh {
                // Only this uninterrupted first attempt proves the local
                // original was not retried before its rejection. A failed
                // compare/remove cannot be carried as terminal evidence.
                if let Err(storage_error) =
                    clear_locked(state, epoch, original, Phase::Pending, slot)
                {
                    state.update(|s| {
                        let m = &mut s.files.directory_snapshot;
                        m.storage_ready = false;
                        m.error = Some(storage_error);
                    });
                }
            } else {
                state.update(|s| s.files.directory_snapshot.error = Some(error.clone()));
            }
            // Keep an authorization rejection scoped to the original identity,
            // even when a simultaneous local cleanup error must stay visible.
            fail_in(
                state,
                epoch,
                ErrorContext::Operation(original.command.operation().to_string()),
                error,
            );
        }
    }
}

#[cfg(test)]
#[path = "../../../../../../test/web/unit/files/directory_snapshot_delivery.rs"]
mod tests;
