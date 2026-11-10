use super::*;
use crate::files::{model::entry_key, page::DirectoryBranch, page::Entry};
use json_codec::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

mod persist;
mod receipt;
mod view;
pub(super) use persist::{Stored, stored};
pub(super) use view::{retained_view, view};

const MAX_ITEMS: usize = 100;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Request {
    slot: String,
    repository: HistoryRepositoryId,
    branch: String,
    tree: String,
    entries: Vec<Selected>,
}
#[derive(Debug, PartialEq, Eq)]
struct Selected {
    entry: String,
    file: HistoryFileId,
    path: name_policy::PortablePath,
}

fn invalid(message: &str) -> api::Error {
    api::error(storage_contract::ErrorCode::InvalidRequest, message)
}
fn identity_error(source: storage_contract::Error) -> api::Error {
    let mut error = invalid("The batch deletion identity is invalid; no request was sent.");
    error.source = Some(Arc::new(api::ErrorSource::FileDeletionRecovery(source)));
    error
}
impl Request {
    fn new(
        slot: String,
        repository: HistoryRepositoryId,
        branch: String,
        tree: String,
        entries: Vec<Selected>,
    ) -> Result<Self, api::Error> {
        name_policy::validate_slot(&slot).map_err(|source| identity_error(source.into()))?;
        storage_contract::validate_id(&branch, "dbr").map_err(identity_error)?;
        storage_contract::validate_id(&tree, "dtree").map_err(identity_error)?;
        if entries.is_empty() || entries.len() > MAX_ITEMS {
            return Err(invalid("Select one to one hundred exact file occurrences."));
        }
        let mut seen = BTreeSet::new();
        for selected in &entries {
            storage_contract::validate_id(&selected.entry, "entry").map_err(identity_error)?;
            if !seen.insert((&selected.entry, selected.path.key())) {
                return Err(invalid(
                    "A file occurrence appears more than once in this batch.",
                ));
            }
        }
        Ok(Self {
            slot,
            repository,
            branch,
            tree,
            entries,
        })
    }

    fn namespace_request(&self) -> storage_contract::DirectoryNamespaceBatch {
        storage_contract::DirectoryNamespaceBatch {
            kind: storage_contract::DirectoryBatchKind::Delete,
            items: self
                .entries
                .iter()
                .map(|entry| storage_contract::DirectoryNamespaceItem {
                    source: storage_contract::DirectoryNamespaceSource {
                        directory_branch_id: self.branch.clone(),
                        expected_tree_id: self.tree.clone(),
                        entry_id: entry.entry.clone(),
                        path: entry.path.as_str().into(),
                    },
                    target: None,
                })
                .collect(),
        }
    }

    pub(super) fn request(&self, operation: &str) -> (String, Value) {
        let request = self.namespace_request();
        (
            format!(
                "/api/file-manager/disks/{}/namespace-batch",
                api::segment(&self.slot)
            ),
            json!({"kind":request.kind,"items":request.items,"operation_id":operation}),
        )
    }

    pub(super) fn input(&self) -> Value {
        json!({"repository_id":self.repository.as_str(),"request":self.namespace_request()})
    }

    fn targets(&self) -> BTreeMap<&str, &name_policy::PortablePath> {
        let mut files = BTreeMap::new();
        for selected in &self.entries {
            files
                .entry(selected.file.as_str())
                .or_insert(&selected.path);
        }
        files
    }

    pub(super) fn label(&self) -> String {
        format!(
            "{} · {} · {}",
            self.slot,
            self.repository,
            self.entries.len()
        )
    }
}

#[derive(Clone)]
pub(super) struct Review {
    epoch: u64,
    generation: u64,
    listing_generation: u64,
    directory: String,
    checked: BTreeSet<String>,
    branch_name: String,
    request: Arc<Request>,
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
        && name_policy::path_key(branch.path())
            == name_policy::path_key(&state.files.repository_path))
    .then_some(branch)
}

fn selection(state: &State) -> Option<(&DirectoryBranch, Vec<&Entry>)> {
    let branch = current_branch(state)?;
    let checked = &state.files.checked_entries;
    if !(2..=MAX_ITEMS).contains(&checked.len()) {
        return None;
    }
    let entries = state
        .files
        .entries
        .iter()
        .filter(|entry| checked.contains(&entry_key(entry)))
        .collect::<Vec<_>>();
    if entries.len() != checked.len()
        || entries.iter().any(|entry| {
            !request::eligible(entry) || entry.repository_id() != Some(branch.repository_id())
        })
    {
        return None;
    }
    Some((branch, entries))
}
pub(super) fn available(state: &State) -> bool {
    selection(state).is_some()
}

fn capture(state: &State) -> Result<Option<Review>, api::Error> {
    let Some((branch, entries)) = selection(state) else {
        return Ok(None);
    };
    let entries = entries
        .into_iter()
        .map(|entry| {
            Ok(Selected {
                entry: entry.entry_id().into(),
                file: HistoryFileId::parse(entry.file_id()).map_err(identity_error)?,
                path: name_policy::PortablePath::parse(
                    entry
                        .repository_path()
                        .expect("invariant: eligible files have a location"),
                )
                .map_err(|source| identity_error(source.into()))?,
            })
        })
        .collect::<Result<Vec<_>, api::Error>>()?;
    let request = Request::new(
        state.files.slot.clone(),
        HistoryRepositoryId::parse(branch.repository_id()).map_err(identity_error)?,
        branch.id().into(),
        branch.head_tree_id().into(),
        entries,
    )?;
    Ok(Some(Review {
        epoch: state.session_epoch,
        generation: state.files.deletion.review_generation + 1,
        listing_generation: state.files.generation,
        directory: state.files.directory.clone(),
        checked: state.files.checked_entries.clone(),
        branch_name: branch.name().into(),
        request: Arc::new(request),
    }))
}
pub(super) fn start(state: RwSignal<State>) {
    match state.with_untracked(capture) {
        Ok(Some(review)) => state.update(|s| {
            s.files.deletion.cancel_review();
            s.files.deletion.review_generation = review.generation;
            s.files.deletion.batch_review = Some(review);
        }),
        Ok(None) => crate::ui::set_notice(
            state,
            web_interface::i18n::t(
                "请选择当前已初始化目录工作分支内本页的 2–100 个可写文件。目录、只读项或未加载项不会被跳过，也不会拆成多次删除。",
                "Select 2–100 writable files on this page in the current initialized folder workspace. Folders, read-only entries and unloaded entries are not skipped or split into separate deletions.",
            ),
        ),
        Err(error) => fail(state, error),
    }
}

fn reviewed(state: &State, epoch: u64, generation: u64) -> Option<&Review> {
    let review = state.files.deletion.batch_review.as_ref()?;
    let branch = current_branch(state)?;
    (review.epoch == epoch
        && state.session_epoch == epoch
        && review.generation == generation
        && review.listing_generation == state.files.generation
        && review.directory == state.files.directory
        && review.checked == state.files.checked_entries
        && review.request.slot == state.files.slot
        && review.request.repository.as_str() == branch.repository_id()
        && review.request.branch == branch.id()
        && review.request.tree == branch.head_tree_id())
    .then_some(review)
}
fn submit(state: RwSignal<State>, epoch: u64, generation: u64) {
    let request = state.with_untracked(|s| {
        reviewed(s, epoch, generation).map(|review| Arc::clone(&review.request))
    });
    if let Some(request) = request {
        request::submit(state, request::Intent::DeleteBatch(request));
    }
}
