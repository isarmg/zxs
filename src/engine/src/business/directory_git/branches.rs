//! Git branch registration and checkout preserve independent child workspaces.
use crate::error::HistoricalBranchError;
use super::*;
use storage_contract::directory_git::HistoricalDirectoryBranchSource;

pub(crate) const HISTORY_CREATE_KIND: &str = "directory.git.branch-create-history";

pub(crate) struct HistoricalBranchRegistration<'a> {
    pub(crate) branch: &'a DirectoryBranch,
    pub(crate) repository: &'a Repository,
}

pub(crate) struct HistoricalBranchCommand<'a> {
    pub(crate) source: &'a HistoricalDirectoryBranchSource,
    pub(crate) path: Option<&'a name_policy::LookupPath>,
    pub(crate) name: &'a storage_contract::BranchName,
}

impl HistoricalBranchCommand<'_> {
    pub(crate) fn apply<'a>(
        &self,
        state: &'a mut State,
        view: Option<&crate::engine::CurrentNamespaces>,
        operation: OperationId,
    ) -> Result<HistoricalBranchRegistration<'a>> {
        let source = self.source.current();
        let status = status_for(
            state,
            view,
            &source.repository_id,
            self.path.map_or("", name_policy::LookupPath::as_str),
        )?;
        if !status.initialized {
            return Err(HistoricalBranchError::Uninitialized.into());
        }
        if status.staged || status.unstaged || status.needs_restage {
            return Err(HistoricalBranchError::Dirty.into());
        }
        let git = &state.directory_git_repositories[&status.directory_id];
        require_reviewed_source(state, &source.repository_id, git, source)?;
        if git.branches.keys().any(|id| {
            name_policy::lookup_key(&state.directory_branches[id].name) == self.name.key()
        }) {
            return Err(HistoricalBranchError::NameCollision.into());
        }
        if let Some(view) = view {
            view.require_directory_branch_write(state, &git.current_branch_id)?;
            view.directory_projection(state, &git.current_branch_id)?
                .require_commit_source(state)?;
        }
        let tree = self.committed_source(state, git)?;
        let tree = match view {
            Some(view) => view.historical_directory_source(state, tree)?,
            None => tree,
        };
        let id = crate::persist::directory_branch::branch_id(operation)?;
        let branch = self.inherited_branch(state, tree, id);
        let index = historical_index(tree);
        state.directory_branches.insert(id.to_string(), branch);
        state
            .directory_git_repositories
            .get_mut(&status.directory_id)
            .expect("invariant: registration preserves the initialized directory repository")
            .branches
            .insert(id.to_string(), index);
        Ok(HistoricalBranchRegistration {
            branch: &state.directory_branches[&id.to_string()],
            repository: &state.repositories[&source.repository_id],
        })
    }

    fn committed_source<'a>(
        &self,
        state: &'a State,
        git: &DirectoryGitRepository,
    ) -> Result<&'a DirectoryTree> {
        let tree = state
            .directory_trees
            .get(&self.source.tree_id().to_string())
            .ok_or(HistoricalBranchError::Missing(self.source.tree_id()))?;
        let owner = &state.directory_branches[&tree.directory_branch_id];
        if owner.repository_id != git.repository_id || owner.directory_id != git.directory_id {
            return Err(HistoricalBranchError::Scope.into());
        }
        if !storage_contract::directory_staging::directory_tree_is_committed(state, tree) {
            return Err(HistoricalBranchError::Uncommitted.into());
        }
        let children =
            ChildBoundaries::new(&tree.entries, |id| tree.directory_refs.contains_key(id));
        for entry in tree.entries.iter().filter(|entry| {
            entry.kind == NamespaceEntryKind::File && !children.contains(entry.lookup_key())
        }) {
            let context = &tree.file_contexts[entry.lookup_key()];
            if state.branches[&context.branch_id].state != BranchState::Active
                || state.branches[&tree.file_branches[&entry.file_id]].state != BranchState::Active
            {
                return Err(HistoricalBranchError::Unavailable.into());
            }
        }
        Ok(tree)
    }

    fn inherited_branch(
        &self,
        state: &State,
        tree: &DirectoryTree,
        id: storage_contract::DirectoryBranchId,
    ) -> DirectoryBranch {
        let owner = &state.directory_branches[&tree.directory_branch_id];
        let mut contexts = tree.file_contexts.clone();
        for context in contexts.values_mut() {
            context.selection_generation = 0;
            context.inherited_from = Some(id.to_string());
        }
        DirectoryBranch {
            directory_branch_id: id.to_string(),
            repository_id: owner.repository_id.clone(),
            directory_id: owner.directory_id.clone(),
            path: tree.path.clone(),
            name: self.name.as_str().into(),
            parent_branch_id: Some(tree.directory_branch_id.clone()),
            head_tree_id: tree.tree_id.clone(),
            commit_id: tree.commit_id.clone(),
            entries: tree.entries.clone(),
            file_branches: tree.file_branches.clone(),
            file_contexts: contexts,
            inherited_files: tree
                .file_branches
                .keys()
                .map(|file| (file.clone(), id.to_string()))
                .collect(),
            directory_refs: tree.directory_refs.clone(),
            materialized: false,
            sequence: state.sequence,
        }
    }
}

