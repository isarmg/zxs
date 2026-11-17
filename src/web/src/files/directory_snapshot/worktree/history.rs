//! Bounded real-node discovery keeps selection separate from the opened writer.
//! An explicit historical selection never falls back to a newer HEAD.
mod view;

use crate::api;
use crate::files::directory_snapshot::wire;
use crate::files::directory_snapshot::worktree::{Scope, available};
use crate::ui::{State, accept_read_response};
use storage_contract::RepositoryId;
use storage_contract::snapshot_worktree::history::{
    DirectorySnapshotHistory, SNAPSHOT_HISTORY_PAGE_SIZE, SnapshotHistoryCursor,
    SnapshotHistoryNode,
};
use storage_contract::snapshot_worktree::{SnapshotHandle, SnapshotHost, SnapshotRootContext};
use web_interface::{prelude::*, tasks::spawn_local};

pub(super) use view::view;

#[derive(Clone)]
struct Page {
    scope: Scope,
    value: DirectorySnapshotHistory,
}

#[derive(Clone)]
struct Selection {
    scope: Scope,
    context: SnapshotRootContext,
    node: SnapshotHistoryNode,
}

#[derive(Clone, Default)]
pub(super) struct Model {
    page: Option<Page>,
    selected: Option<Selection>,
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
        self.page = None;
        // Keep the exact selection visible but unusable if its scope changed.
        // Clearing it here would silently turn a historical choice into HEAD.
    }

    pub(super) fn reading(&self) -> bool {
        self.reading
    }

    fn page<'a>(&'a self, state: &State) -> Option<&'a Page> {
        self.page
            .as_ref()
            .filter(|page| !self.reading && page.scope.current(state))
    }
}

pub(super) fn source(state: &State) -> Option<(SnapshotHandle, SnapshotHost)> {
    let review = state.files.directory_snapshot.reviewed(state)?;
    match &state.files.directory_snapshot.worktree.history.selected {
        Some(selected) => {
            if !selected.scope.current(state)
                || selected.context.host != review.head.context.host
                || selected.context.source.branch_id != review.head.context.source.branch_id
            {
                return None;
            }
            Some((selected.node.snapshot, selected.context.host))
        }
        None => Some((wire::ready_source(&review.head)?, review.head.context.host)),
    }
}

fn can_read(state: &State) -> bool {
    available(state) && !state.files.directory_snapshot.worktree.history.reading
}

fn select(state: &mut State, target: SnapshotHandle) {
    if !can_read(state) {
        return;
    }
    let model = &state.files.directory_snapshot.worktree.history;
    let Some(page) = model.page(state) else {
        return;
    };
    let Some(node) = page.value.nodes.iter().find(|node| node.snapshot == target) else {
        return;
    };
    let selected = Selection {
        scope: page.scope.clone(),
        context: page.value.context.clone(),
        node: node.clone(),
    };
    state.files.directory_snapshot.worktree.history.selected = Some(selected);
}

fn select_head(state: &mut State) {
    if can_read(state) {
        state.files.directory_snapshot.worktree.history.selected = None;
    }
}

fn endpoint(repository: RepositoryId, cursor: Option<SnapshotHistoryCursor>) -> String {
    let route = format!("/api/repositories/{repository}/snapshot-history");
    match cursor {
        None => route,
        Some(cursor) => format!(
            "{route}?sequence={}&publication={}&repository_id={}&branch_id={}&node_id={}",
            cursor.sequence,
            cursor.after.publication,
            cursor.after.repository_id,
            cursor.after.branch_id,
            cursor.after.node_id,
        ),
    }
}

fn admit(
    value: json_codec::Value,
    scope: &Scope,
    context: &SnapshotRootContext,
    cursor: Option<SnapshotHistoryCursor>,
) -> Result<DirectorySnapshotHistory, api::Error> {
    let root = wire::object_field(&value, "context")?;
    wire::object_field(root, "host")?;
    wire::object_field(root, "source")?;
    let nodes = value
        .get("nodes")
        .and_then(json_codec::Value::as_array)
        .ok_or_else(|| wire::invalid("snapshot history nodes must be an array"))?;
    if nodes.len() > SNAPSHOT_HISTORY_PAGE_SIZE {
        return Err(wire::invalid("snapshot history exceeds the page budget"));
    }
    for node in nodes {
        wire::object_field(node, "snapshot")?;
        let parent = node
            .get("parent")
            .ok_or_else(|| wire::invalid("snapshot history must state each parent"))?;
        if !parent.is_null() {
            wire::object_field(node, "parent")?;
        }
    }
    let next = value
        .get("next_cursor")
        .ok_or_else(|| wire::invalid("snapshot history must state its next cursor"))?;
    if !next.is_null() {
        wire::object_field(wire::object_field(&value, "next_cursor")?, "after")?;
    }
    let history: DirectorySnapshotHistory = wire::decode(value)?;
    if history.repository_id != scope.repository || &history.context != context {
        return Err(wire::invalid(
            "snapshot history belongs to another root context",
        ));
    }
    if history.nodes.iter().any(|node| {
        node.snapshot.repository_id != scope.snapshot_repository
            || node
                .parent
                .is_some_and(|parent| parent.repository_id != scope.snapshot_repository)
    }) {
        return Err(wire::invalid(
            "snapshot history contains another snapshot repository",
        ));
    }
    if let Some(next) = history.next_cursor {
        if history.nodes.last().map(|node| node.snapshot) != Some(next.after)
            || cursor.is_some_and(|cursor| cursor.sequence != next.sequence)
        {
            return Err(wire::invalid(
                "snapshot history cursor differs from this page",
            ));
        }
    }
    Ok(history)
}

fn next_cursor(state: &State) -> Option<SnapshotHistoryCursor> {
    if !can_read(state) {
        return None;
    }
    state
        .files
        .directory_snapshot
        .worktree
        .history
        .page(state)?
        .value
        .next_cursor
}

fn next(state: RwSignal<State>) {
    if let Some(cursor) = state.with_untracked(next_cursor) {
        load(state, Some(cursor));
    }
}

fn load(state: RwSignal<State>, cursor: Option<SnapshotHistoryCursor>) {
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
        let model = &mut state.files.directory_snapshot.worktree.history;
        model.invalidate();
        model.reading = true;
        model.cancellation = Some(cancellation.clone());
        model.generation
    });
    let Some(generation) = generation else {
        return;
    };
    spawn_local(async move {
        let result = api::cancellable_get(&endpoint(scope.repository, cursor), cancellation)
            .await
            .and_then(|value| admit(value, &scope, &context, cursor));
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
            let model = &mut state.files.directory_snapshot.worktree.history;
            if model.generation != generation {
                return;
            }
            model.reading = false;
            model.cancellation = None;
            match result {
                Ok(value) => {
                    model.page = Some(Page { scope, value });
                    state.files.directory_snapshot.error = None;
                }
                Err(error) => state.files.directory_snapshot.error = Some(error),
            }
        });
    });
}

#[cfg(test)]
#[path = "../../../../../../test/web/unit/files/directory_worktree_history_tests.rs"]
mod tests;
