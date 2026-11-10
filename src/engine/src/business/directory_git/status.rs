//! Bounded, cancellable status over an immutable admitted metadata observation.
//! One ownership index replaces repeated repository-by-namespace scans. No full
//! directory projection or descendant-tree clone is needed for this read.
use super::*;

const MAX_STATUS_WORK: usize = 1_000_000;
const MAX_STATUS_TEXT_BYTES: usize = 16 * 1024 * 1024;

struct Budget<'a> {
    cancellation: &'a HistoryWindowCancellation,
    work: usize,
    text_bytes: usize,
}
impl<'a> Budget<'a> {
    fn new(cancellation: &'a HistoryWindowCancellation) -> Self {
        Self {
            cancellation,
            work: MAX_STATUS_WORK,
            text_bytes: MAX_STATUS_TEXT_BYTES,
        }
    }
    fn step(&mut self) -> Result<()> {
        checkpoint(self.cancellation)?;
        self.work = self.work.checked_sub(1).ok_or_else(limit)?;
        Ok(())
    }
    fn copy(&mut self, value: &str) -> Result<String> {
        self.text(value)?;
        Ok(value.to_owned())
    }
    fn text(&mut self, value: &str) -> Result<()> {
        checkpoint(self.cancellation)?;
        self.text_bytes = self.text_bytes.checked_sub(value.len()).ok_or_else(limit)?;
        Ok(())
    }
}
fn checkpoint(cancellation: &HistoryWindowCancellation) -> Result<()> {
    if cancellation.is_cancelled() {
        return Err(Error::new(
            ErrorCode::OperationAborted,
            "directory status read cancelled",
        ));
    }
    Ok(())
}
fn limit() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "directory status exceeds its metadata work budget",
    )
}
fn parent(key: &str) -> Option<&str> {
    if key.is_empty() {
        None
    } else {
        Some(key.rsplit_once('/').map_or("", |(parent, _)| parent))
    }
}
#[derive(Clone, Copy)]
struct Location<'a> {
    directory: &'a str,
    path: &'a str,
    key: &'a str,
}
struct Ownership<'a> {
    locations: BTreeMap<&'a str, Location<'a>>,
    paths: BTreeMap<&'a str, &'a str>,
    initialized: BTreeMap<&'a str, &'a str>,
    children: BTreeMap<&'a str, BTreeMap<&'a str, &'a str>>,
}
impl<'a> Ownership<'a> {
    fn build(
        state: &'a State,
        repository: &'a str,
        entries: &'a [Entry],
        budget: &mut Budget<'_>,
    ) -> Result<Self> {
        let root = Location {
            directory: repository,
            path: "",
            key: "",
        };
        let mut result = Self {
            locations: BTreeMap::from([(repository, root)]),
            paths: BTreeMap::from([("", repository)]),
            initialized: BTreeMap::new(),
            children: BTreeMap::new(),
        };
        for entry in entries {
            budget.step()?;
            budget.text(entry.path())?;
            if entry.kind == NamespaceEntryKind::Directory {
                result.locations.insert(
                    &entry.entry_id,
                    Location {
                        directory: &entry.entry_id,
                        path: entry.path(),
                        key: entry.lookup_key(),
                    },
                );
                result.paths.insert(entry.lookup_key(), &entry.entry_id);
            }
        }
        for git in state.directory_git_repositories.values() {
            budget.step()?;
            if git.repository_id == repository
                && let Some(location) = result.locations.get(git.directory_id.as_str())
            {
                result.initialized.insert(location.key, location.directory);
            }
        }
        for (&key, &id) in &result.initialized {
            budget.step()?;
            if let Some(owner) = result.owner(parent(key), budget)? {
                result
                    .children
                    .entry(owner)
                    .or_default()
                    .insert(id, result.locations[id].path);
            }
        }
        Ok(result)
    }
    fn owner(&self, mut key: Option<&str>, budget: &mut Budget<'_>) -> Result<Option<&'a str>> {
        while let Some(current) = key {
            budget.step()?;
            budget.text(current)?;
            if let Some(id) = self.initialized.get(current) {
                return Ok(Some(id));
            }
            key = parent(current);
        }
        Ok(None)
    }
    fn locate(&self, path: &str) -> Result<Location<'a>> {
        let key = if path.is_empty() {
            String::new()
        } else {
            name_policy::LookupPath::parse(path)?.key().to_owned()
        };
        self.paths
            .get(key.as_str())
            .map(|id| self.locations[id])
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DirectoryNotFound,
                    "directory missing from the working view",
                )
            })
    }
}

