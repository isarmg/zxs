//! An opened status fences the occurrence, not the cleanliness of its ledger.
//! Commit proof supplies the candidate. Only the native writer can actually
//! decide the clean-batch, ownership and visibility conditions at mutation time.
use super::{Scope, available, wire};
use crate::{api, ui::State};
use storage_contract::{
    OperationId, Principal, RepositoryId,
    snapshot_worktree::{
        SnapshotHandle, WorktreeHandle, WorktreeRevision, WorktreeStatus,
        mount_change::{MountAction, MountChange, MountChangeReceipt, MountOutcome, RestoreNaming},
    },
};
use web_interface::prelude::*;

mod original;
mod view;
pub(super) use view::view;
use original::{Kind, Query, Reader};

const MAX_RECEIPT_BYTES: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Proof {
    scope: Scope,
    actor: Principal,
    target: WorktreeHandle,
    source: SnapshotHandle,
    expected: WorktreeRevision,
    operation: OperationId,
    committed: SnapshotHandle,
}

/// Untrusted user-supplied reference. It is *not* a receipt read as its author,
/// permission to restore, or a claim that this deletion is still current.
#[derive(Clone, Debug)]
struct KnownDeletion {
    snapshot_repository: storage_contract::working_tree::SnapshotRepositoryId,
    target: WorktreeHandle,
    expected: WorktreeRevision,
    deletion: OperationId,
}

impl KnownDeletion {
    fn parse(raw: &str) -> Result<Self, api::Error> {
        let value = storage_contract::strict_json(raw.as_bytes())
            .map_err(|e| wire::failure(wire::DecodeError::Contract(e)))?;
        let receipt = wire::mount::receipt(value)?;
        match (receipt.request.action, receipt.outcome) {
            (MountAction::Hide { committed, .. }, MountOutcome::Hidden {}) => Ok(Self {
                snapshot_repository: committed.repository_id,
                target: receipt.request.target,
                expected: receipt.request.expected,
                deletion: receipt.request.operation_id,
            }),
            _ => Err(wire::invalid("an exact completed Hidden receipt is required")),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Fields {
    scope: Option<Scope>,
    actor: Option<Principal>,
    operation: String,
    lookup: String,
    hidden: String,
    known: Option<KnownDeletion>,
    name: String,
    invalid_name: bool,
}

#[derive(Clone, Default)]
pub(super) struct Model {
    proof: Option<Proof>,
    seen: Option<(Query, MountChangeReceipt)>,
    fields: Fields,
    reading: bool,
    generation: u64,
    cancel: Option<api::ReadCancellation>,
}

impl Model {
    pub(super) fn cancel(&mut self) {
        if let Some(c) = self.cancel.take() { c.abort(); }
        self.generation += 1;
        self.reading = false;
    }
}

fn form(s: &State) -> Option<&Fields> {
    let f = &s.files.directory_snapshot.worktree.mount.fields;
    (s.files.directory_snapshot.owns(s) && f.actor == s.principal
        && f.scope.is_some() && f.scope == Scope::capture(s)).then_some(f)
}

#[derive(Clone, Copy)]
enum Field { Operation, Lookup, Hidden, Name }

fn edit(s: &mut State, field: Field, value: String) {
    let (Some(scope), Some(actor)) = (Scope::capture(s), s.principal.clone()) else { return; };
    let m = &mut s.files.directory_snapshot.worktree.mount;
    if m.fields.scope.as_ref() != Some(&scope) || m.fields.actor.as_ref() != Some(&actor) {
        m.cancel();
        m.fields = Fields { scope: Some(scope), actor: Some(actor), ..Default::default() };
    }
    // Never truncate pasted text into a different valid operation or name.
    let over = value.len() > MAX_RECEIPT_BYTES;
    let value = if over { String::new() } else { value };
    match field {
        Field::Operation => { m.cancel(); m.proof = None; m.fields.operation = value; }
        Field::Lookup => { m.cancel(); m.seen = None; m.fields.lookup = value; }
        Field::Hidden => {
            m.fields.known = KnownDeletion::parse(&value).ok();
            m.fields.hidden = value;
        }
        Field::Name => { m.fields.name = value; m.fields.invalid_name = over; }
    }
}

pub(super) fn accepted(s: &mut State, record: &wire::Record, value: &wire::Publication) {
    if !s.session_active() || s.principal.as_ref() != Some(&record.principal) { return; }
    let Some(scope) = Scope::capture(s) else { return; };
    match (&record.command, value) {
        (
            wire::Command::CommitWorktree { repository, source, request },
            wire::Publication::WorktreeCommitted { repository: reported, target, snapshot },
        ) if *repository == scope.repository && reported == repository && request.target == *target
            && source.repository_id == scope.snapshot_repository && snapshot.repository_id == source.repository_id => {
            s.files.directory_snapshot.worktree.mount.proof = Some(Proof {
                scope, actor: record.principal.clone(), target: *target, source: *source,
                expected: request.expected, operation: request.operation_id, committed: *snapshot,
            });
        }
        (
            wire::Command::MountChange { repository, request },
            wire::Publication::MountChanged { repository: reported, receipt },
        ) if reported == repository && *repository == scope.repository && &receipt.request == request => {
            let m = &mut s.files.directory_snapshot.worktree;
            if m.handle() == Some(request.target) {
                m.cancel_read();
                m.created = None;
                m.refresh_after_review = false;
            }
            m.instances.invalidate();
        }
        _ => {}
    }
}

fn hide_facts(s: &State) -> Option<(&Proof, &WorktreeStatus)> {
    if !available(s) { return None; }
    let m = &s.files.directory_snapshot.worktree;
    let observed = m.observed(s)?;
    let p = m.mount.proof.as_ref()?;
    (!m.mount.reading && s.principal.as_ref() == Some(&p.actor) && observed.scope == p.scope
        && observed.status.target == p.target && observed.status.original == p.source
        && observed.status.revision == p.expected).then_some((p, &observed.status))
}

fn hide(s: &State, operation_id: OperationId) -> Option<wire::Command> {
    let (p, status) = hide_facts(s)?;
    Some(wire::Command::MountChange {
        repository: p.scope.repository,
        request: MountChange {
            target: p.target, expected: p.expected, operation_id,
            action: MountAction::Hide { committed: p.committed, visible: status.visible },
        },
    })
}

fn restore_facts(s: &State) -> Option<(RepositoryId, &KnownDeletion, RestoreNaming)> {
    if !available(s) { return None; }
    let scope = Scope::capture(s)?;
    let f = form(s)?;
    let known = f.known.as_ref()?;
    if known.snapshot_repository != scope.snapshot_repository || f.invalid_name { return None; }
    let naming = if f.name.is_empty() { RestoreNaming::OriginalOrAutomatic {} }
        else { RestoreNaming::Explicit { name: name_policy::Name::parse(&f.name).ok()? } };
    Some((scope.repository, known, naming))
}

fn restore(s: &State, operation_id: OperationId) -> Option<wire::Command> {
    let (repository, known, naming) = restore_facts(s)?;
    Some(wire::Command::MountChange {
        repository,
        request: MountChange {
            target: known.target, expected: known.expected, operation_id,
            action: MountAction::Restore { deletion: known.deletion, naming },
        },
    })
}

#[cfg(test)]
#[path = "../../../../../../test/web/unit/files/worktree_clean_mount.rs"]
mod tests;
