//! A small management consumer of real worktree handles. Editing remains in
//! SMB; all mutations use the parent snapshot original-request journal.
mod bookmark;
mod history;
mod instances;
mod requests;
mod view;

use super::{Target, can_review, wire};
use crate::{api, ui::State};
use storage_contract::RepositoryId;
use storage_contract::snapshot_worktree::creation::WorktreeCreationRequest;
use storage_contract::snapshot_worktree::{WorktreeHandle, WorktreeStatus};
use storage_contract::working_tree::{DirectoryId, SnapshotRepositoryId};

pub(super) use requests::after_review;
pub(super) use view::view;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Scope {
    epoch: u64,
    repository: RepositoryId,
    snapshot_repository: SnapshotRepositoryId,
    parent: DirectoryId,
}

impl Scope {
    fn capture(state: &State) -> Option<Self> {
        let review = state.files.directory_snapshot.reviewed(state)?;
        let source = wire::ready_source(&review.head)?;
        Some(Self {
            epoch: state.session_epoch,
            repository: review.target.repository,
            snapshot_repository: source.repository_id,
            parent: review.head.context.host.directory_id,
        })
    }

    fn current(&self, state: &State) -> bool {
        Self::capture(state).as_ref() == Some(self)
    }
}

#[derive(Clone)]
struct Observed {
    scope: Scope,
    status: WorktreeStatus,
}

#[derive(Clone, Default)]
pub(super) struct Model {
    history: history::Model,
    instances: instances::Model,
    reference: Option<bookmark::Reference>,
    bookmark_scope: Option<Scope>,
    message: String,
    observed: Option<Observed>,
    created: Option<WorktreeCreationRequest>,
    reading: bool,
    generation: u64,
    cancellation: Option<api::ReadCancellation>,
    refresh_after_review: bool,
}

impl Model {
    pub(super) fn clear(&mut self) {
        self.invalidate();
        *self = Self::default();
    }

    pub(super) fn invalidate(&mut self) {
        self.cancel_read();
        self.history.invalidate();
        self.instances.invalidate();
    }

    fn cancel_read(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.abort();
        }
        self.generation += 1;
        self.reading = false;
        self.observed = None;
    }

    fn handle(&self) -> Option<WorktreeHandle> {
        self.reference.as_ref().map(|reference| reference.target)
    }

    fn observed<'a>(&'a self, state: &State) -> Option<&'a Observed> {
        self.observed.as_ref().filter(|observed| {
            !self.reading
                && observed.scope.current(state)
                && self.reference.as_ref().is_some_and(|reference| {
                    state.principal.as_ref().is_some_and(|principal| {
                        reference.belongs(principal, observed.scope.repository)
                            && reference.target == observed.status.target
                    })
                })
        })
    }
}

fn available(state: &State) -> bool {
    can_review(state) && Scope::capture(state).is_some()
}

fn can_create(state: &State) -> bool {
    available(state)
        && !state.files.directory_snapshot.worktree.history.reading()
        && history::source(state).is_some()
        && !state.files.directory_snapshot.worktree.reading
        && state
            .files
            .directory_snapshot
            .worktree
            .bookmark_scope
            .as_ref()
            == Scope::capture(state).as_ref()
}

fn can_load(state: &State) -> bool {
    available(state)
        && !state.files.directory_snapshot.worktree.reading
        && state
            .files
            .directory_snapshot
            .worktree
            .reference
            .as_ref()
            .is_some_and(|reference| {
                state
                    .principal
                    .as_ref()
                    .zip(Scope::capture(state))
                    .is_some_and(|(principal, scope)| {
                        reference.belongs(principal, scope.repository)
                    })
            })
}

fn can_commit(state: &State) -> bool {
    if !available(state) {
        return false;
    }
    let model = &state.files.directory_snapshot;
    // The exact opened writer, not the newly selected creation source, owns
    // this commit. The core decides continue-versus-fork under its write lock.
    model.worktree.observed(state).is_some()
        && storage_contract::validate_note(&model.worktree.message).is_ok()
}

fn status_endpoint(target: WorktreeHandle) -> String {
    format!(
        "/api/directory-worktrees/{}?publication={}",
        target.worktree_id, target.publication,
    )
}

fn status(
    value: json_codec::Value,
    scope: &Scope,
    target: WorktreeHandle,
    created: Option<&WorktreeCreationRequest>,
) -> Result<WorktreeStatus, api::Error> {
    wire::object_field(&value, "target")?;
    wire::object_field(&value, "original")?;
    wire::object_field(&value, "revision")?;
    let status: WorktreeStatus = wire::decode(value)?;
    if status.target != target
        || status.original.repository_id != scope.snapshot_repository
        || status.parent_directory_id != scope.parent
    {
        return Err(wire::invalid(
            "the worktree status belongs to another instance or root",
        ));
    }
    if created.is_some_and(|request| match request {
        WorktreeCreationRequest::Named(request) => {
            status.original != request.source
                || status.parent_directory_id != request.parent_directory_id
                || status.name != name_policy::LookupName::from(&request.name)
        }
        WorktreeCreationRequest::Automatic(request) => {
            status.original != request.source
                || status.parent_directory_id != request.parent_directory_id
        }
    }) {
        return Err(wire::invalid(
            "the created worktree differs from its original request",
        ));
    }
    Ok(status)
}

fn smb_path(state: &State) -> Option<String> {
    let observed = state.files.directory_snapshot.worktree.observed(state)?;
    let disk = state
        .files
        .disks
        .iter()
        .find(|disk| disk.slot_id == state.files.slot)?;
    Some(format!("{}/{}", disk.name, observed.status.name.as_str()))
}

pub(super) fn remember_publication(
    record: &wire::Record,
    value: &wire::Publication,
) -> Result<(), api::Error> {
    match value {
        wire::Publication::WorktreeCreated { repository, target }
        | wire::Publication::WorktreeCommitted {
            repository, target, ..
        } => bookmark::save(&bookmark::Reference::new(
            record.principal.clone(),
            *repository,
            *target,
        )),
        wire::Publication::Registered(_)
        | wire::Publication::Initialized(_)
        | wire::Publication::Committed { .. } => Ok(()),
    }
}

pub(super) fn accept_publication(
    state: &mut State,
    record: &wire::Record,
    value: &wire::Publication,
) {
    let target = match value {
        wire::Publication::WorktreeCreated { repository, target }
        | wire::Publication::WorktreeCommitted {
            repository, target, ..
        } => {
            if Target::capture(state).is_none_or(|current| current.repository != *repository) {
                return;
            }
            *target
        }
        wire::Publication::Registered(_)
        | wire::Publication::Initialized(_)
        | wire::Publication::Committed { .. } => return,
    };
    let scope = Scope::capture(state);
    let Some(scope) = scope else {
        return;
    };
    let model = &mut state.files.directory_snapshot.worktree;
    model.cancel_read();
    model.reference = Some(bookmark::Reference::new(
        record.principal.clone(),
        scope.repository,
        target,
    ));
    model.bookmark_scope = Some(scope);
    model.created = match &record.command {
        wire::Command::CreateWorktree { request, .. } => {
            Some(WorktreeCreationRequest::Named(request.clone()))
        }
        wire::Command::AllocateWorktree { request, .. } => {
            Some(WorktreeCreationRequest::Automatic(request.clone()))
        }
        wire::Command::RegisterRoot { .. }
        | wire::Command::Initialize { .. }
        | wire::Command::Commit { .. }
        | wire::Command::CommitWorktree { .. } => None,
    };
    model.refresh_after_review = true;
    model.message.clear();
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/directory_worktree_tests.rs"]
mod tests;
