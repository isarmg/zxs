//! Explicit commits freeze exactly one manually initialized directory repository.
//! Working paths remain live; independently initialized children are pinned refs.
use super::*;
use data_serialization::Deserialize;
use json_codec::{Value, json};
use std::collections::BTreeMap;
use storage_contract::directory_staging::DirectoryStagingStatus;

pub struct DirectoryCommitRequest<'a> {
    pub repository_id: &'a str,
    pub directory_branch_id: &'a str,
    pub expected_tree_id: &'a str,
    pub expected_index_generation: u64,
    pub message: &'a str,
    pub principal: &'a str,
    pub operation_id: OperationId,
}
impl Engine {
    pub fn directory_staging_status(
        &self,
        repository: &str,
        id: &str,
    ) -> Result<DirectoryStagingStatus> {
        let (git, index) = directory_git::active_git(&self.state, repository, id)?;
        let tree = directory_git::working_tree(&self.state, git, id)?;
        let status = directory_git::status_for(
            &self.state,
            self.admitted_current_namespaces()?,
            repository,
            &tree.path,
        )?;
        Ok(DirectoryStagingStatus {
            repository_id: repository.into(),
            directory_branch_id: id.into(),
            head_tree_id: tree.tree_id.clone(),
            committed_tree_id: index.head_tree_id.clone(),
            staged: status.staged || status.needs_restage,
            added: status.added,
            modified: status.modified,
            deleted: status.deleted,
        })
    }
    pub fn commit_directory_changes(
        &mut self,
        request: DirectoryCommitRequest<'_>,
    ) -> Result<OperationResult> {
        let input = CommitInput {
            repository_id: request.repository_id.into(),
            directory_branch_id: request.directory_branch_id.into(),
            expected_tree_id: request.expected_tree_id.into(),
            expected_index_generation: request.expected_index_generation,
            message: request.message.into(),
        };
        input.validate()?;
        self.mutate(
            "directory.changes.commit",
            request.principal,
            request.operation_id,
            input.body(),
            |engine, state, _| {
                apply_commit(
                    state,
                    engine.admitted_current_namespaces()?,
                    &input,
                    request.principal,
                    request.operation_id,
                )
            },
        )
    }
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct CommitInput {
    pub repository_id: String,
    pub directory_branch_id: String,
    pub expected_tree_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub expected_index_generation: u64,
    pub message: String,
}
impl CommitInput {
    pub(crate) fn validate(&self) -> Result<()> {
        storage_contract::validate_note(&self.message)?;
        if self.message.trim().is_empty() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "directory commit message must be nonempty",
            ));
        }
        Ok(())
    }
    pub(crate) fn body(&self) -> Value {
        json!({"repository_id":self.repository_id,"directory_branch_id":self.directory_branch_id,"expected_tree_id":self.expected_tree_id,"expected_index_generation":self.expected_index_generation.to_string(),"message":self.message})
    }
}
/// Stable domain-separated IDs make the entire metadata mutation reproducible
/// from its authenticated request without randomness or payload reads.
pub(super) struct Ids {
    pub(super) operation: OperationId,
    pub(super) ordinal: u64,
}
impl Ids {
    pub(super) fn next(&mut self, prefix: &str) -> Result<String> {
        let hash = crate::object_hash(
            "directory-commit-id",
            &json!({"operation_id":self.operation,"ordinal":self.ordinal.to_string(),"kind":prefix}),
        )?;
        self.ordinal = self.ordinal.checked_add(1).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "directory commit identity budget exhausted",
            )
        })?;
        Ok(format!("{prefix}_{}", &hash[..32]))
    }
}

