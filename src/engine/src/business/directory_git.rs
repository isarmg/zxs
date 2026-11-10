//! Manually initialized directory repositories over the live file workspace.
//! Nested initialized repositories are ownership boundaries, not recursive commits.
use super::directory_staging::Ids;
use super::*;
mod branches;
pub(crate) mod export;
mod rename;
mod retention;
mod status;
use json_codec::{Value, json};
pub(crate) use rename::RenameOwnership;
pub(crate) use retention::{protected_git_versions, staged_owned_files};
pub use status::DirectoryGitStatusRead;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::directory_git::*;

pub(super) fn conflict(message: &str) -> Error {
    Error::new(ErrorCode::DirectoryBranchConflict, message)
}
fn required(message: &str) -> Error {
    Error::new(ErrorCode::DirectoryScopeRequired, message)
}
fn increment(value: u64) -> Result<u64> {
    value.checked_add(1).ok_or_else(|| {
        Error::new(
            ErrorCode::ResourceLimit,
            "directory index generation exhausted",
        )
    })
}
fn descendant(path: &str, root: &str) -> bool {
    root.is_empty() || namespace::is_descendant(path, root)
}

pub(crate) fn current_entries<'a>(
    state: &'a State,
    view: Option<&'a crate::engine::CurrentNamespaces>,
    repository: &str,
) -> Result<&'a [Entry]> {
    let repo = state
        .repositories
        .get(repository)
        .ok_or_else(|| Error::new(ErrorCode::RepositoryNotFound, "repository missing"))?;
    let main = &state.branches[&repo.main_branch_id];
    match view {
        Some(view) => view.branch_entries(&repo.main_branch_id, &main.head_commit_id),
        None => Ok(&state.commits[&main.head_commit_id].entries),
    }
}
pub(super) fn locate(entries: &[Entry], repository: &str, path: &str) -> Result<(String, String)> {
    if path.is_empty() {
        return Ok((repository.into(), String::new()));
    }
    let key = name_policy::LookupPath::parse(path)?;
    let entry = entries
        .iter()
        .find(|entry| {
            entry.kind == NamespaceEntryKind::Directory && entry.lookup_key() == key.key()
        })
        .ok_or_else(|| {
            Error::new(
                ErrorCode::DirectoryNotFound,
                "directory missing from the working view",
            )
        })?;
    Ok((entry.entry_id.clone(), entry.path().into()))
}
fn path_for<'a>(entries: &'a [Entry], repository: &str, directory: &str) -> Option<&'a str> {
    if repository == directory {
        Some("")
    } else {
        entries
            .iter()
            .find(|entry| {
                entry.kind == NamespaceEntryKind::Directory && entry.entry_id == directory
            })
            .map(Entry::path)
    }
}
pub(super) fn boundaries(
    state: &State,
    repository: &str,
    directory: &str,
    path: &str,
    entries: &[Entry],
) -> BTreeMap<String, String> {
    let key = name_policy::path_key(path);
    let mut direct = BTreeMap::new();
    let mut selected = BTreeSet::new();
    // Canonical current entries put each ancestor before its descendants.
    // Keep selected ancestors so similarly prefixed siblings do not hide them.
    for entry in entries {
        if entry.kind != NamespaceEntryKind::Directory
            || entry.entry_id == directory
            || !state.directory_git_repositories.get(&entry.entry_id)
                .is_some_and(|git| git.repository_id == repository)
            || !descendant(entry.lookup_key(), &key)
            || entry.lookup_key() == key
        {
            continue;
        }
        if namespace::closest_directory_repository(entry.lookup_key(), |key| {
            selected.get(key).copied()
        }).is_some() {
            continue;
        }
        direct.insert(entry.entry_id.clone(), entry.path().to_owned());
        selected.insert(entry.lookup_key());
    }
    direct
}

