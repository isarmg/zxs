//! Explicit per-file mode changes over the same shared-working HTTP authority.
use crate::api;
use crate::files::{SaveStatus, actions};
use crate::ui::{ErrorContext, State, current_session, fail_in};
use json_codec::Value;
use std::sync::Arc;
use storage_contract::file_draft::{CommitMode, DraftCommand, DraftLocation};
use storage_contract::{BranchId, FileNodeId, OperationId, Principal, VersionId};
use web_interface::{prelude::*, tasks::spawn_local};

mod view;
mod wire;
pub(crate) use view::view;
pub use wire::DecodeError;
use wire::Snapshot;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Target {
    epoch: u64,
    navigation: u64,
    location: DraftLocation,
    branch: BranchId,
    node: FileNodeId,
    version: VersionId,
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
        if entry.read_only() || entry.backup() || entry.unborn().is_some() {
            return None;
        }
        Some(Self {
            epoch: state.session_epoch,
            navigation: state.files.generation,
            location: DraftLocation {
                repository: entry.repository_id()?.parse().ok()?,
                path: name_policy::PortablePath::parse(entry.repository_path()?).ok()?,
                entry: entry.entry_id().parse().ok()?,
                file: entry.file_id().parse().ok()?,
            },
            branch: entry.selected_branch_id()?.parse().ok()?,
            node: entry.node_id()?.parse().ok()?,
            version: entry.version_id()?.parse().ok()?,
        })
    }

    fn current(&self, state: &State) -> bool {
        Self::capture(state).as_ref() == Some(self)
    }

    fn endpoint(&self) -> String {
        format!(
            "/api/file-manager/files/{}/working?repository_id={}&path={}&entry_id={}",
            self.location.file,
            self.location.repository,
            api::segment(self.location.path.as_str()),
            self.location.entry,
        )
    }
}

#[derive(Clone)]
struct Review {
    target: Target,
    snapshot: Snapshot,
}

#[derive(Clone)]
struct Pending {
    epoch: u64,
    principal: Principal,
    operation: OperationId,
    command: DraftCommand,
}

#[derive(Clone, Default)]
pub(crate) struct Model {
    generation: u64,
    reading: Option<Target>,
    review: Option<Review>,
    pending: Option<Arc<Pending>>,
    error: Option<api::Error>,
}

impl Model {
    pub(crate) fn clear_review(&mut self) {
        self.generation += 1;
        self.reading = None;
        self.review = None;
        self.error = None;
    }

    fn reviewed(&self, state: &State) -> Option<&Review> {
        self.review
            .as_ref()
            .filter(|review| self.reading.is_none() && review.target.current(state))
    }
}

fn can_load(state: &State) -> bool {
    Target::capture(state).is_some()
        && !state.files.loading
        && state
            .files
            .commit_mode
            .reading
            .as_ref()
            .is_none_or(|target| !target.current(state))
        && !state.files.mutation_pending
}

fn load(state: RwSignal<State>) {
    let target = state.with_untracked(|s| can_load(s).then(|| Target::capture(s)).flatten());
    let Some(target) = target else {
        return;
    };
    let generation = state.try_update(|s| {
        s.files.commit_mode.clear_review();
        s.files.commit_mode.reading = Some(target.clone());
        s.files.commit_mode.generation
    });
    let Some(generation) = generation else {
        return;
    };
    spawn_local(async move {
        let result = api::request("GET", &target.endpoint(), None)
            .await
            .and_then(|value| Snapshot::parse(value, &target));
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
        state.update(|s| complete_load(s, target, generation, result));
    });
}

fn complete_load(
    state: &mut State,
    target: Target,
    generation: u64,
    result: Result<Snapshot, api::Error>,
) {
    if !target.current(state) || state.files.commit_mode.generation != generation {
        return;
    }
    let model = &mut state.files.commit_mode;
    model.reading = None;
    match result {
        Ok(snapshot) => model.review = Some(Review { target, snapshot }),
        Err(error) => model.error = Some(error),
    }
}

fn can_change(state: &State, mode: CommitMode) -> bool {
    actions::write_allowed(state)
        && state.files.commit_mode.pending.is_none()
        && state
            .files
            .commit_mode
            .reviewed(state)
            .is_some_and(|review| {
                review.snapshot.working.mode() != mode && review.snapshot.working.draft().is_none()
            })
}

fn begin(state: &mut State, mode: CommitMode, operation: OperationId) -> Option<Arc<Pending>> {
    if !can_change(state, mode) {
        return None;
    }
    let review = state.files.commit_mode.reviewed(state)?;
    let pending = Arc::new(Pending {
        epoch: state.session_epoch,
        principal: state.principal.clone()?,
        operation,
        command: DraftCommand::SetMode {
            target: review.target.location.clone(),
            expected: review.snapshot.working.revision(),
            mode,
        },
    });
    state.files.commit_mode.pending = Some(Arc::clone(&pending));
    state.files.commit_mode.error = None;
    state.files.mutation_pending = true;
    state.notices.begin_operation(operation.to_string());
    Some(pending)
}

fn submit(state: RwSignal<State>, mode: CommitMode) {
    if !state.with_untracked(|s| can_change(s, mode)) {
        return;
    }
    let operation = match api::operation_id() {
        Ok(operation) => operation,
        Err(error) => {
            crate::ui::fail(state, error);
            return;
        }
    };
    let Some(pending) = state.try_update(|s| begin(s, mode, operation)).flatten() else {
        return;
    };
    spawn_local(async move {
        let result = match pending.body() {
            Ok(body) => api::request("POST", &pending.endpoint(), Some(body)).await,
            Err(error) => Err(error),
        };
        if !current_session(state, pending.epoch) {
            return;
        }
        if !state.with_untracked(|s| {
            s.files
                .commit_mode
                .pending
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &pending))
        }) {
            return;
        }
        state.update(|s| s.files.mutation_pending = false);
        let id = pending.operation.to_string();
        match result {
            Ok(value) => {
                actions::record_receipt(state, &id, value);
            }
            Err(error) => {
                actions::record_failure(state, pending.epoch, &id, error.clone());
                state.update(|s| s.files.commit_mode.error = Some(error));
            }
        }
    });
}

pub(crate) fn receipt_matches(state: &State, id: &str, value: &Value) -> bool {
    let Some(pending) = state
        .files
        .commit_mode
        .pending
        .as_ref()
        .filter(|pending| pending.operation.to_string() == id)
    else {
        return true;
    };
    pending.epoch == state.session_epoch
        && state.principal.as_ref() == Some(&pending.principal)
        && pending.receipt_matches(value).unwrap_or(false)
}

pub(crate) fn settle(model: &mut Model, id: &str, status: SaveStatus) {
    if status != SaveStatus::Unknown
        && model
            .pending
            .as_ref()
            .is_some_and(|pending| pending.operation.to_string() == id)
    {
        model.pending = None;
        model.clear_review();
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/commit_mode_tests.rs"]
mod tests;
