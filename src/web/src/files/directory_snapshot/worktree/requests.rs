use super::{
    Observed, Scope, available, bookmark, can_commit, can_create, can_load, history, status,
    status_endpoint,
};
use crate::api;
use crate::files::directory_snapshot::{requests, wire};
use crate::ui::{State, accept_read_response};
use storage_contract::snapshot_worktree::CommitWorktree;
use storage_contract::snapshot_worktree::creation::{AllocateWorktree, WorktreeNaming};
use web_interface::{prelude::*, tasks::spawn_local};

pub(super) fn create_command(
    state: &State,
    operation_id: storage_contract::OperationId,
) -> Option<wire::Command> {
    if !can_create(state) {
        return None;
    }
    let model = &state.files.directory_snapshot;
    let review = model.reviewed(state)?;
    let (source, host) = history::source(state)?;
    Some(wire::Command::AllocateWorktree {
        repository: review.target.repository,
        request: AllocateWorktree {
            source,
            parent_directory_id: host.directory_id,
            naming: WorktreeNaming::Automatic,
            expected_root_generation: host.expected_root_generation,
            operation_id,
        },
    })
}

pub(super) fn commit_command(
    state: &State,
    operation_id: storage_contract::OperationId,
) -> Option<wire::Command> {
    if !can_commit(state) {
        return None;
    }
    let model = &state.files.directory_snapshot.worktree;
    let observed = model.observed(state)?;
    Some(wire::Command::CommitWorktree {
        repository: observed.scope.repository,
        source: observed.status.original,
        request: CommitWorktree {
            target: observed.status.target,
            expected: observed.status.revision,
            message: model.message.clone(),
            operation_id,
        },
    })
}

pub(super) fn create(state: RwSignal<State>) {
    if state.with_untracked(can_create) {
        submit(state, create_command);
    }
}

pub(super) fn commit(state: RwSignal<State>) {
    if state.with_untracked(can_commit) {
        submit(state, commit_command);
    }
}

fn submit(
    state: RwSignal<State>,
    capture: fn(&State, storage_contract::OperationId) -> Option<wire::Command>,
) {
    let operation = match api::operation_id() {
        Ok(operation) => operation,
        Err(error) => {
            state.update(|state| state.files.directory_snapshot.error = Some(error));
            return;
        }
    };
    let record = state.with_untracked(|state| {
        Some(wire::Record::new(
            state.principal.clone()?,
            capture(state, operation)?,
        ))
    });
    if let Some(record) = record {
        requests::retain_and_send(state, record);
    }
}

pub(in crate::files::directory_snapshot) fn after_review(state: RwSignal<State>) {
    restore_reference(state);
    if state.with_untracked(|state| {
        state.files.directory_snapshot.worktree.refresh_after_review && can_load(state)
    }) {
        load(state);
    }
}

fn restore_reference(state: RwSignal<State>) {
    let input = state.with_untracked(|state| {
        if !available(state) {
            return None;
        }
        let scope = Scope::capture(state)?;
        if state
            .files
            .directory_snapshot
            .worktree
            .bookmark_scope
            .as_ref()
            == Some(&scope)
        {
            return None;
        }
        Some((scope, state.principal.clone()?))
    });
    let Some((scope, principal)) = input else {
        return;
    };
    let result = bookmark::load(&principal, scope.repository);
    state.update(|state| {
        if !scope.current(state) || state.principal.as_ref() != Some(&principal) {
            return;
        }
        let model = &mut state.files.directory_snapshot.worktree;
        model.cancel_read();
        model.created = None;
        match result {
            Ok(reference) => {
                model.refresh_after_review = reference.is_some();
                model.reference = reference;
                model.bookmark_scope = Some(scope);
            }
            Err(error) => {
                model.reference = None;
                model.bookmark_scope = None;
                state.files.directory_snapshot.error = Some(error);
            }
        }
    });
}

pub(super) fn load(state: RwSignal<State>) {
    let input = state.with_untracked(|state| {
        if !can_load(state) {
            return None;
        }
        let model = &state.files.directory_snapshot.worktree;
        Some((
            Scope::capture(state)?,
            model.handle()?,
            model.created.clone(),
        ))
    });
    let Some((scope, target, created)) = input else {
        return;
    };
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancellation) => cancellation,
        Err(error) => {
            state.update(|state| state.files.directory_snapshot.error = Some(error));
            return;
        }
    };
    let generation = state.try_update(|state| {
        let model = &mut state.files.directory_snapshot.worktree;
        model.cancel_read();
        model.reading = true;
        model.refresh_after_review = false;
        model.cancellation = Some(cancellation.clone());
        model.generation
    });
    let Some(generation) = generation else {
        return;
    };
    spawn_local(async move {
        let result = api::cancellable_get(&status_endpoint(target), cancellation)
            .await
            .and_then(|value| status(value, &scope, target, created.as_ref()));
        if !accept_read_response(state, scope.epoch, &result) {
            return;
        }
        let _ = state.try_update(|state| {
            if !scope.current(state) {
                return;
            }
            let model = &mut state.files.directory_snapshot.worktree;
            if model.generation != generation || model.handle() != Some(target) {
                return;
            }
            model.reading = false;
            model.cancellation = None;
            match result {
                Ok(status) => {
                    model.observed = Some(Observed { scope, status });
                    state.files.directory_snapshot.error = None;
                }
                Err(error) => state.files.directory_snapshot.error = Some(error),
            }
        });
    });
}
