//! Directory lifecycle controls use the existing original-request deletion ledger.
//! Reads establish a review, never a new deletion identity or write authority.
use super::*;
use crate::files::{model::entry_key, page::DirectoryBranch, page::Entry};
use json_codec::Value;
use std::collections::BTreeSet;
use storage_contract::directory_git::{
    DirectoryDeletionNode, DirectoryDeletionSummary, DirectoryDeletionsPage, DirectoryGitStatus,
};
use storage_contract::history_window::HistoryRepositoryId;

mod request;
mod view;
pub(super) mod wire;
pub(super) use request::{Request, Stored, stored};
pub(super) use view::view;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Location {
    epoch: u64,
    generation: u64,
    slot: String,
    directory: String,
    repository: String,
    path: String,
    namespace_head: String,
}
impl Location {
    fn capture(state: &State) -> Option<Self> {
        (state.session_active()
            && state.files.loaded
            && !state.files.loading
            && !state.files.repository_id.is_empty()
            && state.files.directory_preview.is_none()
            && state.files.file_preview.is_none())
        .then(|| Self {
            epoch: state.session_epoch,
            generation: state.files.generation,
            slot: state.files.slot.clone(),
            directory: state.files.directory.clone(),
            repository: state.files.repository_id.clone(),
            path: state.files.repository_path.clone(),
            namespace_head: state.files.namespace_head.clone(),
        })
    }
    fn current(&self, state: &State) -> bool {
        Self::capture(state).as_ref() == Some(self)
    }
}
#[derive(Clone)]
struct Selection {
    location: Location,
    entry: String,
    path: name_policy::PortablePath,
    display_path: String,
    branch: String,
    tree: String,
    checked: BTreeSet<String>,
    selected: Option<String>,
}
impl Selection {
    fn current(&self, state: &State) -> bool {
        self.location.current(state)
            && actions::write_allowed(state)
            && self.checked == state.files.checked_entries
            && self.selected == state.files.selected.as_ref().map(entry_key)
            && current_branch(state).is_some_and(|branch| {
                branch.id() == self.branch && branch.head_tree_id() == self.tree
            })
            && state
                .files
                .entries
                .iter()
                .chain(state.files.selected.iter())
                .any(|entry| {
                    eligible(entry)
                        && entry.entry_id() == self.entry
                        && entry.path() == self.display_path
                        && entry.context()
                            == Some((self.location.repository.as_str(), self.path.as_str()))
                })
    }
}
#[derive(Clone)]
struct Review {
    generation: u64,
    selection: Selection,
    request: Arc<Request>,
}
#[derive(Clone)]
struct Scope {
    location: Location,
    owner: String,
    owner_path: String,
}
impl Scope {
    fn endpoint(&self) -> String {
        format!(
            "/api/repositories/{}/directory-deletions/{}",
            api::segment(&self.location.repository),
            api::segment(&self.owner)
        )
    }
    fn node_endpoint(&self, directory: &str) -> String {
        format!("{}/{}", self.endpoint(), api::segment(directory))
    }
}
#[derive(Clone)]
struct RestoreReview {
    generation: u64,
    node: Arc<DirectoryDeletionNode>,
}
#[derive(Clone, Default)]
pub(super) struct Model {
    review: Option<Review>,
    review_loading: bool,
    open: bool,
    generation: u64,
    loading: bool,
    scope: Option<Scope>,
    page: Option<Arc<DirectoryDeletionsPage>>,
    selected: Option<DirectoryDeletionSummary>,
    node: Option<Arc<DirectoryDeletionNode>>,
    restore_review: Option<RestoreReview>,
}
impl Model {
    pub(super) fn cancel_review(&mut self) {
        self.review = None;
        self.review_loading = false;
        self.restore_review = None;
        self.generation += 1;
        self.loading = false;
    }
    pub(super) fn invalidate(&mut self) {
        self.cancel_review();
        self.loading = false;
        self.page = None;
        self.selected = None;
        self.node = None;
    }
    pub(super) fn close(&mut self) {
        self.invalidate();
        self.open = false;
        self.scope = None;
    }
    fn accepts(&self, generation: u64) -> bool {
        self.open && self.generation == generation
    }
}
fn invalid(message: &str) -> api::Error {
    api::error(storage_contract::ErrorCode::InvalidResponse, message)
}
fn identity_error(source: storage_contract::Error) -> api::Error {
    let mut error = invalid("The directory identity could not be verified.");
    error.source = Some(Arc::new(api::ErrorSource::FileDeletionRecovery(source)));
    error
}
fn directory_id(id: &str, repository: &str, path: &str) -> bool {
    if path.is_empty() {
        id == repository
    } else {
        storage_contract::validate_id(id, "entry").is_ok()
    }
}
fn ancestor(parent: &str, child: &str) -> bool {
    let parent = name_policy::path_key(parent);
    let child = name_policy::path_key(child);
    !child.is_empty()
        && (parent.is_empty()
            || child
                .strip_prefix(&parent)
                .is_some_and(|tail| tail.starts_with('/')))
}
fn status(value: Value, repository: &str, path: &str) -> Result<DirectoryGitStatus, api::Error> {
    if !value.is_object()
        || !(value["owner"].is_null() || value["owner"].is_object())
        || value["children"]
            .as_array()
            .is_none_or(|children| children.iter().any(|child| !child.is_object()))
    {
        return Err(invalid(
            "Directory status must contain named identity objects.",
        ));
    }
    let status: DirectoryGitStatus =
        json_codec::from_value(value).map_err(|source| api::Error {
            status: 200,
            code: storage_contract::ErrorCode::InvalidResponse,
            message: "The directory repository response did not match its contract.".into(),
            retryable: false,
            request_id: None,
            source: Some(Arc::new(api::ErrorSource::DirectoryGit(source))),
        })?;
    if status.repository_id != repository
        || status.path != path
        || !directory_id(&status.directory_id, repository, path)
        || storage_contract::validate_id(repository, "repo").is_err()
        || (!path.is_empty() && name_policy::PortablePath::parse(path).is_err())
        || status.owner.as_ref().is_some_and(|owner| {
            !directory_id(&owner.directory_id, repository, &owner.path)
                || (!owner.path.is_empty()
                    && name_policy::PortablePath::parse(&owner.path).is_err())
        })
    {
        return Err(invalid("The directory status belongs to another location."));
    }
    if status.initialized {
        if !status.owner.as_ref().is_some_and(|owner| {
            owner.directory_id == status.directory_id && owner.path == status.path
        }) || !status
            .directory_branch_id
            .as_ref()
            .is_some_and(|id| storage_contract::validate_id(id, "dbr").is_ok())
            || !status
                .working_tree_id
                .as_ref()
                .is_some_and(|id| storage_contract::validate_id(id, "dtree").is_ok())
            || status
                .head_tree_id
                .as_ref()
                .is_some_and(|id| storage_contract::validate_id(id, "dtree").is_err())
            || status.index_generation == 0
            || (status.head_tree_id.is_none() && !status.staged)
        {
            return Err(invalid("The initialized directory status is inconsistent."));
        }
    } else if status.directory_branch_id.is_some()
        || status.working_tree_id.is_some()
        || status.head_tree_id.is_some()
        || status.index_generation != 0
        || status.staged
        || status.unstaged
        || status.needs_restage
        || status.added != 0
        || status.modified != 0
        || status.deleted != 0
        || !status.children.is_empty()
        || status.owner.as_ref().is_some_and(|owner| {
            owner.directory_id == status.directory_id || !ancestor(&owner.path, path)
        })
    {
        return Err(invalid("The ordinary directory status is inconsistent."));
    }
    Ok(status)
}
async fn read_status(repository: &str, path: &str) -> Result<DirectoryGitStatus, api::Error> {
    let endpoint = format!(
        "/api/repositories/{}/directory-git?path={}",
        api::segment(repository),
        api::segment(path)
    );
    api::validated_request("GET", &endpoint, None, |value| {
        status(value, repository, path)
    })
    .await
}
fn current_branch(state: &State) -> Option<&DirectoryBranch> {
    if !actions::write_allowed(state)
        || !state.files.loaded
        || !state.files.directory_branches_supported
    {
        return None;
    }
    let mut branches = state
        .files
        .directory_branches
        .iter()
        .filter(|branch| branch.current());
    let branch = branches.next()?;
    (branches.next().is_none()
        && branch.repository_id() == state.files.repository_id
        && branch.path() == state.files.repository_path)
        .then_some(branch)
}
fn eligible(entry: &Entry) -> bool {
    entry.kind() == storage_contract::NamespaceEntryKind::Directory
        && !entry.read_only()
        && !entry.preview()
        && !entry.backup()
        && !entry.retained_history()
        && entry.context().is_some()
}
fn capture(state: &State, explicit: Option<&Entry>) -> Result<Option<Selection>, api::Error> {
    let Some(location) = Location::capture(state) else {
        return Ok(None);
    };
    let Some(branch) = current_branch(state) else {
        return Ok(None);
    };
    let entry = match explicit {
        Some(entry) => Some(entry),
        None if state.files.checked_entries.is_empty() => state.files.selected.as_ref(),
        None if state.files.checked_entries.len() == 1 => state
            .files
            .entries
            .iter()
            .find(|entry| state.files.checked_entries.contains(&entry_key(entry))),
        None => None,
    };
    let Some(entry) = entry.filter(|entry| eligible(entry)) else {
        return Ok(None);
    };
    let Some((repository, path)) = entry.context() else {
        return Ok(None);
    };
    if repository != location.repository
        || path.rsplit_once('/').map_or("", |(parent, _)| parent) != location.path
    {
        return Ok(None);
    }
    let selection = Selection {
        location,
        entry: entry.entry_id().into(),
        path: name_policy::PortablePath::parse(path)
            .map_err(|source| identity_error(source.into()))?,
        display_path: entry.path().into(),
        branch: branch.id().into(),
        // This is the branch CAS head from the listing, not Current's working tree.
        tree: branch.head_tree_id().into(),
        checked: state.files.checked_entries.clone(),
        selected: state.files.selected.as_ref().map(entry_key),
    };
    Ok(selection.current(state).then_some(selection))
}
pub(crate) fn available(state: &State) -> bool {
    matches!(capture(state, None), Ok(Some(_)))
}
pub(crate) fn entry_available(state: &State, entry: &Entry) -> bool {
    matches!(capture(state, Some(entry)), Ok(Some(_)))
}
pub(crate) fn start(state: RwSignal<State>, entry: Option<Entry>) {
    let selection = match state.with_untracked(|s| capture(s, entry.as_ref())) {
        Ok(Some(selection)) => selection,
        Ok(None) => return,
        Err(error) => {
            fail(state, error);
            return;
        }
    };
    let generation = state
        .try_update(|s| {
            s.files.deletion.cancel_review();
            s.files.deletion.directory.review_loading = true;
            s.files.deletion.review_generation
        })
        .expect("invariant: mounted directory control owns its state");
    spawn_local(async move {
        let result: Result<Arc<Request>, api::Error> = async {
            let child =
                read_status(&selection.location.repository, selection.path.as_str()).await?;
            if child.initialized || child.directory_id != selection.entry {
                return Err(invalid(
                    "Only an ordinary child directory can use whole-directory soft deletion.",
                ));
            }
            let owner = child
                .owner
                .as_ref()
                .filter(|owner| owner.path == selection.location.path)
                .ok_or_else(|| invalid("The folder does not belong to this initialized owner."))?;
            let parent = read_status(&selection.location.repository, &owner.path).await?;
            if !parent.initialized
                || parent.directory_id != owner.directory_id
                || parent.directory_branch_id.as_deref() != Some(selection.branch.as_str())
            {
                return Err(invalid(
                    "The reviewed directory owner or current branch changed.",
                ));
            }
            Request::new(
                selection.location.slot.clone(),
                HistoryRepositoryId::parse(&selection.location.repository)
                    .map_err(identity_error)?,
                owner.directory_id.clone(),
                selection.branch.clone(),
                selection.tree.clone(),
                selection.entry.clone(),
                selection.path.clone(),
            )
            .map(Arc::new)
        }
        .await;
        if !accept_read_response(state, selection.location.epoch, &result) {
            return;
        }
        if !state.with_untracked(|s| {
            s.files.deletion.review_generation == generation && selection.current(s)
        }) {
            state.update(|s| {
                if s.files.deletion.review_generation == generation {
                    s.files.deletion.directory.review_loading = false;
                }
            });
            return;
        }
        state.update(|s| s.files.deletion.directory.review_loading = false);
        match result {
            Ok(request) => state.update(|s| {
                s.files.deletion.directory.review = Some(Review {
                    generation,
                    selection,
                    request,
                });
            }),
            Err(error) => fail(state, error),
        }
    });
}
fn reviewed(state: &State, generation: u64) -> Option<&Review> {
    state
        .files
        .deletion
        .directory
        .review
        .as_ref()
        .filter(|review| {
            review.generation == generation
                && state.files.deletion.review_generation == generation
                && review.selection.current(state)
        })
}
fn confirm_delete(state: RwSignal<State>, generation: u64) {
    let request =
        state.with_untracked(|s| reviewed(s, generation).map(|review| Arc::clone(&review.request)));
    if let Some(request) = request {
        super::request::submit(state, super::request::Intent::DeleteDirectory(request));
    }
}
fn read_allowed(state: &State) -> bool {
    Location::capture(state).is_some() && !state.files.deletion.directory.loading
}
fn scope_current(state: &State) -> Option<&Scope> {
    let model = &state.files.deletion.directory;
    model
        .scope
        .as_ref()
        .filter(|scope| model.open && scope.location.current(state))
}
fn accepts_read(state: RwSignal<State>, location: &Location, generation: u64) -> bool {
    state
        .try_update(|s| {
            let current = location.current(s) && s.files.deletion.directory.accepts(generation);
            if !current && s.files.deletion.directory.accepts(generation) {
                // A listing refresh can invalidate this read without a navigation.
                // It must not leave the next explicit reload permanently disabled.
                s.files.deletion.directory.loading = false;
            }
            current
        })
        .unwrap_or(false)
}
fn load(state: RwSignal<State>, next: bool) {
    let captured = state.with_untracked(|s| {
        if !read_allowed(s) {
            return None;
        }
        let location = Location::capture(s)?;
        let next = if next {
            let scope = scope_current(s)?.clone();
            let page = s.files.deletion.directory.page.as_ref()?;
            Some((scope, page.snapshot.clone(), page.next_after.clone()?))
        } else {
            None
        };
        Some((location, next))
    });
    let Some((location, next)) = captured else {
        return;
    };
    let generation = state
        .try_update(|s| {
            s.files.deletion.cancel_review();
            let model = &mut s.files.deletion.directory;
            model.invalidate();
            model.open = true;
            model.loading = true;
            model.scope = None;
            model.generation
        })
        .expect("invariant: mounted directory discovery owns its state");
    spawn_local(async move {
        let result: Result<(Scope, DirectoryDeletionsPage), api::Error> = async {
            let (scope, endpoint) = if let Some((scope, snapshot, after)) = next {
                let endpoint = format!(
                    "{}?after={}&snapshot={}",
                    scope.endpoint(),
                    api::segment(&after),
                    api::segment(&snapshot)
                );
                (scope, endpoint)
            } else {
                let status = read_status(&location.repository, &location.path).await?;
                let owner = status.owner.ok_or_else(|| {
                    invalid("This location has no initialized directory owner to list deletions.")
                })?;
                let scope = Scope {
                    location: location.clone(),
                    owner: owner.directory_id,
                    owner_path: owner.path,
                };
                let endpoint = scope.endpoint();
                (scope, endpoint)
            };
            let page = api::validated_request("GET", &endpoint, None, wire::page).await?;
            Ok((scope, page))
        }
        .await;
        if !accept_read_response(state, location.epoch, &result)
            || !accepts_read(state, &location, generation)
        {
            return;
        }
        state.update(|s| s.files.deletion.directory.loading = false);
        match result {
            Ok((scope, page)) => state.update(|s| {
                s.files.deletion.directory.scope = Some(scope);
                s.files.deletion.directory.page = Some(Arc::new(page));
            }),
            Err(error) => fail(state, error),
        }
    });
}
fn select(state: RwSignal<State>, summary: DirectoryDeletionSummary) {
    let scope = state.with_untracked(|s| {
        let model = &s.files.deletion.directory;
        scope_current(s)
            .filter(|_| {
                !model.loading
                    && model
                        .page
                        .as_ref()
                        .is_some_and(|page| page.entries.contains(&summary))
            })
            .cloned()
    });
    let Some(scope) = scope else {
        return;
    };
    let generation = state
        .try_update(|s| {
            s.files.deletion.cancel_review();
            let model = &mut s.files.deletion.directory;
            model.generation += 1;
            model.loading = true;
            model.selected = Some(summary.clone());
            model.node = None;
            model.generation
        })
        .expect("invariant: mounted directory selection owns its state");
    spawn_local(async move {
        let result = api::validated_request(
            "GET",
            &scope.node_endpoint(&summary.directory_id),
            None,
            |value| wire::node(value, &scope.location.repository, &scope.owner, &summary),
        )
        .await;
        if !accept_read_response(state, scope.location.epoch, &result)
            || !accepts_read(state, &scope.location, generation)
        {
            return;
        }
        state.update(|s| s.files.deletion.directory.loading = false);
        match result {
            Ok(node) => state.update(|s| s.files.deletion.directory.node = node.map(Arc::new)),
            Err(error) => fail(state, error),
        }
    });
}
fn undo_allowed(state: &State) -> bool {
    let model = &state.files.deletion.directory;
    actions::write_allowed(state)
        && scope_current(state).is_some()
        && !model.loading
        && model.node.is_some()
}
fn restore_button_current(state: &State, expected: &Arc<DirectoryDeletionNode>) -> bool {
    undo_allowed(state)
        && state
            .files
            .deletion
            .directory
            .node
            .as_ref()
            .is_some_and(|node| Arc::ptr_eq(node, expected))
}
fn review_restore(state: RwSignal<State>, expected: Arc<DirectoryDeletionNode>) {
    let target = state.with_untracked(|s| {
        if !restore_button_current(s, &expected) {
            return None;
        }
        Some((
            scope_current(s)?.clone(),
            s.files.deletion.directory.selected.clone()?,
            s.files.deletion.directory.node.clone()?,
        ))
    });
    let Some((scope, summary, original)) = target else {
        return;
    };
    let (generation, review_generation) = state
        .try_update(|s| {
            s.files.deletion.cancel_review();
            let model = &mut s.files.deletion.directory;
            model.generation += 1;
            model.loading = true;
            (model.generation, s.files.deletion.review_generation)
        })
        .expect("invariant: mounted directory restore owns its state");
    spawn_local(async move {
        let result = api::validated_request(
            "GET",
            &scope.node_endpoint(&summary.directory_id),
            None,
            |value| {
                wire::unchanged_node(
                    value,
                    &scope.location.repository,
                    &scope.owner,
                    &summary,
                    &original,
                )
            },
        )
        .await;
        if !accept_read_response(state, scope.location.epoch, &result)
            || !accepts_read(state, &scope.location, generation)
            || !state.with_untracked(|s| s.files.deletion.review_generation == review_generation)
        {
            return;
        }
        state.update(|s| s.files.deletion.directory.loading = false);
        match result {
            Ok(node) => state.update(|s| {
                let node = node.map(Arc::new);
                s.files.deletion.directory.node = node.clone();
                s.files.deletion.directory.restore_review = node.map(|node| RestoreReview {
                    generation: review_generation,
                    node,
                });
            }),
            Err(error) => fail(state, error),
        }
    });
}
fn restore_reviewed(state: &State, generation: u64) -> Option<Arc<DirectoryDeletionNode>> {
    if !undo_allowed(state) {
        return None;
    }
    let model = &state.files.deletion.directory;
    model
        .restore_review
        .as_ref()
        .filter(|review| {
            review.generation == generation
                && state.files.deletion.review_generation == generation
                && model
                    .node
                    .as_ref()
                    .is_some_and(|node| Arc::ptr_eq(node, &review.node))
        })
        .map(|review| Arc::clone(&review.node))
}
fn confirm_restore(state: RwSignal<State>, generation: u64) {
    let node = state.with_untracked(|s| restore_reviewed(s, generation));
    if let Some(node) = node {
        state.update(|s| s.files.deletion.cancel_review());
        super::request::submit(state, super::request::Intent::UndoDirectory(node));
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/deletion/directory_tests.rs"]
mod tests;