/// Freeze the owned files plus the exact child commits reviewed by the user.
/// This never moves any working branch, child HEAD, or namespace selection.
fn frozen_tree(
    state: &State,
    working: &DirectoryTree,
    index: &storage_contract::directory_git::DirectoryGitBranch,
    direct: &BTreeMap<String, String>,
) -> Result<DirectoryTree> {
    let boundaries = direct
        .values()
        .map(|path| name_policy::path_key(path))
        .collect::<std::collections::BTreeSet<_>>();
    let mut mounts = BTreeMap::new();
    let mut entries = Vec::new();
    let mut owned_paths = std::collections::BTreeSet::new();
    for entry in &working.entries {
        if entry.kind == NamespaceEntryKind::Directory && direct.contains_key(&entry.entry_id) {
            mounts.insert(entry.entry_id.as_str(), entry);
        }
        if namespace::closest_directory_repository(entry.lookup_key(), |key| {
            boundaries.get(key).map(String::as_str)
        })
        .is_none()
        {
            owned_paths.insert(entry.lookup_key());
            entries.push(entry.clone());
        }
    }
    let file_contexts = working
        .file_contexts
        .iter()
        .filter(|(key, _)| owned_paths.contains(key.as_str()))
        .map(|(key, context)| (key.clone(), context.clone()))
        .collect();
    // Copy only the parent's owned entries. Child contents below are taken
    // from the explicitly staged commits, not cloned then discarded here.
    let mut frozen = DirectoryTree {
        tree_id: working.tree_id.clone(),
        directory_branch_id: working.directory_branch_id.clone(),
        parent_tree_id: working.parent_tree_id.clone(),
        commit_id: working.commit_id.clone(),
        path: working.path.clone(),
        entries,
        file_contexts,
        file_branches: working.file_branches.clone(),
        inherited_files: working.inherited_files.clone(),
        directory_refs: BTreeMap::new(),
        sequence: working.sequence,
    };
    for (directory, path) in direct {
        let Some(reference) = index.index_children.get(directory) else {
            continue;
        };
        let Some(tree_id) = &reference.tree_id else {
            continue;
        };
        let child = &state.directory_trees[tree_id];
        let mount = mounts.get(directory.as_str()).copied().ok_or_else(|| {
            Error::new(
                ErrorCode::StateInvalid,
                "independent child mount is absent from its working tree",
            )
        })?;
        frozen.entries.push(mount.clone());
        let rebased = directory::rebase_entries(&child.entries, &child.path, path)?;
        for (original, entry) in child.entries.iter().zip(&rebased) {
            if entry.kind == NamespaceEntryKind::File {
                frozen.file_contexts.insert(
                    entry.lookup_key().into(),
                    child.file_contexts[original.lookup_key()].clone(),
                );
            }
        }
        frozen.entries.extend(rebased);
        frozen.file_branches.extend(child.file_branches.clone());
        frozen.inherited_files.extend(child.inherited_files.clone());
        frozen.directory_refs.insert(
            directory.clone(),
            DirectoryReference {
                directory_branch_id: reference.directory_branch_id.clone(),
                tree_id: tree_id.clone(),
            },
        );
    }
    frozen
        .entries
        .sort_by(|a, b| a.lookup_key().cmp(b.lookup_key()));
    let files = frozen
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| entry.file_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    frozen
        .file_branches
        .retain(|file, _| files.contains(file.as_str()));
    frozen
        .inherited_files
        .retain(|file, _| files.contains(file.as_str()));
    Ok(frozen)
}

