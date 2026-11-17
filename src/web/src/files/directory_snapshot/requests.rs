//! Bounded reads and a single frozen POST. Unknown outcomes retain the account,
//! source, exact selections and operation ID across navigation and browser reload.
use crate::api;
use crate::files::actions;
use crate::files::directory_snapshot::{
    BodyKind, Command, Pending, Phase, ReadItem, Record, Review, Target, can_initialize,
    can_register, can_review, can_submit, storage, wire,
};
use crate::ui::{ErrorContext, State, accept_read_response, current_session, fail_in};
use std::collections::BTreeSet;
use std::sync::Arc;
use storage_contract::snapshot_worktree::InitializeSnapshot;
use storage_contract::snapshot_worktree::draft_commit::CommitDirectoryDrafts;
use web_interface::{prelude::*, tasks::spawn_local};

pub(super) fn load(state: RwSignal<State>) {
    let input = state.with_untracked(|state| {
        if !can_review(state) {
            return None;
        }
        Some((Target::capture(state)?, Arc::clone(&state.files.entries)))
    });
    let Some((target, entries)) = input else {
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
        let model = &mut state.files.directory_snapshot;
        model.clear_review();
        model.error = None;
        model.reading = Some(target.clone());
        model.cancellation = Some(cancellation.clone());
        model.read_generation
    });
    let Some(generation) = generation else {
        return;
    };
    spawn_local(async move {
        let result = read(state, &target, &entries, cancellation).await;
        if !accept_read_response(state, target.epoch, &result) {
            return;
        }
        let _ = state.try_update(|state| {
            if !target.current(state)
                || state.files.directory_snapshot.read_generation != generation
            {
                return;
            }
            let model = &mut state.files.directory_snapshot;
            model.reading = None;
            model.cancellation = None;
            match result {
                Ok(review) => model.review = Some(review),
                Err(error) => model.error = Some(error),
            }
        });
        super::worktree::after_review(state);
    });
}

async fn read(
    state: RwSignal<State>,
    target: &Target,
    entries: &[crate::files::page::Entry],
    cancellation: api::ReadCancellation,
) -> Result<Review, api::Error> {
    let head = api::cancellable_get(&target.endpoint(), cancellation.clone())
        .await
        .and_then(|value| wire::head(value, target.repository))?;
    let mut candidates = Vec::new();
    if wire::ready_source(&head).is_some() {
        if entries.len() > wire::MAX_SELECTIONS {
            return Err(wire::invalid(
                "the listing exceeds this review's page budget",
            ));
        }
        // Sequential reads bound both browser and backend occupancy. The frozen
        // revisions are compared together by the later single directory POST.
        for entry in entries {
            if state.try_with_untracked(|state| target.current(state)) != Some(true) {
                return Err(wire::invalid("the directory changed during review"));
            }
            let Some(item) = ReadItem::from_page(entry, target)? else {
                continue;
            };
            let value = api::cancellable_get(&item.endpoint(), cancellation.clone()).await?;
            let candidate = match item.kind {
                BodyKind::Born => wire::born(value, item.target)?,
                BodyKind::Unborn => Some(wire::unborn(value, item.target)?),
            };
            if let Some(candidate) = candidate {
                candidates.push(candidate);
            }
        }
    }
    Ok(Review {
        target: target.clone(),
        head,
        candidates,
        selected: BTreeSet::new(),
    })
}

pub(super) fn register(state: RwSignal<State>) {
    let input = state.with_untracked(|state| {
        if !can_register(state) {
            return None;
        }
        Some((
            state.principal.clone()?,
            state.files.slot.parse::<name_policy::SlotId>().ok()?,
        ))
    });
    let Some((principal, slot)) = input else {
        return;
    };
    let operation_id = match api::operation_id() {
        Ok(operation) => operation,
        Err(error) => {
            state.update(|state| state.files.directory_snapshot.error = Some(error));
            return;
        }
    };
    retain_and_send(
        state,
        Record::new(principal, Command::RegisterRoot { slot, operation_id }),
    );
}