/// Borrow all occurrence values. Only the bounded map nodes are allocated, not
/// namespace entries, path strings, file contexts, or nested immutable trees.
#[derive(PartialEq)]
struct Occurrence<'a> {
    entry_id: &'a str,
    path: &'a str,
    kind: NamespaceEntryKind,
    file_id: &'a str,
    version_id: &'a str,
    size: u64,
    node: Option<&'a str>,
    branch: Option<&'a str>,
}
struct Contents<'a> {
    entries: &'a [Entry],
    contexts: &'a BTreeMap<String, FileContext>,
    path: &'a str,
}
fn relative(path: &str, depth: usize) -> &str {
    path.splitn(depth + 1, '/')
        .nth(depth)
        .expect("invariant: directory entries are strict descendants of their root")
}
fn owned<'a>(
    contents: Contents<'a>,
    boundaries: &BTreeSet<&str>,
    budget: &mut Budget<'_>,
) -> Result<BTreeMap<&'a str, Occurrence<'a>>> {
    let depth = if contents.path.is_empty() {
        0
    } else {
        contents.path.split('/').count()
    };
    let mut blocked = BTreeSet::new();
    for entry in contents.entries {
        budget.step()?;
        if entry.kind == NamespaceEntryKind::Directory
            && boundaries.contains(entry.entry_id.as_str())
        {
            blocked.insert(entry.lookup_key());
        }
    }
    let mut result = BTreeMap::new();
    for entry in contents.entries {
        budget.step()?;
        budget.text(entry.path())?;
        let mut ancestor = Some(entry.lookup_key());
        let mut excluded = false;
        while let Some(key) = ancestor {
            budget.step()?;
            budget.text(key)?;
            if blocked.contains(key) {
                excluded = true;
                break;
            }
            ancestor = parent(key);
        }
        if excluded {
            continue;
        }
        let Entry {
            entry_id,
            path,
            kind,
            file_id,
            version_id,
            size,
        } = entry;
        let context = if *kind == NamespaceEntryKind::File {
            Some(contents.contexts.get(entry.lookup_key()).ok_or_else(|| {
                Error::new(
                    ErrorCode::StateInvalid,
                    "snapshot file path context missing",
                )
            })?)
        } else {
            None
        };
        result.insert(
            relative(entry.lookup_key(), depth),
            Occurrence {
                entry_id,
                path: relative(path.as_str(), depth),
                kind: *kind,
                file_id,
                version_id,
                size: *size,
                node: context.and_then(|context| context.node_id.as_deref()),
                branch: context.map(|context| context.branch_id.as_str()),
            },
        );
    }
    Ok(result)
}
fn changes(
    state: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    git: &DirectoryGitRepository,
    branch: &str,
    boundaries: &BTreeMap<&str, &str>,
    budget: &mut Budget<'_>,
) -> Result<(u64, u64, u64, bool)> {
    let index = &git.branches[branch];
    let tree = working_tree(state, git, branch)?;
    let contents = match view {
        Some(view) => Contents {
            entries: view.directory_entries(branch, &tree.tree_id)?,
            contexts: &state.directory_branches[branch].file_contexts,
            path: &tree.path,
        },
        None => Contents {
            entries: &tree.entries,
            contexts: &tree.file_contexts,
            path: &tree.path,
        },
    };
    let mut boundary_ids = BTreeSet::new();
    for &id in boundaries.keys() {
        budget.step()?;
        boundary_ids.insert(id);
    }
    let working = owned(contents, &boundary_ids, budget)?;
    let staged = &state.directory_trees[&index.index_tree_id];
    let after = owned(
        Contents {
            entries: &staged.entries,
            contexts: &staged.file_contexts,
            path: &staged.path,
        },
        &boundary_ids,
        budget,
    )?;
    let mut unstaged = working.len() != after.len();
    for (current, indexed) in working.iter().zip(&after) {
        budget.step()?;
        if current != indexed {
            unstaged = true;
            break;
        }
    }
    let before = match &index.head_tree_id {
        Some(id) => {
            let head = &state.directory_trees[id];
            // A newly initialized child was ordinary owned content in the old
            // commit. Its removal remains staged until the parent commits.
            let mut ids = BTreeSet::new();
            for id in head.directory_refs.keys() {
                budget.step()?;
                ids.insert(id.as_str());
            }
            owned(
                Contents {
                    entries: &head.entries,
                    contexts: &head.file_contexts,
                    path: &head.path,
                },
                &ids,
                budget,
            )?
        }
        None => BTreeMap::new(),
    };
    let (mut added, mut modified, mut deleted) = (0, 0, 0);
    for (key, value) in &after {
        budget.step()?;
        match before.get(key) {
            None => added += 1,
            Some(old) if old != value => modified += 1,
            Some(_) => (),
        }
    }
    for key in before.keys() {
        budget.step()?;
        if !after.contains_key(key) {
            deleted += 1;
        }
    }
    Ok((added, modified, deleted, unstaged))
}

