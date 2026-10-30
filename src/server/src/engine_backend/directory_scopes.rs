//! Shared borrowed filtering, followed by the requested owned read projection.
use super::*;
use crate::{
    DirectoryBranchChoice, DirectoryBranchProjection, DirectoryScopeBranch, DirectoryScopeRows,
    DirectoryScopesSnapshot, ReadDirectoryScopes,
};

pub(super) fn matching<'a>(
    engine: &'a Engine,
    repository: &Repository,
    requested_key: Option<&str>,
) -> Result<Vec<&'a storage_contract::DirectoryBranch>, ApiError> {
    let state = engine.state();
    let namespace = requested_key.filter(|key| !key.is_empty()).map(|_| {
        let branch = state
            .branches
            .get(&repository.main_branch_id)
            .expect("invariant: admitted repository references its main branch");
        engine
            .commit_namespace(&branch.head_commit_id)
            .expect("invariant: admitted branch references its head commit")
    });
    let scope = ScopeMatch::resolve(repository, requested_key, namespace)?;
    Ok(state
        .directory_branches
        .values()
        .filter(|branch| scope.matches(branch))
        .collect())
}

/// The same stable-directory/fallback-path rule for catalogues and bounded IDs.
pub(super) struct ScopeMatch<'a> {
    repository_id: &'a str,
    requested_key: Option<&'a str>,
    requested_directory_id: Option<String>,
}

impl<'a> ScopeMatch<'a> {
    pub(super) fn resolve(
        repository: &'a Repository,
        requested_key: Option<&'a str>,
        namespace: Option<storage_engine::NamespaceTree<'_>>,
    ) -> Result<Self, ApiError> {
        let requested_directory_id = match requested_key {
            Some("") => Some(repository.repository_id.clone()),
            Some(key) => namespace
                .expect("invariant: non-root scope resolution supplies its main namespace")
                .get_key(key)
                .map_err(file_manager::activity::read_error)?
                .filter(|entry| entry.kind == storage_contract::NamespaceEntryKind::Directory)
                .map(|entry| entry.entry_id),
            None => None,
        };
        Ok(Self {
            repository_id: &repository.repository_id,
            requested_key,
            requested_directory_id,
        })
    }

    pub(super) fn matches(&self, branch: &storage_contract::DirectoryBranch) -> bool {
        branch.repository_id == self.repository_id
            && self.requested_key.is_none_or(|key| {
                self.requested_directory_id.as_deref().map_or_else(
                    || name_policy::path_key(&branch.path) == key,
                    |id| branch.directory_id == id,
                )
            })
    }
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

/// Order already captured observations; no engine or authorization grant is retained.
pub(super) fn sort(snapshot: &mut DirectoryScopesSnapshot) {
    match &mut snapshot.branches {
        DirectoryScopeRows::Full(branches) => branches.sort_by(|left, right| {
            (
                std::cmp::Reverse(left.branch.sequence),
                &left.branch.directory_branch_id,
            )
                .cmp(&(
                    std::cmp::Reverse(right.branch.sequence),
                    &right.branch.directory_branch_id,
                ))
        }),
        DirectoryScopeRows::Summary(branches) => sort_choices(branches),
    }
}

pub(super) fn sort_choices(branches: &mut [DirectoryBranchChoice]) {
    branches.sort_by(|left, right| {
        (std::cmp::Reverse(left.sequence), &left.directory_branch_id).cmp(&(
            std::cmp::Reverse(right.sequence),
            &right.directory_branch_id,
        ))
    });
}
