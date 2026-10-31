//! Shared borrowed filtering, followed by the requested owned read projection.
use super::*;
use crate::{
    DirectoryBranchChoice, DirectoryBranchProjection, DirectoryScopeBranch, DirectoryScopeRows,
    DirectoryScopesSnapshot, ReadDirectoryScopes,
};

pub(super) fn matching<'a>(
    state: &'a storage_contract::State,
    repository: &Repository,
    requested_key: Option<&str>,
) -> Vec<&'a storage_contract::DirectoryBranch> {
    matching_tables(
        &state.branches,
        &state.commits,
        &state.directory_branches,
        repository,
        requested_key,
    )
}

// Shared predicate with explicit table requirements; listing need not retain a
// general State just to reuse directory identity and ordering semantics.
pub(super) fn matching_tables<'a>(
    branches: &'a storage_contract::TrackedMap<storage_contract::Branch>,
    commits: &'a storage_contract::TrackedRecords<storage_contract::Commit>,
    directory_branches: &'a storage_contract::TrackedMap<storage_contract::DirectoryBranch>,
    repository: &Repository,
    requested_key: Option<&str>,
) -> Vec<&'a storage_contract::DirectoryBranch> {
    let requested_directory_id = requested_key.and_then(|key| {
        if key.is_empty() {
            return Some(repository.repository_id.as_str());
        }
        let namespace = branches
            .get(&repository.main_branch_id)
            .expect("invariant: admitted repository references its main branch");
        let commit = commits
            .get(&namespace.head_commit_id)
            .expect("invariant: admitted branch references its head commit");
        commit
            .entries
            .iter()
            .find(|entry| {
                entry.kind == storage_contract::NamespaceEntryKind::Directory
                    && entry.lookup_key() == key
            })
            .map(|entry| entry.entry_id.as_str())
    });
    let mut branches = directory_branches
        .values()
        .filter(|branch| {
            branch.repository_id == repository.repository_id
                && requested_key.is_none_or(|key| {
                    requested_directory_id.map_or_else(
                        || name_policy::path_key(&branch.path) == key,
                        |id| branch.directory_id == id,
                    )
                })
        })
        .collect::<Vec<_>>();
    branches.sort_by_key(|branch| {
        (
            std::cmp::Reverse(branch.sequence),
            branch.directory_branch_id.as_str(),
        )
    });
    branches
}
pub(super) fn capture(
    engine: &Engine,
    request: ReadDirectoryScopes<'_>,
) -> Result<DirectoryScopesSnapshot, ApiError> {
    let repo = repository(engine, request.repository_id)?;
    let state = engine.state();
    let scopes = matching(state, repo, request.filter.key());
    let current = |branch: &storage_contract::DirectoryBranch| {
        state.directory_selections.get(&branch.directory_id) == Some(&branch.directory_branch_id)
    };
    let branches = match request.projection {
        DirectoryBranchProjection::Full => DirectoryScopeRows::Full(
            scopes
                .into_iter()
                .map(|branch| DirectoryScopeBranch {
                    branch: branch.clone(),
                    current: current(branch),
                })
                .collect(),
        ),
        DirectoryBranchProjection::Summary => DirectoryScopeRows::Summary(
            scopes
                .into_iter()
                .map(|branch| DirectoryBranchChoice::capture(branch, current(branch)))
                .collect(),
        ),
    };
    Ok(DirectoryScopesSnapshot {
        repository_id: repo.repository_id.clone(),
        branches,
    })
}