fn mount_paths<'a>(
    tree: &'a DirectoryTree,
    budget: &mut Budget<'_>,
) -> Result<BTreeMap<&'a str, &'a str>> {
    let depth = if tree.path.is_empty() {
        0
    } else {
        tree.path.split('/').count()
    };
    let mut paths = BTreeMap::new();
    for entry in &tree.entries {
        budget.step()?;
        if entry.kind == NamespaceEntryKind::Directory {
            budget.text(entry.path())?;
            paths.insert(entry.entry_id.as_str(), relative(entry.path(), depth));
        }
    }
    Ok(paths)
}
struct Summary {
    added: u64,
    modified: u64,
    deleted: u64,
    staged: bool,
    unstaged: bool,
    staged_children: BTreeSet<String>,
    needs_restage: bool,
    dirty: bool,
}
fn summarize_scope(
    state: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    git: &DirectoryGitRepository,
    branch: &str,
    children: &BTreeMap<&str, &str>,
    budget: &mut Budget<'_>,
) -> Result<Summary> {
    let index = &git.branches[branch];
    let (added, modified, deleted, unstaged) = changes(state, view, git, branch, children, budget)?;
    let mut needs_restage = index.index_boundaries.len() != children.len();
    let mut staged = index.head_tree_id.is_none() || added != 0 || modified != 0 || deleted != 0;
    let indexed_paths = if children.is_empty() {
        BTreeMap::new()
    } else {
        mount_paths(&state.directory_trees[&index.index_tree_id], budget)?
    };
    let committed_paths = match &index.head_tree_id {
        Some(head) if !children.is_empty() => mount_paths(&state.directory_trees[head], budget)?,
        _ => BTreeMap::new(),
    };
    let mut staged_children = BTreeSet::new();
    for (&child, &path) in children {
        budget.step()?;
        budget.text(path)?;
        needs_restage |= index.index_boundaries.get(child).map(String::as_str) != Some(path);
        let pinned = index_child(index, child);
        let changed = pinned
            != head_child(state, index, child).map(|reference| reference.tree_id.as_str())
            || (pinned.is_some() && indexed_paths.get(child) != committed_paths.get(child));
        if changed {
            staged_children.insert(budget.copy(child)?);
            staged = true;
        }
    }
    if let Some(head) = &index.head_tree_id {
        for child in state.directory_trees[head].directory_refs.keys() {
            budget.step()?;
            staged |= !children.contains_key(child.as_str());
        }
    }
    Ok(Summary {
        added,
        modified,
        deleted,
        staged,
        unstaged,
        staged_children,
        needs_restage,
        dirty: staged || unstaged || needs_restage,
    })
}
fn summarize(
    state: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    ownership: &Ownership<'_>,
    directory: &str,
    summaries: Option<&BTreeMap<&str, Summary>>,
    budget: &mut Budget<'_>,
) -> Result<Summary> {
    let git = &state.directory_git_repositories[directory];
    let index = &git.branches[&git.current_branch_id];
    let empty = BTreeMap::new();
    let children = ownership.children.get(directory).unwrap_or(&empty);
    let mut summary = summarize_scope(state, view, git, &git.current_branch_id, children, budget)?;
    if let Some(summaries) = summaries {
        for &child in children.keys() {
            budget.step()?;
            let child_git = &state.directory_git_repositories[child];
            summary.dirty |= summaries[child].dirty
                || child_git.branches[&child_git.current_branch_id]
                    .head_tree_id
                    .as_deref()
                    != index_child(index, child);
        }
    }
    Ok(summary)
}