fn historical_index(tree: &DirectoryTree) -> DirectoryGitBranch {
    DirectoryGitBranch {
        head_tree_id: Some(tree.tree_id.clone()),
        index_tree_id: tree.tree_id.clone(),
        index_generation: 1,
        index_boundaries: tree
            .entries
            .iter()
            .filter(|entry| {
                entry.kind == NamespaceEntryKind::Directory
                    && tree.directory_refs.contains_key(&entry.entry_id)
            })
            .map(|entry| (entry.entry_id.clone(), entry.path().into()))
            .collect(),
        index_children: tree
            .directory_refs
            .iter()
            .map(|(directory, reference)| {
                (
                    directory.clone(),
                    DirectoryGitChildReference {
                        directory_branch_id: reference.directory_branch_id.clone(),
                        tree_id: Some(reference.tree_id.clone()),
                    },
                )
            })
            .collect(),
    }
}

pub(super) fn create(
    state: &mut State,
    view: Option<&crate::engine::CurrentNamespaces>,
    input: &Value,
    ids: &mut Ids,
) -> Result<Value> {
    let repository = text(input, "repository_id")?;
    let path = text(input, "path")?;
    let name = storage_contract::BranchName::parse(text(input, "name")?)?;
    let status = status_for(state, view, repository, path)?;
    if !status.initialized {
        return Err(required(
            "initialize this directory repository before creating branches",
        ));
    }
    if status.staged || status.unstaged || status.needs_restage {
        return Err(conflict(
            "stage and commit this repository's changes before creating a branch",
        ));
    }
    let git = &state.directory_git_repositories[&status.directory_id];
    let current = &git.current_branch_id;
    if let Some(source) = input.get("source") {
        let source: DirectoryBranchCreateSource = json_codec::from_value(source.clone())?;
        source.validate()?;
        require_reviewed_source(state, repository, git, &source)?;
    }
    if git
        .branches
        .keys()
        .any(|id| name_policy::lookup_key(&state.directory_branches[id].name) == name.key())
    {
        return Err(Error::new(
            ErrorCode::NameCollision,
            "directory branch name already exists",
        ));
    }
    if let Some(view) = view {
        view.require_directory_branch_write(state, current)?;
        view.directory_projection(state, current)?
            .require_commit_source(state)?;
    }
    let source = &state.directory_branches[current];
    require_source(state, source)?;
    let id = ids.next("dbr")?;
    let mut branch = source.clone();
    branch.directory_branch_id = id.clone();
    branch.parent_branch_id = Some(current.clone());
    branch.name = name.as_str().into();
    branch.materialized = false;
    branch.sequence = state.sequence;
    for context in branch.file_contexts.values_mut() {
        context.selection_generation = 0;
        context.inherited_from = Some(id.clone());
    }
    branch.inherited_files = branch
        .file_branches
        .keys()
        .map(|file| (file.clone(), id.clone()))
        .collect();
    let mut index = git.branches[current].clone();
    index.index_tree_id = source.head_tree_id.clone();
    index.index_generation = 1;
    let directory = git.directory_id.clone();
    let tree = branch.head_tree_id.clone();
    let files=branch.entries.iter().filter(|entry|entry.kind==NamespaceEntryKind::File).map(|entry|json!({"file_id":entry.file_id,"branch_id":branch.file_branches[&entry.file_id],"path":entry.path(),"version_id":entry.version_id})).collect::<Vec<_>>();
    let count = files.len();
    state.directory_branches.insert(id.clone(), branch);
    state
        .directory_git_repositories
        .get_mut(&directory)
        .expect("invariant: initialized repository")
        .branches
        .insert(id.clone(), index);
    Ok(
        json!({"input":input,"directory_branch_id":id,"history_tree_id":state.repositories[repository].history_tree_id,"path":path,"name":name.as_str(),"head_tree_id":tree,"count":count.to_string(),"branches":files,"copied_files":"0","placeholder_files":"0","materialized":false,"selected":false}),
    )
}

