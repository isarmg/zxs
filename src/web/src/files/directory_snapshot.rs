//! Formal root snapshots and explicit draft selections. The browser stores only
//! the original descriptions; every write still enters the real HTTP authority.
mod requests;
mod storage;
mod view;
mod wire;
mod worktree;

use crate::api;
use crate::files::{actions, page::Entry};
use crate::ui::State;
use std::collections::BTreeSet;
use std::sync::Arc;
use storage_contract::file_draft::DraftLocation;
use storage_contract::snapshot_worktree::discovery::{
    DirectorySnapshotHead, SnapshotHeadDiscovery,
};
use storage_contract::{CommitId, EntryId, NamespaceEntryKind, Principal, RepositoryId};
use web_interface::prelude::*;
use wire::{Candidate, Command, Publication, Record};

pub(crate) use requests::restore;
pub(crate) use view::{recovery_view, view};
pub use wire::DecodeError;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Target {
    epoch: u64,
    navigation: u64,
    repository: RepositoryId,
    namespace_head: CommitId,
}

impl Target {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active()
            || !state.files.loaded
            || state.files.loading
            || !state.files.repository_path.is_empty()
            || state.files.selected.is_some()
            || state.files.directory_preview.is_some()
            || state.files.file_preview.is_some()
        {
            return None;
        }
        Some(Self {
            epoch: state.session_epoch,
            navigation: state.files.generation,
            repository: state.files.repository_id.parse().ok()?,
            namespace_head: state.files.namespace_head.parse().ok()?,
        })
    }

    fn current(&self, state: &State) -> bool {
        Self::capture(state).as_ref() == Some(self)
    }

    fn endpoint(&self) -> String {
        format!("/api/repositories/{}/snapshot-head", self.repository)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BodyKind {
    Born,
    Unborn,
}

struct ReadItem {
    target: DraftLocation,
    kind: BodyKind,
}

impl ReadItem {
    fn from_entry(entry: &Entry, repository: RepositoryId) -> Result<Option<Self>, api::Error> {
        if entry.kind() != NamespaceEntryKind::File || entry.read_only() {
            return Ok(None);
        }
        if let Some(target) = entry.unborn() {
            if target.repository != repository {
                return Err(wire::invalid("the listing contains another repository"));
            }
            return Ok(Some(Self {
                target: target.clone(),
                kind: BodyKind::Unborn,
            }));
        }
        let Some((owner, path)) = entry.context() else {
            return Err(wire::invalid(
                "a current file is missing its exact occurrence",
            ));
        };
        let owner: RepositoryId = owner
            .parse()
            .map_err(|source| wire::failure(DecodeError::Contract(source)))?;
        if owner != repository {
            return Err(wire::invalid("the listing contains another repository"));
        }
        Ok(Some(Self {
            target: DraftLocation {
                repository,
                path: name_policy::PortablePath::parse(path)
                    .map_err(|source| wire::failure(DecodeError::Path(source)))?,
                entry: entry
                    .entry_id()
                    .parse()
                    .map_err(|source| wire::failure(DecodeError::Contract(source)))?,
                file: entry
                    .file_id()
                    .parse()
                    .map_err(|source| wire::failure(DecodeError::Contract(source)))?,
            },
            kind: BodyKind::Born,
        }))
    }

    fn endpoint(&self) -> String {
        let suffix = match self.kind {
            BodyKind::Born => "working",
            BodyKind::Unborn => "unborn-working",
        };
        format!(
            "/api/file-manager/files/{}/{suffix}?repository_id={}&path={}&entry_id={}",
            self.target.file,
            self.target.repository,
            api::segment(self.target.path.as_str()),
            self.target.entry,
        )
    }
}

#[derive(Clone)]
struct Review {
    target: Target,
    head: DirectorySnapshotHead,
    candidates: Vec<Candidate>,
    selected: BTreeSet<EntryId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Pending,
    Sending,
    Settled,
}

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
    retained_count: usize,
    reading: Option<Target>,
    cancellation: Option<api::ReadCancellation>,
    read_generation: u64,
    review: Option<Review>,
    message: String,
    error: Option<api::Error>,
    publication: Option<Publication>,
    worktree: worktree::Model,
}

impl Model {
    pub(crate) fn clear_review(&mut self) {
        self.worktree.invalidate();
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.abort();
        }
        self.read_generation += 1;
        self.reading = None;
        self.review = None;
        self.message.clear();
    }

    fn owns(&self, state: &State) -> bool {
        state.session_active()
            && self.owner.as_ref().is_some_and(|(epoch, principal)| {
                *epoch == state.session_epoch && state.principal.as_ref() == Some(principal)
            })
    }

    fn reviewed(&self, state: &State) -> Option<&Review> {
        self.review.as_ref().filter(|review| {
            self.reading.is_none() && review.target.current(state) && self.owns(state)
        })
    }
}

pub(crate) fn blocks_writes(state: &State) -> bool {
    let model = &state.files.directory_snapshot;
    model.owns(state) && (!model.storage_ready || model.pending.is_some())
}

fn can_review(state: &State) -> bool {
    let model = &state.files.directory_snapshot;
    model.owns(state)
        && model.storage_ready
        && actions::write_allowed(state)
        && model.reading.is_none()
        && Target::capture(state).is_some()
}

fn can_register(state: &State) -> bool {
    let model = &state.files.directory_snapshot;
    model.owns(state)
        && model.storage_ready
        && actions::write_allowed(state)
        && state.files.loaded
        && state.files.selected.is_none()
        && !state.files.slot.is_empty()
        && state.files.slot.parse::<name_policy::SlotId>().is_ok()
        && state.files.directory.is_empty()
        && state.files.repository_id.is_empty()
        && state.files.entries.is_empty()
        && state.files.total_entries == 0
        && state.files.query.is_empty()
        && state.files.next_cursor.is_none()
        && state.files.previous_cursor.is_none()
}

fn can_initialize(state: &State) -> bool {
    let model = &state.files.directory_snapshot;
    if !can_review(state) {
        return false;
    }
    let Some(review) = model.reviewed(state) else {
        return false;
    };
    matches!(review.head.head, SnapshotHeadDiscovery::NotInitialized)
        && initialization_listing(state)
}

fn initialization_listing(state: &State) -> bool {
    state.files.query.is_empty()
        && state.files.next_cursor.is_none()
        && state.files.previous_cursor.is_none()
        && state.files.total_entries == state.files.entries.len() as u64
        && state.files.entries.iter().all(|entry| {
            entry.kind() == NamespaceEntryKind::File
                && entry.unborn().is_none()
                && !entry.read_only()
                && entry.version_id().is_some()
        })
}

fn can_submit(state: &State) -> bool {
    if !can_review(state) {
        return false;
    }
    let model = &state.files.directory_snapshot;
    let Some(review) = model.reviewed(state) else {
        return false;
    };
    wire::ready_source(&review.head).is_some()
        && !review.selected.is_empty()
        && storage_contract::validate_note(&model.message).is_ok()
}

fn toggle(state: RwSignal<State>, entry: EntryId, generation: u64, selected: bool) {
    state.update(|state| {
        if !can_review(state) || state.files.directory_snapshot.read_generation != generation {
            return;
        }
        let Some(review) = state.files.directory_snapshot.review.as_mut() else {
            return;
        };
        if !review
            .candidates
            .iter()
            .any(|item| item.selected.target.entry == entry)
        {
            return;
        }
        if selected {
            review.selected.insert(entry);
        } else {
            review.selected.remove(&entry);
        }
    });
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/directory_snapshot_tests.rs"]
mod tests;