/// One history page shares one ownership index and one total computation budget.
/// Deleted directory histories retain their reviewed boundaries by stable ID;
/// they must never bind to a newly created directory at the same path.
pub(crate) struct BranchStatuses<'a> {
    state: &'a State,
    ownership: Ownership<'a>,
    budget: Budget<'a>,
}
impl<'a> BranchStatuses<'a> {
    pub(crate) fn new(
        state: &'a State,
        view: Option<&'a crate::engine::CurrentNamespaces>,
        repository: &'a str,
        cancellation: &'a HistoryWindowCancellation,
    ) -> Result<Self> {
        let mut budget = Budget::new(cancellation);
        budget.step()?;
        let entries = current_entries(state, view, repository)?;
        let ownership = Ownership::build(state, repository, entries, &mut budget)?;
        Ok(Self {
            state,
            ownership,
            budget,
        })
    }
    pub(crate) fn staged(&mut self, branch: &str) -> Result<bool> {
        self.budget.step()?;
        let record = &self.state.directory_branches[branch];
        let git = &self.state.directory_git_repositories[&record.directory_id];
        let empty = BTreeMap::new();
        let retained;
        let children = if self
            .ownership
            .locations
            .contains_key(git.directory_id.as_str())
        {
            self.ownership
                .children
                .get(git.directory_id.as_str())
                .unwrap_or(&empty)
        } else {
            let mut boundaries = BTreeMap::new();
            for (id, path) in &git.branches[branch].index_boundaries {
                self.budget.step()?;
                self.budget.text(path)?;
                boundaries.insert(id.as_str(), path.as_str());
            }
            retained = boundaries;
            &retained
        };
        let summary = summarize_scope(self.state, None, git, branch, children, &mut self.budget)?;
        Ok(summary.staged || summary.unstaged || summary.needs_restage)
    }
}

fn summaries<'a>(
    state: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    ownership: &Ownership<'a>,
    directory: &'a str,
    budget: &mut Budget<'_>,
) -> Result<BTreeMap<&'a str, Summary>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![(directory, false)];
    while let Some((id, expanded)) = pending.pop() {
        budget.step()?;
        if expanded {
            result.insert(
                id,
                summarize(state, view, ownership, id, Some(&result), budget)?,
            );
        } else {
            pending.push((id, true));
            if let Some(children) = ownership.children.get(id) {
                for &child in children.keys() {
                    budget.step()?;
                    pending.push((child, false));
                }
            }
        }
    }
    Ok(result)
}

pub(crate) struct ScopeChanges {
    pub(crate) directory_id: String,
    pub(crate) initialized: bool,
    pub(crate) added: u64,
    pub(crate) modified: u64,
    pub(crate) deleted: u64,
    pub(crate) staged: bool,
    pub(crate) unstaged: bool,
    pub(crate) needs_restage: bool,
}

/// Commit already resolved the current directory and its direct children.
/// Reuse that scope; do not rebuild an ownership index for the entire namespace.
pub(in crate::business) fn staged_changes(
    state: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    git: &DirectoryGitRepository,
    branch: &str,
    direct: &BTreeMap<String, String>,
) -> Result<ScopeChanges> {
    let cancellation = HistoryWindowCancellation::new();
    let mut budget = Budget::new(&cancellation);
    let children = direct.iter().map(|(id, path)| (id.as_str(), path.as_str())).collect();
    let summary = summarize_scope(state, view, git, branch, &children, &mut budget)?;
    Ok(ScopeChanges {
        directory_id: budget.copy(&git.directory_id)?,
        initialized: true,
        added: summary.added,
        modified: summary.modified,
        deleted: summary.deleted,
        staged: summary.staged,
        unstaged: summary.unstaged,
        needs_restage: summary.needs_restage,
    })
}

/// Mutations and history headers inspect only this repository, never recurse
/// through independent children whose dirtiness cannot block its own commit.
pub(crate) fn current_changes(
    state: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    repository: &str,
    path: &str,
    cancellation: &HistoryWindowCancellation,
) -> Result<ScopeChanges> {
    let mut budget = Budget::new(cancellation);
    budget.step()?;
    budget.text(path)?;
    let entries = current_entries(state, view, repository)?;
    let ownership = Ownership::build(state, repository, entries, &mut budget)?;
    let location = ownership.locate(path)?;
    let mut changes = ScopeChanges {
        directory_id: budget.copy(location.directory)?,
        initialized: state
            .directory_git_repositories
            .contains_key(location.directory),
        added: 0,
        modified: 0,
        deleted: 0,
        staged: false,
        unstaged: false,
        needs_restage: false,
    };
    if changes.initialized {
        let summary = summarize(
            state,
            view,
            &ownership,
            location.directory,
            None,
            &mut budget,
        )?;
        changes.staged = summary.staged;
        changes.unstaged = summary.unstaged;
        changes.needs_restage = summary.needs_restage;
        changes.added = summary.added;
        changes.modified = summary.modified;
        changes.deleted = summary.deleted;
    }
    Ok(changes)
}