pub(super) fn initialize(state: RwSignal<State>) {
    let input = state.with_untracked(|state| {
        if !can_initialize(state) {
            return None;
        }
        let review = state.files.directory_snapshot.reviewed(state)?;
        Some((
            state.principal.clone()?,
            review.target.repository,
            review.head.context.clone(),
        ))
    });
    let Some((principal, repository, context)) = input else {
        return;
    };
    let operation_id = match api::operation_id() {
        Ok(operation) => operation,
        Err(error) => {
            state.update(|state| state.files.directory_snapshot.error = Some(error));
            return;
        }
    };
    retain_and_send(
        state,
        Record::new(
            principal,
            Command::Initialize {
                repository,
                request: InitializeSnapshot {
                    host: context.host,
                    source: context.source,
                    operation_id,
                },
            },
        ),
    );
}

pub(super) fn submit(state: RwSignal<State>) {
    let input = state.with_untracked(|state| {
        if !can_submit(state) {
            return None;
        }
        let model = &state.files.directory_snapshot;
        let review = model.reviewed(state)?;
        let selected = super::picks::collect(model.kept.as_ref(), state.principal.as_ref()?, review)
            .map(|items| items.into_iter().map(|item|item.selected.clone()).collect());
        Some((
            state.principal.clone()?,
            review.target.repository,
            wire::ready_source(&review.head)?,
            selected,
            model.message.clone(),
        ))
    });
    let Some((principal, repository, source, selected, message)) = input else {
        return;
    };
    let selected = match selected {
        Ok(items) => items,
        Err(error) => {
            state.update(|s| s.files.directory_snapshot.error=Some(error.error()));
            return;
        }
    };
    let operation_id = match api::operation_id() {
        Ok(operation) => operation,
        Err(error) => {
            state.update(|state| state.files.directory_snapshot.error = Some(error));
            return;
        }
    };
    retain_and_send(
        state,
        Record::new(
            principal,
            Command::Commit {
                repository,
                request: CommitDirectoryDrafts {
                    source,
                    selected,
                    message,
                    operation_id,
                },
            },
        ),
    );
}

pub(super) fn retain_and_send(state: RwSignal<State>, record: Result<Record, api::Error>) {
    let record = match record {
        Ok(record) => Arc::new(record),
        Err(error) => {
            state.update(|state| state.files.directory_snapshot.error = Some(error));
            return;
        }
    };
    if let Err(error) = storage::save(&record) {
        state.update(|state| {
            state.files.directory_snapshot.storage_ready = false;
            state.files.directory_snapshot.error = Some(error);
        });
        return;
    }
    state.update(|state| {
        let model = &mut state.files.directory_snapshot;
        model.pending = Some(Pending {
            record: Arc::clone(&record),
            phase: Phase::Pending,
        });
        model.retained_count = 1;
        if matches!(&record.command, Command::Commit { .. }) { model.kept = None; }
        model.error = None;
        model.publication = None;
    });
    send(state, record, true);
}

pub(super) fn retry_record(state: &State) -> Option<Arc<Record>> {
    let model = &state.files.directory_snapshot;
    if !model.owns(state) || state.files.mutation_pending {
        return None;
    }
    let pending = model.pending.as_ref()?;
    // Retained Named requests can only query a receipt. They need readable
    // storage, not write permission; other commands keep their existing gate.
    let available = match pending.record.command {
        Command::CreateWorktree { .. } => state.management.storage.readable(),
        Command::RegisterRoot { .. }
        | Command::Initialize { .. }
        | Command::Commit { .. }
        | Command::AllocateWorktree { .. }
        | Command::CommitWorktree { .. } => state.management.storage.writable(),
    };
    (pending.phase == Phase::Pending && available).then(|| Arc::clone(&pending.record))
}

pub(super) fn retry_post_record(state: &State) -> Option<Arc<Record>> {
    retry_record(state).filter(|record| record.command.can_post())
}

