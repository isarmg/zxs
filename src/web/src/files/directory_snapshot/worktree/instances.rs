//! Existing-instance selection is a read. Its bookmark never grants writer authority.
mod view;

use crate::api;
use crate::files::directory_snapshot::wire;
use crate::files::directory_snapshot::worktree::{Scope, available, bookmark, requests};
use crate::ui::{State, accept_read_response};
use storage_contract::RepositoryId;
use storage_contract::snapshot_worktree::instances::{
    DIRECTORY_WORKTREE_LIST_LIMIT, DirectoryWorktrees,
};
use storage_contract::snapshot_worktree::{SnapshotRootContext, WorktreeHandle};
use web_interface::{prelude::*, tasks::spawn_local};

pub(super) use view::view;

#[derive(Clone)]
struct Listing {
    scope: Scope,
    value: DirectoryWorktrees,
}

#[derive(Clone, Default)]
pub(super) struct Model {
    listing: Option<Listing>,
    reading: bool,
    generation: u64,
    cancellation: Option<api::ReadCancellation>,
}

impl Model {
    pub(super) fn invalidate(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.abort();
        }
        self.generation += 1;
        self.reading = false;
        self.listing = None;
    }

    fn listing<'a>(&'a self, state: &State) -> Option<&'a Listing> {
        self.listing.as_ref().filter(|listing| {
            !self.reading
                && listing.scope.current(state)
                && state
                    .files
                    .directory_snapshot
                    .reviewed(state)
                    .is_some_and(|review| review.head.context == listing.value.context)
        })
    }
}

fn can_read(state: &State) -> bool {
    available(state)
        && !state.files.directory_snapshot.worktree.reading
        && !state.files.directory_snapshot.worktree.instances.reading
}

struct Selection {
    scope: Scope,
    reference: bookmark::Reference,
}

impl Selection {
    fn capture(state: &State, target: WorktreeHandle) -> Option<Self> {
        if !can_read(state) {
            return None;
        }
        let listing = state
            .files
            .directory_snapshot
            .worktree
            .instances
            .listing(state)?;
        if !listing
            .value
            .instances
            .iter()
            .any(|status| status.target == target)
        {
            return None;
        }
        Some(Self {
            scope: listing.scope.clone(),
            reference: bookmark::Reference::new(
                state.principal.clone()?,
                listing.scope.repository,
                target,
            ),
        })
    }

    fn apply(self, state: &mut State) -> bool {
        if !self.scope.current(state)
            || !can_read(state)
            || !state
                .principal
                .as_ref()
                .is_some_and(|principal| self.reference.belongs(principal, self.scope.repository))
        {
            return false;
        }
        let model = &mut state.files.directory_snapshot.worktree;
        model.cancel_read();
        model.reference = Some(self.reference);
        model.bookmark_scope = Some(self.scope);
        model.created = None;
        model.refresh_after_review = false;
        model.message.clear();
        // No list revision becomes write authority. The existing exact GET
        // must admit this target before observed/status can enable a commit.
        true
    }
}

fn open(state: RwSignal<State>, target: WorktreeHandle) {
    let Some(selected) = state.with_untracked(|state| Selection::capture(state, target)) else {
        return;
    };
    if let Err(error) = bookmark::save(&selected.reference) {
        state.update(|state| state.files.directory_snapshot.error = Some(error));
        return;
    }
    if state.try_update(|state| selected.apply(state)) == Some(true) {
        requests::load(state);
    }
}

fn endpoint(repository: RepositoryId) -> String {
    format!("/api/repositories/{repository}/directory-worktrees")
}

fn admit(
    value: json_codec::Value,
    scope: &Scope,
    context: &SnapshotRootContext,
) -> Result<DirectoryWorktrees, api::Error> {
    let root = wire::object_field(&value, "context")?;
    wire::object_field(root, "host")?;
    wire::object_field(root, "source")?;
    let instances = value
        .get("instances")
        .and_then(json_codec::Value::as_array)
        .ok_or_else(|| wire::invalid("directory worktree instances must be an array"))?;
    if instances.len() > usize::from(DIRECTORY_WORKTREE_LIST_LIMIT.get()) {
        return Err(wire::invalid(
            "directory worktree instances exceed the complete-list budget",
        ));
    }
    for instance in instances {
        wire::object_field(instance, "target")?;
        wire::object_field(instance, "original")?;
        wire::object_field(instance, "revision")?;
    }
    let listing: DirectoryWorktrees = wire::decode(value)?;
    if listing.repository_id != scope.repository || &listing.context != context {
        return Err(wire::invalid(
            "directory worktree instances belong to another root context",
        ));
    }
    if listing.instances.iter().any(|status| {
        status.original.repository_id != scope.snapshot_repository
            || status.parent_directory_id != scope.parent
    }) {
        return Err(wire::invalid(
            "a directory worktree instance belongs to another source or parent",
        ));
    }
    Ok(listing)
}

fn load(state: RwSignal<State>) {
    let input = state.with_untracked(|state| {
        if !can_read(state) {
            return None;
        }
        Some((
            Scope::capture(state)?,
            state
                .files
                .directory_snapshot
                .reviewed(state)?
                .head
                .context
                .clone(),
        ))
    });
    let Some((scope, context)) = input else {
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
        let model = &mut state.files.directory_snapshot.worktree.instances;
        model.invalidate();
        model.reading = true;
        model.cancellation = Some(cancellation.clone());
        model.generation
    });
    let Some(generation) = generation else {
        return;
    };
    spawn_local(async move {
        let result = api::cancellable_get(&endpoint(scope.repository), cancellation)
            .await
            .and_then(|value| admit(value, &scope, &context));
        if !accept_read_response(state, scope.epoch, &result) {
            return;
        }
        let _ = state.try_update(|state| {
            if !scope.current(state)
                || state
                    .files
                    .directory_snapshot
                    .reviewed(state)
                    .is_none_or(|review| review.head.context != context)
            {
                return;
            }
            let model = &mut state.files.directory_snapshot.worktree.instances;
            if model.generation != generation {
                return;
            }
            model.reading = false;
            model.cancellation = None;
            match result {
                Ok(value) => {
                    model.listing = Some(Listing { scope, value });
                    state.files.directory_snapshot.error = None;
                }
                Err(error) => state.files.directory_snapshot.error = Some(error),
            }
        });
    });
}

#[cfg(test)]
#[path = "../../../../../../test/web/unit/files/directory_worktree_instances_tests.rs"]
mod tests;