pub(super) fn read(
    state: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    repository: &str,
    path: &str,
    cancellation: &HistoryWindowCancellation,
) -> Result<DirectoryGitStatus> {
    let mut budget = Budget::new(cancellation);
    budget.step()?;
    budget.text(path)?;
    let entries = current_entries(state, view, repository)?;
    let ownership = Ownership::build(state, repository, entries, &mut budget)?;
    let location = ownership.locate(path)?;
    let owner = ownership
        .owner(Some(location.key), &mut budget)?
        .map(|directory| -> Result<DirectoryGitOwner> {
            Ok(DirectoryGitOwner {
                directory_id: budget.copy(directory)?,
                path: budget.copy(ownership.locations[directory].path)?,
            })
        })
        .transpose()?;
    let mut status = DirectoryGitStatus {
        repository_id: budget.copy(repository)?,
        directory_id: budget.copy(location.directory)?,
        path: budget.copy(location.path)?,
        initialized: false,
        owner,
        directory_branch_id: None,
        working_tree_id: None,
        head_tree_id: None,
        index_generation: 0,
        staged: false,
        unstaged: false,
        needs_restage: false,
        added: 0,
        modified: 0,
        deleted: 0,
        children: Vec::new(),
        legacy_history: false,
    };
    let Some(git) = state.directory_git_repositories.get(location.directory) else {
        return Ok(status);
    };
    let summaries = summaries(state, view, &ownership, location.directory, &mut budget)?;
    let summary = &summaries[location.directory];
    let index = &git.branches[&git.current_branch_id];
    status.initialized = true;
    status.directory_branch_id = Some(budget.copy(&git.current_branch_id)?);
    status.working_tree_id =
        Some(budget.copy(&working_tree(state, git, &git.current_branch_id)?.tree_id)?);
    status.head_tree_id = index
        .head_tree_id
        .as_deref()
        .map(|id| budget.copy(id))
        .transpose()?;
    status.index_generation = index.index_generation;
    status.staged = summary.staged;
    status.unstaged = summary.unstaged;
    status.needs_restage = summary.needs_restage;
    status.added = summary.added;
    status.modified = summary.modified;
    status.deleted = summary.deleted;
    if let Some(children) = ownership.children.get(location.directory) {
        for (&child, &path) in children {
            budget.step()?;
            budget.text(path)?;
            let child_git = &state.directory_git_repositories[child];
            let child_index = &child_git.branches[&child_git.current_branch_id];
            let staged_tree_id = index_child(index, child)
                .map(|id| budget.copy(id))
                .transpose()?;
            let committed_tree_id = head_child(state, index, child)
                .map(|reference| budget.copy(&reference.tree_id))
                .transpose()?;
            status.children.push(DirectoryGitChildStatus {
                directory_id: budget.copy(child)?,
                path: budget.copy(path)?,
                directory_branch_id: budget.copy(&child_git.current_branch_id)?,
                head_tree_id: child_index
                    .head_tree_id
                    .as_deref()
                    .map(|id| budget.copy(id))
                    .transpose()?,
                dirty: summaries[child].dirty,
                reference_changed: child_index.head_tree_id != staged_tree_id,
                staged: summary.staged_children.contains(child),
                staged_tree_id,
                committed_tree_id,
                needs_restage: summaries[child].needs_restage,
            });
        }
    }
    checkpoint(cancellation)?;
    Ok(status)
}

/// Detached business observation. It grants no authorization or mutation rights;
/// the caller authenticates before capture and again before the final recheck.
pub struct DirectoryGitStatusRead {
    view: MetadataView,
    repository: String,
    path: String,
    cancellation: HistoryWindowCancellation,
}
impl DirectoryGitStatusRead {
    pub fn status(&self) -> Result<DirectoryGitStatus> {
        read(
            self.view.business_projection(),
            self.view.current_namespaces.admitted()?,
            &self.repository,
            &self.path,
            &self.cancellation,
        )
    }
}
impl Engine {
    pub fn capture_directory_git_status(
        &self,
        repository: &str,
        path: &str,
        cancellation: HistoryWindowCancellation,
    ) -> Result<DirectoryGitStatusRead> {
        checkpoint(&cancellation)?;
        validate_id(repository, "repo")?;
        if !path.is_empty() {
            name_policy::LookupPath::parse(path)?;
        }
        self.admitted_current_namespaces()?;
        Ok(DirectoryGitStatusRead {
            view: self.metadata_view(),
            repository: repository.into(),
            path: path.into(),
            cancellation,
        })
    }
    pub fn recheck_directory_git_status(&self, read: &DirectoryGitStatusRead) -> Result<()> {
        checkpoint(&read.cancellation)?;
        self.admitted_current_namespaces()?;
        if self.state.sequence != read.view.business_projection().sequence
            || !self
                .namespace_read_view()
                .same_publication(&read.view.current_namespaces)
        {
            return Err(Error::new(
                ErrorCode::ListingChanged,
                "directory status changed; refresh before using it",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/directory_git_status_tests.rs"]
mod tests;