pub(super) fn retry(state: RwSignal<State>) {
    if let Some(record) = state.with_untracked(retry_post_record) {
        send(state, record, false);
    }
}

fn begin(state: RwSignal<State>, record: &Arc<Record>) -> Option<u64> {
    if let Err(error) = storage::verify(record) {
        state.update(|state| state.files.directory_snapshot.error = Some(error));
        return None;
    }
    state
        .try_update(|state| {
            let expected = retry_record(state)?;
            if !Arc::ptr_eq(&expected, record) {
                return None;
            }
            let pending = state.files.directory_snapshot.pending.as_mut()?;
            pending.phase = Phase::Sending;
            state.files.mutation_pending = true;
            Some(state.session_epoch)
        })
        .flatten()
}

fn release(state: RwSignal<State>, epoch: u64, record: &Arc<Record>) -> bool {
    if !current_session(state, epoch) {
        return false;
    }
    state.try_update(|state| {
        let Some(pending) = state.files.directory_snapshot.pending.as_mut() else {
            return false;
        };
        if !Arc::ptr_eq(&pending.record, record) {
            return false;
        }
        pending.phase = Phase::Pending;
        state.files.mutation_pending = false;
        true
    }) == Some(true)
}

fn send(state: RwSignal<State>, record: Arc<Record>, first: bool) {
    // A retained Named description proves only local intent, not a server
    // reservation. Reject its POST before begin; its GET and bytes remain usable.
    let body = match record.command.body() {
        Ok(body) => body,
        Err(error) => {
            state.update(|state| state.files.directory_snapshot.error = Some(error));
            return;
        }
    };
    let Some(epoch) = begin(state, &record) else {
        return;
    };
    spawn_local(async move {
        let result = api::request("POST", &record.command.endpoint(), Some(body)).await;
        if !current_session(state, epoch) {
            return;
        }
        let (result, fresh_rejection) = match result {
            Ok(value) if matches!(record.command, Command::Initialize { .. }) => {
                // The POST succeeded. A later GET rejection cannot establish
                // that initialization was rejected or release its original.
                (verify_initialization(&record, value).await, false)
            }
            other => (other, first),
        };
        if !release(state, epoch, &record) {
            return;
        }
        let (result, fresh_rejection) = match result {
            Ok(value) => (record.result(value), false),
            Err(error) => (Err(error), fresh_rejection),
        };
        finish(state, epoch, &record, result, fresh_rejection);
    });
}

async fn verify_initialization(
    record: &Record,
    value: json_codec::Value,
) -> Result<json_codec::Value, api::Error> {
    let Command::Initialize { repository, .. } = record.command else {
        return Err(wire::invalid(
            "head verification requires an initialization request",
        ));
    };
    let snapshot = wire::decode(value.clone())?;
    let endpoint = format!("/api/repositories/{repository}/snapshot-head");
    let head = api::request("GET", &endpoint, None)
        .await
        .and_then(|value| wire::head(value, repository))?;
    verify_initialization_head(snapshot, &head)?;
    Ok(value)
}

pub(super) fn verify_initialization_head(
    snapshot: storage_contract::snapshot_worktree::SnapshotHandle,
    head: &storage_contract::snapshot_worktree::discovery::DirectorySnapshotHead,
) -> Result<(), api::Error> {
    let Some(current) = wire::ready_source(head) else {
        return Err(wire::invalid(
            "initialization is not yet visible in the real head index",
        ));
    };
    // A later commit may already have advanced the node. The actual initial
    // branch and repository must still be those returned by this original POST.
    if current.repository_id != snapshot.repository_id || current.branch_id != snapshot.branch_id {
        return Err(wire::invalid(
            "initialization differs from the real registered branch",
        ));
    }
    Ok(())
}

