//! Explicit first-history review over actual Unborn metadata. Browser recovery
//! retains only the exact original request; it never grants publication authority.
use super::{SaveStatus, actions};
use crate::{
    api,
    ui::{ErrorContext, State, current_session, fail_in},
};
use std::sync::Arc;
use storage_contract::Principal;
use storage_contract::file_draft::DraftLocation;
use web_interface::{prelude::*, tasks::spawn_local};
mod recovery;
mod storage;
mod view;
mod wire;
pub(crate) use recovery::restore;
use recovery::{Submission, dispatch};
use storage::Original;
pub(crate) use view::{recovery_view, view};
use wire::{Command, Record, invalid};
pub(in crate::files) use wire::Snapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    epoch: u64,
    navigation: u64,
    location: DraftLocation,
}
impl Target {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active()
            || state.files.directory_preview.is_some()
            || state.files.file_preview.is_some()
        {
            return None;
        }
        let entry = state.files.selected.as_ref()?;
        if entry.read_only() {
            return None;
        }
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
            self.location.file,
            self.location.repository,
            api::segment(self.location.path.as_str()),
            self.location.entry,
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Retaining,
    Pending,
    RetryQueued,
    LookingUp,
    Sending,
    Settled,
    Removing,
}
#[derive(Clone)]
struct Pending {
    original: Arc<Original>,
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
    fn changing_storage(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| {
            matches!(
                p.phase,
                Phase::Retaining | Phase::RetryQueued | Phase::Removing
            )
        })
    }
    fn owns(&self, state: &State) -> bool {
        state.session_active()
            && self.owner.as_ref().is_some_and(|(epoch, principal)| {
                *epoch == state.session_epoch && state.principal.as_ref() == Some(principal)
            })
    }
}
pub(crate) fn blocks_writes(state: &State) -> bool {
    let model = &state.files.unborn;
    model.owns(state) && (!model.storage_ready || model.pending.is_some())
}
fn can_review(state: &State) -> bool {
    state.files.unborn.owns(state)
        && state.files.unborn.storage_ready
        && actions::write_allowed(state)
        && Target::capture(state).is_some()
        && state.files.unborn.reading.is_none()
}
fn load(state: RwSignal<State>) {
    let target = state.with_untracked(|s| can_review(s).then(|| Target::capture(s)).flatten());
    let Some(target) = target else {
        return;
    };
    let generation = state.try_update(|s| {
        s.files.unborn.clear_review();
        s.files.unborn.error = None;
        s.files.unborn.reading = Some(target.clone());
        s.files.unborn.generation
    });
    let Some(generation) = generation else {
        return;
    };
    spawn_local(async move {
        let result = api::request("GET", &target.endpoint(), None)
            .await
            .and_then(|value| Snapshot::parse(value, &target.location));
        if !current_session(state, target.epoch) {
            return;
        }
        if let Err(error) = &result
            && error.status == 401
        {
            fail_in(
                state,
                target.epoch,
                ErrorContext::Read(target.endpoint()),
                error.clone(),
            );
            return;
        }
        state.update(|s| {
            if !target.current(s) || s.files.unborn.generation != generation {
                return;
            }
            s.files.unborn.reading = None;
            match result {
                Ok(snapshot) => s.files.unborn.review = Some((target, snapshot)),
                Err(error) => s.files.unborn.error = Some(error),
            }
        });
    });
}
fn can_submit(state: &State) -> bool {
    can_review(state)
        && state
            .files
            .unborn
            .review
            .as_ref()
            .is_some_and(|(target, _)| {
                target.current(state)
                    && storage_contract::validate_note(&state.files.unborn.message).is_ok()
            })
}
fn submit(state: RwSignal<State>) {
    let input = state.with_untracked(|s| {
        if !can_submit(s) {
            return None;
        }
        let (_, snapshot) = s.files.unborn.review.as_ref()?;
        Some((
            s.principal.clone()?,
            snapshot.clone(),
            s.files.unborn.message.clone(),
        ))
    });
    let Some((principal, snapshot, message)) = input else {
        return;
    };
    let Some(id) = crate::ui::file_operation_id(state) else {
        return;
    };
    let operation_id = id
        .parse()
        .expect("invariant: UI generates typed operation IDs");
    let record = Record::new(
        principal,
        Command {
            operation_id,
            target: snapshot.target,
            expected: snapshot.expected,
            message,
        },
    );
    let original = match record.and_then(Original::create) {
        Ok(original) => Arc::new(original),
        Err(error) => {
            state.update(|s| s.files.unborn.error = Some(error));
            return;
        }
    };
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| {
        s.files.unborn.pending = Some(Pending {
            original: Arc::clone(&original),
            phase: Phase::Retaining,
        });
    });
    dispatch(state, epoch, original, Submission::First);
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/unborn_tests.rs"]
mod tests;