pub(super) fn working_tree<'a>(
    state: &'a State,
    git: &DirectoryGitRepository,
    branch: &str,
) -> Result<&'a DirectoryTree> {
    if !git.branches.contains_key(branch) {
        return Err(Error::new(
            ErrorCode::DirectoryBranchNotFound,
            "directory Git branch missing",
        ));
    }
    let id = &state
        .directory_branches
        .get(branch)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::StateInvalid,
                "Git branch has no working composition",
            )
        })?
        .head_tree_id;
    state
        .directory_trees
        .get(id)
        .ok_or_else(|| Error::new(ErrorCode::StateInvalid, "Git index tree missing"))
}
pub(crate) fn require_owned_path(
    state: &State,
    branch: &DirectoryBranch,
    path: &str,
) -> Result<()> {
    if !state
        .directory_git_repositories
        .get(&branch.directory_id)
        .is_some_and(|git| git.branches.contains_key(&branch.directory_branch_id))
    {
        return Err(required(
            "initialize the directory repository before editing its branch scope",
        ));
    }
    let key = name_policy::path_key(path);
    if branch
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && state
                    .directory_git_repositories
                    .contains_key(&entry.entry_id)
        })
        .any(|entry| {
            key == entry.lookup_key()
                || namespace::is_descendant(&key, entry.lookup_key())
                || namespace::is_descendant(entry.lookup_key(), &key)
        })
    {
        return Err(Error::new(
            ErrorCode::DirectoryBranchScope,
            "this path belongs to an independent child repository; select that repository explicitly",
        ));
    }
    Ok(())
}

pub(super) fn active_git<'a>(
    state: &'a State,
    repository: &str,
    branch: &str,
) -> Result<(&'a DirectoryGitRepository, &'a DirectoryGitBranch)> {
    let composition = state.directory_branches.get(branch).ok_or_else(|| {
        Error::new(
            ErrorCode::DirectoryBranchNotFound,
            "directory branch missing",
        )
    })?;
    if composition.repository_id != repository {
        return Err(Error::new(
            ErrorCode::DirectoryBranchScope,
            "directory branch belongs to another repository",
        ));
    }
    let git = state
        .directory_git_repositories
        .get(&composition.directory_id)
        .ok_or_else(|| {
            required("initialize this directory repository before changing its branches or commits")
        })?;
    if git.current_branch_id != branch {
        return Err(conflict(
            "check out this directory branch before changing its index",
        ));
    }
    Ok((
        git,
        git.branches.get(branch).ok_or_else(|| {
            Error::new(
                ErrorCode::StateInvalid,
                "current Git branch lacks its index",
            )
        })?,
    ))
}

#[derive(PartialEq)]
struct OwnedEntry {
    entry: Entry,
    node: Option<String>,
    branch: Option<String>,
}
fn owned(
    tree: &DirectoryTree,
    boundaries: &BTreeSet<&str>,
) -> Result<BTreeMap<String, OwnedEntry>> {
    let blocked = tree
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && boundaries.contains(entry.entry_id.as_str())
        })
        .map(|entry| entry.lookup_key())
        .collect::<Vec<_>>();
    let mut result = BTreeMap::new();
    for entry in &tree.entries {
        if blocked.iter().any(|path| {
            entry.lookup_key() == *path || namespace::is_descendant(entry.lookup_key(), path)
        }) {
            continue;
        }
        let node = tree
            .file_contexts
            .get(entry.lookup_key())
            .and_then(|context| context.node_id.clone());
        let branch = tree
            .file_contexts
            .get(entry.lookup_key())
            .map(|context| context.branch_id.clone());
        let mut entry = entry.clone();
        let relative = directory::rebase_path(entry.path(), &tree.path, "")?;
        entry.relocate(name_policy::PortablePath::parse(&relative)?);
        result.insert(
            entry.lookup_key().to_owned(),
            OwnedEntry {
                entry,
                node,
                branch,
            },
        );
    }
    Ok(result)
}
/// A child-only working update is not a change to the parent's reviewed index.
/// Accept an older token only through proven ancestry and exact owned contents.
pub(super) fn reviewed_working_tree(
    state: &State,
    expected: &str,
    current: &DirectoryTree,
    direct: &BTreeMap<String, String>,
) -> Result<bool> {
    if expected == current.tree_id {
        return Ok(true);
    }
    let Some(reviewed) = state.directory_trees.get(expected) else {
        return Ok(false);
    };
    if reviewed.path != current.path {
        return Ok(false);
    }
    let mut ancestor = current.parent_tree_id.as_deref();
    while let Some(id) = ancestor {
        if id == expected {
            let ids = direct.keys().map(String::as_str).collect();
            return Ok(owned(reviewed, &ids)? == owned(current, &ids)?);
        }
        ancestor = state.directory_trees[id].parent_tree_id.as_deref();
    }
    Ok(false)
}

fn head_child<'a>(
    state: &'a State,
    index: &DirectoryGitBranch,
    directory: &str,
) -> Option<&'a DirectoryReference> {
    index
        .head_tree_id
        .as_ref()
        .and_then(|id| state.directory_trees[id].directory_refs.get(directory))
}
fn index_child<'a>(index: &'a DirectoryGitBranch, directory: &str) -> Option<&'a str> {
    index
        .index_children
        .get(directory)
        .and_then(|reference| reference.tree_id.as_deref())
}