/// Used by both live planning and Current's cold, closed-write-set replay.
/// The native instantaneous command omits a reviewed source; HTTP never does.
fn require_reviewed_source(
    state: &State,
    repository: &str,
    git: &DirectoryGitRepository,
    source: &DirectoryBranchCreateSource,
) -> Result<()> {
    let DirectoryBranchCreateSource {
        repository_id,
        directory_id,
        directory_branch_id,
        working_tree_id,
        head_tree_id,
        index_generation,
        namespace_head,
    } = source;
    let current = &git.current_branch_id;
    let branch = &state.directory_branches[current];
    let index = &git.branches[current];
    let repo = &state.repositories[repository];
    if repository_id != repository
        || directory_id != &git.directory_id
        || directory_branch_id != current
        || state.directory_selections.get(directory_id) != Some(current)
        || working_tree_id != &branch.head_tree_id
        || index.head_tree_id.as_ref() != Some(head_tree_id)
        || index_generation != &index.index_generation
        || namespace_head != &state.branches[&repo.main_branch_id].head_commit_id
    {
        return Err(conflict(
            "reviewed directory branch source changed; refresh before creating a branch",
        ));
    }
    Ok(())
}

fn require_source(state: &State, source: &DirectoryBranch) -> Result<()> {
    let missing = |message| Error::new(ErrorCode::StateInvalid, message);
    state
        .directory_trees
        .get(&source.head_tree_id)
        .ok_or_else(|| missing("selected directory head tree missing"))?;
    state
        .commits
        .get(&source.commit_id)
        .ok_or_else(|| missing("selected directory commit missing"))?;
    for reference in source.directory_refs.values() {
        state
            .directory_branches
            .get(&reference.directory_branch_id)
            .ok_or_else(|| missing("snapshot directory branch missing"))?;
        state
            .directory_trees
            .get(&reference.tree_id)
            .ok_or_else(|| missing("snapshot directory tree missing"))?;
    }
    let independent = ChildBoundaries::new(&source.entries, |id| {
        state.directory_git_repositories.contains_key(id)
    });
    for entry in &source.entries {
        if entry.kind != NamespaceEntryKind::File {
            continue;
        }
        let owner = source
            .file_branches
            .get(&entry.file_id)
            .ok_or_else(|| missing("snapshot file branch reference missing"))?;
        let context = source
            .file_contexts
            .get(entry.lookup_key())
            .ok_or_else(|| missing("snapshot file path context missing"))?;
        // Child contents are retained historical proof, never reactivated by
        // this parent. Their branch may have been archived independently.
        let owned = !independent.contains(entry.lookup_key());
        for branch in [owner, &context.branch_id] {
            if !state.branches.get(branch).is_some_and(|branch| {
                branch.file_id.as_deref() == Some(entry.file_id.as_str())
                    && (!owned || branch.state == BranchState::Active)
            }) {
                return Err(Error::new(
                    ErrorCode::FileBranchNotFound,
                    "snapshot references an unavailable public file branch",
                ));
            }
        }
    }
    Ok(())
}

struct RestoredContext<'a> {
    file_id: &'a str,
    context: FileContext,
}