pub(crate) fn apply_commit(
    state: &mut State,
    view: Option<&crate::engine::CurrentNamespaces>,
    input: &CommitInput,
    principal: &str,
    operation: OperationId,
) -> Result<Value> {
    input.validate()?;
    let (git, index) =
        directory_git::active_git(state, &input.repository_id, &input.directory_branch_id)?;
    let directory = git.directory_id.clone();
    let working = directory_git::working_tree(state, git, &input.directory_branch_id)?;
    let entries = directory_git::current_entries(state, view, &input.repository_id)?;
    let (live_directory, _) = directory_git::locate(entries, &input.repository_id, &working.path)?;
    if live_directory != directory {
        return Err(directory_git::conflict(
            "the directory path now belongs to a different repository identity",
        ));
    }
    let direct = directory_git::boundaries(
        state,
        &input.repository_id,
        &directory,
        &working.path,
        entries,
    );
    if !directory_git::reviewed_working_tree(state, &input.expected_tree_id, working, &direct)
        || index.index_generation != input.expected_index_generation
    {
        return Err(directory_git::conflict(
            "directory working tree or index changed; review before committing",
        ));
    }
    let status =
        directory_git::staged_changes(state, view, git, &input.directory_branch_id, &direct)?;
    if status.needs_restage {
        return Err(directory_git::conflict(
            "independent child ownership changed; stage this repository again before committing",
        ));
    }
    if let Some(view) = view {
        view.require_directory_branch_write(state, &input.directory_branch_id)?;
        view.directory_projection(state, &input.directory_branch_id)?
            .require_commit_source(state)?;
    }
    if !status.staged {
        let head = index
            .head_tree_id
            .as_ref()
            .expect("invariant: an unborn repository is staged");
        let tree = &state.directory_trees[head];
        return Ok(
            json!({"input":input.body(),"directory_branch_id":input.directory_branch_id,"head_tree_id":head,"commit_id":tree.commit_id,"no_change":true,"directory_branch_ids":[]}),
        );
    }
    let staged = &state.directory_trees[&index.index_tree_id];
    if let Some(view) = view {
        view.require_staged_commit_source(state, staged, &index.index_boundaries)?;
    } else if directory_git::staged_owned_files(staged, &index.index_boundaries)
        .any(|entry| state.content_states.get(&entry.version_id) != Some(&ContentState::Available))
    {
        return Err(directory_git::conflict(
            "staged content is unavailable; stage the working contents again",
        ));
    }
    let mut tree = frozen_tree(state, staged, index, &direct)?;
    let previous_head = index.head_tree_id.clone();
    let next_generation = index.index_generation.checked_add(1).ok_or_else(|| {
        Error::new(
            ErrorCode::ResourceLimit,
            "directory index generation exhausted",
        )
    })?;
    let mut ids = Ids {
        operation,
        ordinal: 0,
    };
    let commit_id = ids.next("commit")?;
    let tree_id = ids.next("dtree")?;
    let mut commit = state.commits[&staged.commit_id].clone();
    let key = name_policy::path_key(&staged.path);
    commit
        .entries
        .retain(|entry| !key.is_empty() && !namespace::is_descendant(entry.lookup_key(), &key));
    commit.entries.extend(tree.entries.clone());
    commit
        .entries
        .sort_by(|a, b| a.lookup_key().cmp(b.lookup_key()));
    commit.pending_file_versions.retain(|file, version| {
        commit
            .entries
            .iter()
            .any(|entry| &entry.file_id == file && &entry.version_id == version)
    });
    commit.commit_id = commit_id.clone();
    commit.kind = "directory_commit".into();
    commit.parent_commit_ids = previous_head
        .iter()
        .map(|head| state.directory_trees[head].commit_id.clone())
        .collect();
    commit.tree_id = ids.next("tree")?;
    commit.sequence = state.sequence;
    commit.operation_id = operation;
    commit.author = principal.into();
    commit.message = input.message.clone();
    commit.namespace_deletions = None;
    commit.provenance = Some(tree_id.clone());
    if state.commits.contains_key(&commit_id) || state.directory_trees.contains_key(&tree_id) {
        return Err(Error::new(
            ErrorCode::StateInvalid,
            "directory commit identity already exists",
        ));
    }
    tree.tree_id = tree_id.clone();
    tree.directory_branch_id = input.directory_branch_id.clone();
    tree.parent_tree_id = previous_head;
    tree.commit_id = commit_id.clone();
    tree.sequence = state.sequence;
    state.commits.insert(commit_id.clone(), commit);
    state.directory_trees.insert(tree_id.clone(), tree);
    let index = state
        .directory_git_repositories
        .get_mut(&directory)
        .expect("invariant: committing an initialized directory")
        .branches
        .get_mut(&input.directory_branch_id)
        .expect("invariant: active Git branch has an index");
    index.head_tree_id = Some(tree_id.clone());
    index.index_generation = next_generation;
    Ok(
        json!({"input":input.body(),"directory_branch_id":input.directory_branch_id,"head_tree_id":tree_id,"commit_id":commit_id,"no_change":false,"directory_branch_ids":[input.directory_branch_id]}),
    )
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/directory_staging_tests.rs"]
mod tests;