pub(crate) fn status_for(
    state: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    repository: &str,
    path: &str,
) -> Result<status::ScopeChanges> {
    current_changes(
        state,
        view,
        repository,
        path,
        &HistoryWindowCancellation::new(),
    )
}
pub(crate) use status::{BranchStatuses, current_changes};
pub(super) use status::staged_changes;

impl Engine {
    pub(super) fn directory_git_checkout(
        &mut self,
        input: Value,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            "directory.git.checkout",
            principal,
            operation,
            input.clone(),
            |engine, state, _| {
                let result = apply_command(
                    state,
                    engine.admitted_current_namespaces()?,
                    "directory.git.checkout",
                    &input,
                    principal,
                    operation,
                )?;
                let repository = text(&input, "repository_id")?;
                let main = &state.repositories[repository].main_branch_id;
                if state.commits[&state.branches[main].head_commit_id]
                    .entries
                    .len()
                    > engine.config.max_entries
                {
                    return Err(Error::new(
                        ErrorCode::ResourceLimit,
                        "checkout exceeds the configured namespace entry limit",
                    ));
                }
                Ok(result)
            },
        )
    }

    /// Read the designated main branch's latest saved working composition.
    /// Includes saved, uncommitted edits, even before the first user commit.
    /// This does not follow the current checkout, branch name, index or Git HEAD.
    ///
    /// This is retained metadata, not a physical placement plan or a promise of
    /// body availability. Nested repository snapshots remain in the composition;
    /// a layout consumer must treat their stable identities as ownership borders
    /// and use each child's own main workspace instead of expanding the parent.
    /// No state, selection, receipt, container or working bytes are changed.
    pub fn directory_git_main_workspace(
        &self,
        repository: &str,
        directory: &str,
    ) -> Result<&DirectoryBranch> {
        let git = self
            .state
            .directory_git_repositories
            .get(directory)
            .filter(|git| git.repository_id == repository)
            .ok_or_else(|| required("directory repository is not initialized in this scope"))?;
        Ok(&self.state.directory_branches[&git.main_branch_id])
    }

    pub fn directory_git_status(&self, repository: &str, path: &str) -> Result<DirectoryGitStatus> {
        status::read(
            &self.state,
            self.admitted_current_namespaces()?,
            repository,
            path,
            &HistoryWindowCancellation::new(),
        )
    }
    fn mutate_git_command(
        &mut self,
        command: GitCommand<'_>,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        let input = command.body();
        self.mutate(command.kind(), principal, operation, input.clone(), |engine, state, _| {
            let mut ids = Ids { operation, ordinal: 0 };
            command
                .apply(state, engine.admitted_current_namespaces()?, &mut ids)
                .map(|outcome| outcome.receipt(&input))
        })
    }

    pub fn initialize_directory_git(
        &mut self,
        repository: &str,
        body: &DirectoryGitInitializeBody,
        principal: &str,
    ) -> Result<OperationResult> {
        self.mutate_git_command(
            GitCommand::Initialize {
                repository,
                path: &body.path,
                expected_namespace_head: &body.expected_namespace_head,
            },
            principal,
            body.operation_id,
        )
    }
    pub fn stage_directory_git_child(
        &mut self,
        repository: &str,
        body: &DirectoryGitStageChildBody,
        principal: &str,
    ) -> Result<OperationResult> {
        self.mutate_git_command(
            GitCommand::Stage {
                repository,
                directory: &body.directory_id,
                branch: &body.directory_branch_id,
                expected_generation: body.expected_index_generation,
                selection: IndexSelection::Child {
                    directory: &body.child_directory_id,
                    expected_head: &body.expected_child_head_tree_id,
                },
            },
            principal,
            body.operation_id,
        )
    }
    pub fn stage_directory_git_owned(
        &mut self,
        repository: &str,
        body: &DirectoryGitStageOwnedBody,
        principal: &str,
    ) -> Result<OperationResult> {
        self.mutate_git_command(
            GitCommand::Stage {
                repository,
                directory: &body.directory_id,
                branch: &body.directory_branch_id,
                expected_generation: body.expected_index_generation,
                selection: IndexSelection::Owned { expected_tree: &body.expected_tree_id },
            },
            principal,
            body.operation_id,
        )
    }
}

