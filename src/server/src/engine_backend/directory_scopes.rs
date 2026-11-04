//! Shared borrowed filtering, followed by the requested owned read projection.
use super::*;
use crate::{
    DirectoryBranchChoice, DirectoryBranchProjection, DirectoryScopeBranch, DirectoryScopeRows,
    DirectoryScopesSnapshot, ReadDirectoryScopes,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn matching<'a>(
    engine: &'a Engine,
    repository: &'a Repository,
    requested_key: Option<&'a str>,
) -> Result<Vec<&'a storage_contract::DirectoryBranch>, ApiError> {
    Ok(matching_tables(
        engine
            .namespace_tree(&repository.main_branch_id)
            .map_err(error)?,
        &engine.state().directory_branches,
        repository,
        requested_key,
    ))
}

// Shared predicate with explicit table requirements; listing need not retain a
// general State just to reuse directory identity and ordering semantics.
pub(super) fn matching_tables<'a>(
    namespace: storage_engine::NamespaceTree<'a>,
    directory_branches: &'a storage_contract::TrackedMap<storage_contract::DirectoryBranch>,
    repository: &'a Repository,
    requested_key: Option<&'a str>,
) -> Vec<&'a storage_contract::DirectoryBranch> {
    let mut branches =
        matching_rows(namespace, directory_branches, repository, requested_key).collect::<Vec<_>>();
    branches.sort_by_key(|branch| {
        (
            std::cmp::Reverse(branch.sequence),
            branch.directory_branch_id.as_str(),
        )
    });
    branches
}

pub(super) fn matching_unordered<'a>(
    engine: &'a Engine,
    repository: &'a Repository,
    requested_key: Option<&'a str>,
) -> Result<impl Iterator<Item = &'a storage_contract::DirectoryBranch>, ApiError> {
    Ok(matching_rows(
        engine
            .namespace_tree(&repository.main_branch_id)
            .map_err(error)?,
        &engine.state().directory_branches,
        repository,
        requested_key,
    ))
}

fn matching_rows<'a>(
    namespace: storage_engine::NamespaceTree<'a>,
    directory_branches: &'a storage_contract::TrackedMap<storage_contract::DirectoryBranch>,
    repository: &'a Repository,
    requested_key: Option<&'a str>,
) -> impl Iterator<Item = &'a storage_contract::DirectoryBranch> {
    enum DirectoryIdentities<'view> {
        Current(BTreeSet<&'view str>),
        Requested(Option<&'view str>),
    }
    // The unfiltered endpoint is still current: index live identities once,
    // rather than probing every namespace entry for each historical branch.
    let identities = match requested_key {
        None => DirectoryIdentities::Current(
            namespace
                .entries()
                .iter()
                .filter(|entry| entry.kind == storage_contract::NamespaceEntryKind::Directory)
                .map(|entry| entry.entry_id.as_str())
                .chain(std::iter::once(repository.repository_id.as_str()))
                .collect(),
        ),
        Some("") => DirectoryIdentities::Requested(Some(repository.repository_id.as_str())),
        Some(key) => DirectoryIdentities::Requested(
            namespace
                .get_key(key)
                .filter(|entry| entry.kind == storage_contract::NamespaceEntryKind::Directory)
                .map(|entry| entry.entry_id.as_str()),
        ),
    };
    directory_branches.values().filter(move |branch| {
        branch.repository_id == repository.repository_id
            && match &identities {
                DirectoryIdentities::Current(ids) => ids.contains(branch.directory_id.as_str()),
                DirectoryIdentities::Requested(id) => {
                    id.is_some_and(|id| branch.directory_id == id)
                }
            }
    })
}

pub(super) fn capture(
    engine: &Engine,
    request: ReadDirectoryScopes<'_>,
) -> Result<DirectoryScopesSnapshot, ApiError> {
    let repo = repository(engine, request.repository_id)?;
    let state = engine.state();
    let scopes = matching(engine, repo, request.filter.key())?;
    let current = |branch: &storage_contract::DirectoryBranch| {
        state.directory_selections.get(&branch.directory_id) == Some(&branch.directory_branch_id)
    };
    let branches = match request.projection {
        DirectoryBranchProjection::Full => DirectoryScopeRows::Full(
            scopes
                .into_iter()
                .map(|branch| {
                    let entries = engine
                        .current_directory_entries(&branch.directory_branch_id)
                        .map_err(error)?;
                    Ok(DirectoryScopeBranch {
                        branch: current_branch(branch, entries),
                        current: current(branch),
                    })
                })
                .collect::<Result<_, ApiError>>()?,
        ),
        DirectoryBranchProjection::Summary => DirectoryScopeRows::Summary(
            scopes
                .into_iter()
                .map(|branch| {
                    let entries = engine
                        .current_directory_entries(&branch.directory_branch_id)
                        .map_err(error)?;
                    Ok(DirectoryBranchChoice::capture_entries(
                        branch,
                        current(branch),
                        entries,
                    ))
                })
                .collect::<Result<_, ApiError>>()?,
        ),
    };
    Ok(DirectoryScopesSnapshot {
        repository_id: repo.repository_id.clone(),
        branches,
    })
}

// No historical selector exists on this endpoint. Its dictionaries follow the
// same admitted entries as the payload; the immutable branch/tree is unchanged.
fn current_branch(
    branch: &storage_contract::DirectoryBranch,
    entries: &[storage_contract::Entry],
) -> storage_contract::DirectoryBranch {
    let storage_contract::DirectoryBranch {
        directory_branch_id,
        repository_id,
        directory_id,
        path,
        name,
        parent_branch_id,
        head_tree_id,
        commit_id,
        entries: _,
        file_branches: source_branches,
        file_contexts: source_contexts,
        inherited_files: source_origins,
        directory_refs: source_directories,
        materialized,
        sequence,
    } = branch;
    let mut file_branches = BTreeMap::new();
    let mut file_contexts = BTreeMap::new();
    let mut inherited_files = BTreeMap::new();
    let mut directory_refs = BTreeMap::new();
    for entry in entries {
        match entry.kind {
            storage_contract::NamespaceEntryKind::File => {
                if let Some(source) = source_branches.get(&entry.file_id) {
                    file_branches
                        .entry(entry.file_id.clone())
                        .or_insert_with(|| source.clone());
                }
                if let Some(context) = source_contexts.get(entry.lookup_key()) {
                    file_contexts.insert(entry.lookup_key().to_owned(), context.clone());
                }
                if let Some(origin) = source_origins.get(&entry.file_id) {
                    inherited_files
                        .entry(entry.file_id.clone())
                        .or_insert_with(|| origin.clone());
                }
            }
            storage_contract::NamespaceEntryKind::Directory => {
                if let Some(reference) = source_directories.get(&entry.entry_id) {
                    directory_refs.insert(entry.entry_id.clone(), reference.clone());
                }
            }
        }
    }
    storage_contract::DirectoryBranch {
        directory_branch_id: directory_branch_id.clone(),
        repository_id: repository_id.clone(),
        directory_id: directory_id.clone(),
        path: path.clone(),
        name: name.clone(),
        parent_branch_id: parent_branch_id.clone(),
        head_tree_id: head_tree_id.clone(),
        commit_id: commit_id.clone(),
        entries: entries.to_vec(),
        file_branches,
        file_contexts,
        inherited_files,
        directory_refs,
        materialized: *materialized,
        sequence: *sequence,
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/directory_scope_projection_tests.rs"]
mod tests;