pub(super) fn lookup(state: RwSignal<State>) {
    let Some(record) = state.with_untracked(retry_record) else {
        return;
    };
    let Some(endpoint) = record.command.outcome_endpoint() else {
        return;
    };
    let Some(epoch) = begin(state, &record) else {
        return;
    };
    spawn_local(async move {
        let result = api::request("GET", &endpoint, None)
            .await
            .and_then(|value| record.lookup_result(value));
        if !release(state, epoch, &record) {
            return;
        }
        // A missing read is not proof that the original POST did not publish.
        match result {
            Ok(wire::LookupResult::Pending) => {
                state.update(|state| state.files.directory_snapshot.error = None);
            }
            Ok(wire::LookupResult::Complete(publication)) => {
                finish(state, epoch, &record, Ok(publication), false);
            }
            Err(error) => finish(state, epoch, &record, Err(error), false),
        }
    });
}

fn finish(
    state: RwSignal<State>,
    epoch: u64,
    record: &Record,
    result: Result<wire::Publication, api::Error>,
    first: bool,
) {
    match result {
        Ok(publication) => {
            if let Err(error) = super::worktree::remember_publication(record, &publication) {
                // Publication is confirmed, but losing the only browser handle
                // reference is not successful cleanup. Keep the original so a
                // later read-only lookup can obtain and retain that same result.
                state.update(|state| {
                    state.files.directory_snapshot.publication = Some(publication);
                    state.files.directory_snapshot.error = Some(error);
                });
                return;
            }
            state.update(|state| {
                super::worktree::accept_publication(state, record, &publication);
                let model = &mut state.files.directory_snapshot;
                if let Some(pending) = model.pending.as_mut() {
                    pending.phase = Phase::Settled;
                }
                model.publication = Some(publication);
                model.error = None;
                model.clear_review();
            });
            cleanup(state);
        }
        Err(error) => {
            if first && !error.mutation_outcome_unknown() {
                state.update(|state| {
                    if let Some(pending) = state.files.directory_snapshot.pending.as_mut() {
                        pending.phase = Phase::Settled;
                    }
                    state.files.directory_snapshot.clear_review();
                });
                cleanup(state);
            }
            state.update(|state| state.files.directory_snapshot.error = Some(error.clone()));
            fail_in(
                state,
                epoch,
                ErrorContext::Operation(record.command.operation().to_string()),
                error,
            );
        }
    }
}

pub(super) fn cleanup(state: RwSignal<State>) {
    let record = state.with_untracked(|state| {
        let model = &state.files.directory_snapshot;
        if !model.owns(state) {
            return None;
        }
        model
            .pending
            .as_ref()
            .filter(|pending| pending.phase == Phase::Settled)
            .map(|pending| Arc::clone(&pending.record))
    });
    let Some(record) = record else {
        return;
    };
    match storage::remove(&record) {
        Ok(()) => {
            state.update(|state| {
                state.files.directory_snapshot.pending = None;
                state.files.directory_snapshot.storage_ready = true;
            });
            // Re-scan so another tab's independent original is not forgotten.
            restore(state);
            actions::refresh_directory(state);
        }
        Err(error) => state.update(|state| state.files.directory_snapshot.error = Some(error)),
    }
}

pub(crate) fn restore(state: RwSignal<State>) {
    let owner = state.with_untracked(|state| {
        if !state.session_active() || state.files.mutation_pending {
            return None;
        }
        Some((state.session_epoch, state.principal.clone()?))
    });
    let Some(owner) = owner else {
        return;
    };
    let result = storage::load(&owner.1);
    state.update(|state| {
        let model = &mut state.files.directory_snapshot;
        if model.owner.as_ref() != Some(&owner) {
            model.worktree.clear();
            model.kept=None;
        }
        model.owner = Some(owner);
        model.clear_review();
        match result {
            Ok(records) => {
                model.storage_ready = true;
                model.error = None;
                model.retained_count = records.len();
                model.pending = records.into_iter().next().map(|record| Pending {
                    record,
                    phase: Phase::Pending,
                });
            }
            Err(error) => {
                model.storage_ready = false;
                model.error = Some(error);
            }
        }
    });
}