pub(super) fn checkout(
    state: &mut State,
    view: Option<&crate::engine::CurrentNamespaces>,
    input: &Value,
    principal: &str,
    operation: OperationId,
    ids: &mut Ids,
) -> Result<Value> {
    let repository = text(input, "repository_id")?;
    let selected_owned = match input.get("directory_branch_id").and_then(Value::as_str) {
        Some(id) => id.to_owned(),
        None => {
            let entries = current_entries(state, view, repository)?;
            let (directory, _) = locate(entries, repository, text(input, "path")?)?;
            let name = storage_contract::BranchName::parse(text(input, "name")?)?;
            state
                .directory_branches
                .values()
                .find(|branch| {
                    branch.directory_id == directory
                        && name_policy::lookup_key(&branch.name) == name.key()
                })
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::DirectoryBranchNotFound,
                        "directory branch missing",
                    )
                })?
                .directory_branch_id
                .clone()
        }
    };
    let selected_id = selected_owned.as_str();
    let selected = state
        .directory_branches
        .get(selected_id)
        .filter(|branch| branch.repository_id == repository)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::DirectoryBranchNotFound,
                "directory branch missing",
            )
        })?
        .clone();
    if input.get("root_only").and_then(Value::as_bool) == Some(true)
        && selected.directory_id != repository
    {
        return Err(Error::new(
            ErrorCode::DirectoryBranchScope,
            "default requires a root directory repository",
        ));
    }
    let git = state
        .directory_git_repositories
        .get(&selected.directory_id)
        .ok_or_else(|| {
            required("initialize this directory repository before checking out branches")
        })?;
    if !git.branches.contains_key(selected_id) {
        return Err(required(
            "branch is not registered in this directory repository",
        ));
    }
    let entries = current_entries(state, view, repository)?.to_vec();
    let path = path_for(&entries, repository, &selected.directory_id)
        .ok_or_else(|| Error::new(ErrorCode::DirectoryNotFound, "directory no longer exists"))?
        .to_owned();
    let key = name_policy::path_key(&path);
    let status = status_for(state, view, repository, &path)?;
    let repo = state.repositories[repository].clone();
    let main = state.branches[&repo.main_branch_id].clone();
    if let Some(view) = view {
        view.require_branch_write(state, &repo.main_branch_id)?;
    }
    if let Some(expected) = input.get("expected_head").and_then(Value::as_str)
        && expected != main.head_commit_id
    {
        return Err(Error::new(
            ErrorCode::ListingChanged,
            "default namespace changed before checkout",
        ));
    }
    if let Some(expected) = input.get("expected_tree_id").and_then(Value::as_str)
        && expected != selected.head_tree_id
    {
        return Err(Error::new(
            ErrorCode::ListingChanged,
            "selected branch changed before checkout",
        ));
    }
    let direct = boundaries(state, repository, &selected.directory_id, &path, &entries);
    let children = ChildBoundaries::new(&entries, |id| direct.contains_key(id));
    let mut source = match view {
        Some(view) => {
            view.require_directory_branch_write(state, selected_id)?;
            view.directory_checkout_projection(state, selected_id, &direct)?
                .raw_branch()
                .clone()
        }
        None => selected.clone(),
    };
    require_source(state, &source)?;
    namespace::container_name::normalize_directory_branch(state, &mut source)?;
    if git.current_branch_id == selected_id {
        return Ok(
            json!({"input":input,"directory_branch_id":selected_id,"branch_id":repo.main_branch_id,"commit_id":main.head_commit_id,"no_change":true}),
        );
    }
    if status.staged || status.unstaged || status.needs_restage {
        return Err(conflict(
            "stage and commit this repository's changes before checkout; child working changes may remain",
        ));
    }
    // A nested independent repository keeps the live ordinary directories that
    // make its mount reachable, even if the selected parent predates them.
    let ancestors = entries
        .iter()
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && descendant(entry.lookup_key(), &key)
                && children.has_descendant(entry.lookup_key())
        })
        .map(|entry| (entry.lookup_key(), entry))
        .collect::<BTreeMap<_, _>>();
    let historical_children = ChildBoundaries::new(&source.entries, |id| direct.contains_key(id));
    let mut restored = Vec::new();
    let mut contexts = BTreeMap::new();
    for entry in &source.entries {
        if historical_children.contains(entry.lookup_key()) {
            continue;
        }
        if entry.kind == NamespaceEntryKind::Directory
            && state
                .directory_git_repositories
                .contains_key(&entry.entry_id)
        {
            return Err(conflict(
                "checkout cannot restore an independent child repository absent from this live scope",
            ));
        }
        let mut target = entry.clone();
        target.relocate(name_policy::PortablePath::parse(&directory::rebase_path(
            entry.path(),
            &source.path,
            &path,
        )?)?);
        if children.contains(target.lookup_key()) {
            continue;
        }
        if let Some(ancestor) = ancestors.get(target.lookup_key()) {
            if target.kind != NamespaceEntryKind::Directory || target.entry_id != ancestor.entry_id
            {
                return Err(conflict(
                    "checkout would replace an ancestor of an independent child repository",
                ));
            }
            continue;
        }
        if target.kind == NamespaceEntryKind::File {
            // A frozen tree chooses identity/content, never the current container name.
            let names = namespace::container_name::current_names(
                state, std::slice::from_ref(&target),
            )?;
            namespace::container_name::apply_file_names(
                std::slice::from_mut(&mut target), &names,
            )?;
            let context = source.file_contexts[entry.lookup_key()].clone();
            if state.branches[&context.branch_id].state != BranchState::Active {
                return Err(Error::new(
                    ErrorCode::FileBranchNotFound,
                    "checkout references an unavailable file branch",
                ));
            }
            contexts.insert(
                directory::context_key_from_key(repository, target.lookup_key()),
                RestoredContext {
                    file_id: &entry.file_id,
                    context,
                },
            );
        }
        restored.push(target);
    }
    // Preserve mounts, their ancestors, and all live descendants, regardless of
    // the parent's historical child pin.
    restored.extend(
        entries
            .iter()
            .filter(|entry| {
                children.contains(entry.lookup_key()) || ancestors.contains_key(entry.lookup_key())
            })
            .cloned(),
    );
    let old_owned = entries
        .iter()
        .filter(|entry| {
            descendant(entry.lookup_key(), &key)
                && !children.contains(entry.lookup_key())
                && !ancestors.contains_key(entry.lookup_key())
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut next_entries = entries
        .iter()
        .filter(|entry| !descendant(entry.lookup_key(), &key))
        .cloned()
        .collect::<Vec<_>>();
    next_entries.extend(restored);
    namespace::validate_entries(&mut next_entries, usize::MAX)?;
    if let Some(view) = view {
        view.remove_hidden_contexts(state, repository, &repo.main_branch_id);
    }
    for entry in &old_owned {
        if entry.kind == NamespaceEntryKind::File {
            state.file_contexts.remove(&directory::context_key_from_key(
                repository,
                entry.lookup_key(),
            ));
        } else {
            state.directory_selections.remove(&entry.entry_id);
            state.directory_tree_selections.remove(&entry.entry_id);
        }
    }
    for (key, RestoredContext { file_id, context }) in contexts {
        state
            .file_selections
            .insert(file_id.to_owned(), context.branch_id.clone());
        match &context.inherited_from {
            Some(origin) => {
                state
                    .file_write_origins
                    .insert(file_id.to_owned(), origin.clone());
            }
            None => {
                state.file_write_origins.remove(file_id);
            }
        }
        state.file_contexts.insert(key, context);
    }
    let commit_id = ids.next("commit")?;
    let mut commit = state.commits[&main.head_commit_id].clone();
    commit.commit_id = commit_id.clone();
    commit.tree_id = ids.next("tree")?;
    commit.parent_commit_ids = vec![main.head_commit_id];
    commit.entries = next_entries;
    commit.kind = "directory_git_checkout".into();
    commit.provenance = Some(selected.commit_id.clone());
    commit.namespace_deletions = None;
    commit.sequence = state.sequence;
    commit.operation_id = operation;
    commit.author = principal.into();
    commit.message = format!("Check out directory branch {}", selected.name);
    commit.pending_file_versions = directory::snapshot_pending(state, &commit.entries);
    state.commits.insert(commit_id.clone(), commit);
    let main = state
        .branches
        .get_mut(&repo.main_branch_id)
        .expect("invariant: admitted main branch");
    main.head_commit_id = commit_id.clone();
    main.generation = increment(main.generation)?;
    let previous_id = state.directory_git_repositories[&selected.directory_id]
        .current_branch_id
        .clone();
    let previous_tree = state.directory_branches[&previous_id].head_tree_id.clone();
    let git = state
        .directory_git_repositories
        .get_mut(&selected.directory_id)
        .expect("invariant: initialized repository");
    let old = git
        .branches
        .get_mut(&previous_id)
        .expect("invariant: current branch index");
    old.index_tree_id = previous_tree;
    old.index_generation = increment(old.index_generation)?;
    git.current_branch_id = selected_id.into();
    let index = git
        .branches
        .get_mut(selected_id)
        .expect("invariant: target index");
    index.index_generation = increment(index.index_generation)?;
    state
        .directory_selections
        .insert(selected.directory_id.clone(), selected_id.into());
    state
        .directory_tree_selections
        .insert(selected.directory_id.clone(), selected.head_tree_id.clone());
    let generation = state
        .directory_selection_generations
        .entry(selected.directory_id.clone())
        .or_default();
    *generation = increment(*generation)?;
    // The new namespace is already the admitted splice. Rebuild working-only
    // compositions from it; independently initialized child HEADs remain intact.
    directory::initialize_git_working_scope(state, repository, "", None, &mut |prefix| {
        ids.next(prefix)
    })?;
    Ok(
        json!({"input":input,"directory_branch_id":selected_id,"branch_id":repo.main_branch_id,"commit_id":commit_id,"history_tree_id":repo.history_tree_id,"path":path,"name":selected.name,"selected":true,"no_change":false}),
    )
}
