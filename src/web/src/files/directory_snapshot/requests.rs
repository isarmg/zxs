//! Bounded reads and a single frozen POST. Unknown outcomes retain the account,
//! source, exact selections and operation ID across navigation and browser reload.
mod delivery;
use crate::api;
use crate::files::directory_snapshot::{
    BodyKind, Command, Pending, Phase, ReadItem, Record, Review, Target, can_initialize,
    can_register, can_review, can_submit, storage, wire,
};
use crate::ui::{State, accept_read_response};
pub(super) use delivery::{
    cleanup, lookup, observe_restored, retain_and_send, retry, retry_post_record, retry_record,
};
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
        let selected =
            super::picks::collect(model.kept.as_ref(), state.principal.as_ref()?, review).map(
                |items| {
                    items
                        .into_iter()
                        .map(|item| item.selected.clone())
                        .collect()
                },
            );
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
            state.update(|s| s.files.directory_snapshot.error = Some(error.error()));
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

pub(crate) fn restore(state: RwSignal<State>) {
    let owner = state.with_untracked(|state| {
        if !state.session_active() || state.files.mutation_pending {
            return None;
        }
        let owner = (state.session_epoch, state.principal.clone()?);
        let m = &state.files.directory_snapshot;
        let preferred = (m.owner.as_ref() == Some(&owner))
            .then(|| {
                m.pending
                    .as_ref()
                    .map(|pending| pending.record.command.operation())
            })
            .flatten();
        Some((owner, preferred))
    });
    let Some((owner, preferred)) = owner else {
        return;
    };
    let result = storage::load(&owner.1, preferred);
    state.update(|state| {
        if state.session_active()
            && state.session_epoch == owner.0
            && state.principal.as_ref() == Some(&owner.1)
            && !state.files.mutation_pending
        {
            restored(&mut state.files.directory_snapshot, owner, result);
        }
    });
}

fn restored(
    model: &mut super::Model,
    owner: (u64, storage_contract::Principal),
    result: Result<storage::Inventory, api::Error>,
) {
    // Even an unreadable new account must not inherit the previous account's
    // pending operation, produced node, kept paths, message or worktree handle.
    // This drops only the view; it never removes another account's stored bytes.
    if model.owner.as_ref() != Some(&owner) {
        model.worktree.clear();
        model.kept = None;
        model.pending = None;
        model.retained_count = 0;
        model.publication = None;
        model.observed = None;
        model.observe_on_restore = None;
        model.error = None;
    }
    model.owner = Some(owner);
    model.clear_review();
    match result {
        Ok(records) => {
            model.retained_count = records.count();
            if let Some(pending) = &model.pending {
                if records.first.as_deref() == Some(pending.record.as_ref()) {
                    // Explicit re-observation is not a new operation. In particular,
                    // a known terminal receipt must never be demoted to a POST retry.
                    model.storage_ready = true;
                    model.error = None;
                    return;
                }
                let previous_absent = records.first.as_ref().is_none_or(|seen| {
                    seen.command.operation() != pending.record.command.operation()
                });
                if pending.phase != Phase::Settled || !previous_absent {
                    model.storage_ready = false;
                    model.error = Some(wire::invalid(
                        "The local original changed or is missing. The in-memory original is retained.",
                    ));
                    return;
                }
            }
            model.storage_ready = true;
            model.error = None;
            model.observed = None;
            model.observe_on_restore = records.first.as_ref().and_then(|r| {
                matches!(&r.command, Command::Commit { .. }).then(|| r.command.operation())
            });
            model.pending = records.first.map(|record| Pending {
                record,
                phase: Phase::Pending,
            });
        }
        Err(error) => {
            model.storage_ready = false;
            model.error = Some(error);
        }
    }
}