fn text<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input[key].as_str().ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidRequest,
            format!("directory Git input lacks {key}"),
        )
    })
}
#[derive(Clone, Copy)]
enum IndexSelection<'a> {
    Owned { expected_tree: &'a str },
    Child { directory: &'a str, expected_head: &'a str },
}

#[derive(Clone, Copy)]
enum GitCommand<'a> {
    Initialize {
        repository: &'a str,
        path: &'a str,
        expected_namespace_head: &'a str,
    },
    Stage {
        repository: &'a str,
        directory: &'a str,
        branch: &'a str,
        expected_generation: u64,
        selection: IndexSelection<'a>,
    },
}

enum GitOutcome {
    Initialized { directory: String, branch: String },
    Staged { directory: String, branch: String, generation: u64 },
}
impl GitOutcome {
    fn receipt(self, input: &Value) -> Value {
        match self {
            Self::Initialized { directory, branch } => {
                json!({"input":input,"directory_id":directory,"directory_branch_id":branch,"initialized":true})
            }
            Self::Staged { directory, branch, generation } => {
                json!({"input":input,"directory_id":directory,"directory_branch_id":branch,"index_generation":generation.to_string()})
            }
        }
    }
}
impl GitCommand<'_> {
    fn kind(self) -> &'static str {
        match self {
            Self::Initialize { .. } => "directory.git.initialize",
            Self::Stage { selection: IndexSelection::Owned { .. }, .. } => "directory.git.stage-owned",
            Self::Stage { selection: IndexSelection::Child { .. }, .. } => "directory.git.stage-child",
        }
    }
    fn body(self) -> Value {
        match self {
            Self::Initialize { repository, path, expected_namespace_head } => {
                json!({"repository_id":repository,"path":path,"expected_namespace_head":expected_namespace_head})
            }
            Self::Stage { repository, directory, branch, expected_generation, selection } => match selection {
                IndexSelection::Owned { expected_tree } => {
                    json!({"repository_id":repository,"directory_id":directory,"directory_branch_id":branch,"expected_tree_id":expected_tree,"expected_index_generation":expected_generation.to_string()})
                }
                IndexSelection::Child { directory: child, expected_head } => {
                    json!({"repository_id":repository,"directory_id":directory,"directory_branch_id":branch,"expected_index_generation":expected_generation.to_string(),"child_directory_id":child,"expected_child_head_tree_id":expected_head})
                }
            },
        }
    }
    fn apply(
        self,
        state: &mut State,
        view: Option<&crate::engine::CurrentNamespaces>,
        ids: &mut Ids,
    ) -> Result<GitOutcome> {
        match self {
            Self::Initialize { repository, path, expected_namespace_head } => {
                let entries = current_entries(state, view, repository)?.to_vec();
                let (directory, path) = locate(&entries, repository, path)?;
                let repo = &state.repositories[repository];
                if state.branches[&repo.main_branch_id].head_commit_id != expected_namespace_head {
                    return Err(conflict("working namespace changed before initialization"));
                }
                if state.directory_git_repositories.contains_key(&directory) {
                    return Err(conflict("directory repository is already initialized"));
                }
                let branch = directory::initialize_git_working_scope(
                    state, repository, &path, view, &mut |prefix| ids.next(prefix),
                )?;
                let tree = state.directory_branches[&branch].head_tree_id.clone();
                let index = DirectoryGitBranch {
                    head_tree_id: None,
                    index_tree_id: tree,
                    index_generation: 1,
                    index_boundaries: boundaries(state, repository, &directory, &path, &entries),
                    index_children: BTreeMap::new(),
                };
                state.directory_git_repositories.insert(
                    directory.clone(),
                    DirectoryGitRepository {
                        repository_id: repository.into(),
                        directory_id: directory.clone(),
                        initialized_sequence: state.sequence,
                        main_branch_id: branch.clone(),
                        current_branch_id: branch.clone(),
                        branches: BTreeMap::from([(branch.clone(), index)]),
                        deletions: BTreeMap::new(),
                    },
                );
                // Initializing a child changes its parent's ownership boundary,
                // not its reviewed index contents or committed HEAD.
                for (parent_id, parent) in state.directory_git_repositories.iter_mut() {
                    if parent_id == &directory || parent.repository_id != repository {
                        continue;
                    }
                    if let Some(parent_path) = path_for(&entries, repository, parent_id)
                        && descendant(&name_policy::path_key(&path), &name_policy::path_key(parent_path))
                    {
                        for index in parent.branches.values_mut() {
                            index.index_generation = increment(index.index_generation)?;
                        }
                    }
                }
                Ok(GitOutcome::Initialized { directory, branch })
            }
            Self::Stage { repository, directory, branch, expected_generation, selection } => {
                let (git, index) = active_git(state, repository, branch)?;
                if git.directory_id != directory {
                    return Err(conflict("index belongs to another directory"));
                }
                if index.index_generation != expected_generation {
                    return Err(conflict("directory index changed; refresh before staging"));
                }
                if let Some(view) = view {
                    view.require_directory_branch_write(state, branch)?;
                }
                let working = working_tree(state, git, branch)?.tree_id.clone();
                let entries = current_entries(state, view, repository)?;
                let path = path_for(entries, repository, directory)
                    .ok_or_else(|| Error::new(ErrorCode::DirectoryNotFound, "directory no longer exists"))?
                    .to_owned();
                let direct = boundaries(state, repository, directory, &path, entries);
                match selection {
                    IndexSelection::Child { directory: child, expected_head } => {
                        if !direct.contains_key(child) {
                            return Err(conflict("only a direct independent child repository can be staged"));
                        }
                        let child_git = &state.directory_git_repositories[child];
                        let head = child_git.branches[&child_git.current_branch_id].head_tree_id.as_ref()
                            .ok_or_else(|| conflict("commit the child repository before staging its reference"))?;
                        if head != expected_head {
                            return Err(conflict("child HEAD changed before staging"));
                        }
                        let reference = DirectoryGitChildReference {
                            directory_branch_id: state.directory_trees[head].directory_branch_id.clone(),
                            tree_id: Some(head.clone()),
                        };
                        let index = state.directory_git_repositories
                            .get_mut(directory).expect("invariant: staging a validated parent repository")
                            .branches.get_mut(branch).expect("invariant: staging its current branch");
                        index.index_children.insert(child.into(), reference);
                        index.index_generation = increment(index.index_generation)?;
                    }
                    IndexSelection::Owned { expected_tree } => {
                        if working != expected_tree {
                            return Err(conflict("working directory changed before staging"));
                        }
                        directory::initialize_git_working_scope(
                            state, repository, &path, view, &mut |prefix| ids.next(prefix),
                        )?;
                        let working = state.directory_branches[branch].head_tree_id.clone();
                        let index = state.directory_git_repositories
                            .get_mut(directory).expect("invariant: staging an initialized repository")
                            .branches.get_mut(branch).expect("invariant: staging its active index");
                        index.index_children.retain(|id, _| direct.contains_key(id));
                        index.index_boundaries = direct;
                        index.index_tree_id = working;
                        index.index_generation = increment(index.index_generation)?;
                    }
                }
                Ok(GitOutcome::Staged {
                    directory: directory.into(),
                    branch: branch.into(),
                    generation: state.directory_git_repositories[directory].branches[branch].index_generation,
                })
            }
        }
    }
}

