//! A small management consumer of real worktree handles. Editing remains in
//! SMB; all mutations use the parent snapshot original-request journal.
mod requests;
mod view;

use super::{Target, can_review, wire};
use crate::{api, ui::State};
use name_policy::LookupName;
use storage_contract::RepositoryId;
use storage_contract::snapshot_worktree::{CreateWorktree, WorktreeHandle, WorktreeStatus};
use storage_contract::working_tree::{DirectoryId, SnapshotRepositoryId};
use web_interface::prelude::*;

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
    name: String,
    // Unparsed form text is never a lookup capability or mutation target.
    handle_text: String,
    publication_text: String,
    message: String,
    observed: Option<Observed>,
    created: Option<CreateWorktree>,
    reading: bool,
    generation: u64,
    cancellation: Option<api::ReadCancellation>,
    refresh_after_review: bool,
}

impl Model {
    pub(super) fn clear(&mut self) {
        self.cancel_read();
        *self = Self::default();
    }

    pub(super) fn invalidate(&mut self) {
        self.cancel_read();
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
        Some(WorktreeHandle {
            worktree_id: self.handle_text.parse().ok()?,
            publication: self.publication_text.parse().ok()?,
        })
    }

    fn observed<'a>(&'a self, state: &State) -> Option<&'a Observed> {
        self.observed.as_ref().filter(|observed| {
            !self.reading
                && observed.scope.current(state)
                && self.handle() == Some(observed.status.target)
        })
    }
}

fn available(state: &State) -> bool {
    can_review(state) && Scope::capture(state).is_some()
}

fn can_create(state: &State) -> bool {
    available(state)
        && !state.files.directory_snapshot.worktree.reading
        && name_policy::Name::parse(&state.files.directory_snapshot.worktree.name).is_ok()
}

fn can_load(state: &State) -> bool {
    available(state)
        && !state.files.directory_snapshot.worktree.reading
        && state.files.directory_snapshot.worktree.handle().is_some()
}

fn can_commit(state: &State) -> bool {
    if !available(state) {
        return false;
    }
    let model = &state.files.directory_snapshot;
    let Some(observed) = model.worktree.observed(state) else {
        return false;
    };
    let Some(review) = model.reviewed(state) else {
        return false;
    };
    // The production commit currently continues only a source still at HEAD.
    // This is a useful UI restriction, not permission to bypass the core CAS.
    wire::ready_source(&review.head) == Some(observed.status.original)
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
    created: Option<&CreateWorktree>,
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
    if created.is_some_and(|request| {
        status.original != request.source
            || status.parent_directory_id != request.parent_directory_id
            || status.name != LookupName::from(&request.name)
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

fn edit_handle(state: RwSignal<State>, publication: bool, value: String) {
    state.update(|state| {
        if !available(state) {
            return;
        }
        let model = &mut state.files.directory_snapshot.worktree;
        model.cancel_read();
        model.created = None;
        model.refresh_after_review = false;
        if publication {
            model.publication_text = value;
        } else {
            model.handle_text = value;
        }
    });
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
    let model = &mut state.files.directory_snapshot.worktree;
    model.cancel_read();
    model.handle_text = target.worktree_id.to_string();
    model.publication_text = target.publication.to_string();
    model.created = match &record.command {
        wire::Command::CreateWorktree { request, .. } => Some(request.clone()),
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