/// Decode stored operation input only at the replay/publication boundary.
/// Normal typed calls use the same command without serializing to parse again.
pub(crate) fn apply_command(
    state: &mut State,
    view: Option<&crate::engine::CurrentNamespaces>,
    kind: &str,
    input: &Value,
    principal: &str,
    operation: OperationId,
) -> Result<Value> {
    let repository = text(input, "repository_id")?;
    let mut ids = Ids { operation, ordinal: 0 };
    let command = match kind {
        "directory.git.initialize" => GitCommand::Initialize {
            repository,
            path: text(input, "path")?,
            expected_namespace_head: text(input, "expected_namespace_head")?,
        },
        "directory.git.stage-child" | "directory.git.stage-owned" => GitCommand::Stage {
            repository,
            directory: text(input, "directory_id")?,
            branch: text(input, "directory_branch_id")?,
            expected_generation: storage_contract::parse_u64_decimal(text(input, "expected_index_generation")?)?,
            selection: if kind == "directory.git.stage-child" {
                IndexSelection::Child {
                    directory: text(input, "child_directory_id")?,
                    expected_head: text(input, "expected_child_head_tree_id")?,
                }
            } else {
                IndexSelection::Owned { expected_tree: text(input, "expected_tree_id")? }
            },
        },
        "directory.git.branch-create" => return branches::create(state, view, input, &mut ids),
        "directory.git.checkout" => return branches::checkout(state, view, input, principal, operation, &mut ids),
        _ => return Err(Error::new(ErrorCode::InvalidRequest, "unknown directory Git operation")),
    };
    command.apply(state, view, &mut ids).map(|outcome| outcome.receipt(input))
}
