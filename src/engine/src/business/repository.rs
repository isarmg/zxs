//! Repository ownership and explicit SMB binding.
use crate::*;
use json_codec::json;

/// Follow admitted State ownership; a side/file branch is not a main namespace.
/// Candidate mutations preserve these links, including explicit SMB rebinding.
pub(super) fn main_for_branch<'a>(state: &'a State, branch: &str) -> Option<&'a Repository> {
    let branch = state.branches.get(branch)?;
    let history = state
        .histories
        .get(&branch.history_tree_id)
        .expect("invariant: admitted branch belongs to an existing history");
    let repository = state
        .repositories
        .get(&history.repository_id)
        .expect("invariant: admitted history belongs to an existing repository");
    (repository.main_branch_id == branch.branch_id).then_some(repository)
}

impl Engine {
    /// Rebind an SMB share only after the manager explicitly selects a live
    /// branch. A history split never invokes this operation implicitly.
    /// Registry admission proves a metadata-only change, not body I/O or write
    /// authority for a disabled or detached owner.
    pub fn set_smb_binding(
        &mut self,
        repository: &str,
        branch_id: &str,
        enabled: bool,
        expected_generation: u64,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            "smb.binding.update",
            principal,
            operation,
            json!({
                "repository_id": repository,
                "branch_id": branch_id,
                "enabled": enabled,
                "expected_generation": expected_generation.to_string()
            }),
            |engine, state, _| {
                let repo = state.repositories.get(repository).cloned().ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::RepositoryNotFound,
                        "repository not found",
                    )
                })?;

                let history = state.histories.get(&repo.history_tree_id).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::StateInvalid,
                        "repository history missing",
                    )
                })?;
                if history.generation != expected_generation {
                    return Err(Error::new(
                        storage_contract::ErrorCode::GenerationMismatch,
                        "share binding history changed",
                    ));
                }
                let branch = state.branches.get(branch_id).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::BranchNotFound,
                        "share branch missing",
                    )
                })?;
                if branch.history_tree_id != history.history_tree_id
                    || branch.state != BranchState::Active
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::BranchNotActive,
                        "share binding requires a live branch in this repository",
                    ));
                }
                if branch.file_id.is_some() {
                    return Err(Error::new(
                        storage_contract::ErrorCode::FileBranchScope,
                        "a file branch cannot replace a directory or SMB namespace",
                    ));
                }
                if engine.workspaces.values().any(|w| {
                    matches!(w.state, WorkspaceState::Open | WorkspaceState::Sealed)
                        && state
                            .branches
                            .get(&w.branch_id)
                            .is_some_and(|b| b.history_tree_id == history.history_tree_id)
                }) {
                    return Err(Error::new(
                        storage_contract::ErrorCode::WorkspaceBusy,
                        "close affected write workspaces before rebinding the share",
                    ));
                }
                let namespace_binding = engine
                    .admitted_current_namespaces()?
                    .map(|current| current.smb_binding_source(state, &repo, branch))
                    .transpose()?;
                let history_id = repo.history_tree_id.clone();
                let repo = state
                    .repositories
                    .get_mut(repository)
                    .expect("repository checked");
                repo.smb_binding_generation =
                    repo.smb_binding_generation.checked_add(1).ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::ResourceLimit,
                            "SMB binding generation exhausted",
                        )
                    })?;
                repo.main_branch_id = branch_id.into();
                repo.smb_enabled = enabled;
                let binding_generation = repo.smb_binding_generation;
                let history = state
                    .histories
                    .get_mut(&history_id)
                    .expect("history checked");
                history.main_branch_id = Some(branch_id.into());
                history.generation = history.generation.checked_add(1).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::ResourceLimit,
                        "history generation exhausted",
                    )
                })?;
                let mut result = json!({
                    "repository_id": repository,
                    "branch_id": branch_id,
                    "smb_enabled": enabled,
                    "generation": history.generation.to_string(),
                    "smb_binding_generation": binding_generation.to_string()
                });
                if let Some(binding) = namespace_binding {
                    result["namespace_binding"] = binding;
                }
                Ok(result)
            },
        )
    }
    pub fn create_repository(
        &mut self,
        name: &storage_contract::RepositoryName,
        primary: &str,
        copies: u32,
        principal: &str,
        operation: OperationId,
    ) -> Result<Repository> {
        self.create_repository_kind(name, primary, copies, principal, operation, false)
    }
    pub fn create_disk_root(
        &mut self,
        primary: &str,
        principal: &str,
        operation: OperationId,
    ) -> Result<Repository> {
        Self::require_actor(principal)?;
        let primary = self.disk(primary)?.slot_id.clone();
        // An unused operation may resolve the existing disk root. A published
        // operation must first match its original owner and creation request,
        // and return that receipt rather than the root's current SMB binding.
        if !self.state.operations.contains_key(&operation) {
            let current = self.admitted_current_namespaces()?;
            if let Some(current) = current {
                current.require_write_slot(&primary)?;
            }
            if let Some(repo) = self
                .state
                .repositories
                .values()
                .find(|r| r.file_manager_root && r.primary_slot_id == primary)
            {
                if let Some(current) = current {
                    current.require_branch_write(&self.state, &repo.main_branch_id)?;
                }
                return Ok(repo.clone());
            }
        }
        self.create_repository_kind(
            &storage_contract::RepositoryName::parse(&format!("ZXS disk {primary}"))?,
            &primary,
            // A disk root needs only its own primary. Additional copies are
            // an explicit protection policy, not a prerequisite for using it.
            1,
            principal,
            operation,
            true,
        )
    }
    fn create_repository_kind(
        &mut self,
        name: &storage_contract::RepositoryName,
        primary: &str,
        copies: u32,
        principal: &str,
        operation: OperationId,
        file_manager_root: bool,
    ) -> Result<Repository> {
        let mut input =
            json!({"name":name.as_str(),"primary_slot_id":primary,"target_copies":copies});
        if file_manager_root {
            input["file_manager_root"] = json!(true);
        }
        let result = self.mutate(
            "repository.create",
            principal,
            operation,
            input,
            |engine, state, _| {
                // Current routes authorize a new root, never overwrite the
                // meaning of a permanent creation receipt after a later exit.
                engine.route(primary, copies)?;
                let repository = project_creation(
                    state,
                    RepositoryCreation {
                        name,
                        primary: &engine.disk(primary)?.slot_id,
                        copies,
                        file_manager_root,
                        principal,
                        operation,
                    },
                    RepositoryIds::random()?,
                )?;
                let repo_id = &repository.repository_id;
                let history_id = &repository.history_tree_id;
                let branch_id = &repository.main_branch_id;
                let commit_id = &state.histories[history_id].root_commit_id;
                Ok(json!({
                    "repository_id": repo_id,
                    "history_tree_id": history_id,
                    "branch_id": branch_id,
                    "commit_id": commit_id,
                    "created_repository": repository
                }))
            },
        )?;
        // Later SMB binding changes do not change a creation response. Do not
        // substitute the live repository when replaying its original receipt.
        json_codec::from_value(result.result["created_repository"].clone()).map_err(|source| {
            Error::from(source).context(
                ErrorCode::StateInvalid,
                "created repository receipt is invalid",
            )
        })
    }
}

pub(super) struct RepositoryCreation<'a> {
    pub(super) name: &'a storage_contract::RepositoryName,
    pub(super) primary: &'a str,
    pub(super) copies: u32,
    pub(super) file_manager_root: bool,
    pub(super) principal: &'a str,
    pub(super) operation: OperationId,
}

pub(super) struct RepositoryIds {
    repository: String,
    history: String,
    branch: String,
    commit: String,
    tree: String,
}
impl RepositoryIds {
    fn random() -> Result<Self> {
        Ok(Self {
            repository: new_id("repo")?,
            history: new_id("history")?,
            branch: new_id("br")?,
            commit: new_id("commit")?,
            tree: new_id("tree")?,
        })
    }

    pub(super) fn move_target(operation: OperationId) -> Result<Self> {
        let id = |prefix: &str| -> Result<String> {
            let digest = object_hash("object-move-root", &(operation, prefix))?;
            Ok(format!("{prefix}_{}", &digest[..32]))
        };
        Ok(Self {
            repository: id("repo")?,
            history: id("history")?,
            branch: id("br")?,
            commit: id("commit")?,
            tree: id("tree")?,
        })
    }
}

/// The one root-record projection for explicit creation and an empty-disk move.
/// The caller supplies the admitted route; publication remains its operation.
pub(super) fn project_creation(
    state: &mut State,
    creation: RepositoryCreation<'_>,
    ids: RepositoryIds,
) -> Result<Repository> {
    let RepositoryCreation {
        name,
        primary,
        copies,
        file_manager_root,
        principal,
        operation,
    } = creation;
    if state
        .repositories
        .values()
        .any(|repository| name_policy::lookup_key(&repository.name) == name.key())
    {
        return Err(Error::new(
            ErrorCode::NameCollision,
            "repository name already exists",
        ));
    }
    let RepositoryIds {
        repository: repo_id,
        history: history_id,
        branch: branch_id,
        commit: commit_id,
        tree: tree_id,
    } = ids;
    let repository = Repository {
        repository_id: repo_id.clone(),
        smb_enabled: true,
        smb_binding_generation: 1,
        file_manager_root,
        name: name.as_str().into(),
        history_tree_id: history_id.clone(),
        main_branch_id: branch_id.clone(),
        primary_slot_id: primary.to_owned(),
        target_copies: copies,
        name_policy: name_policy::NAME_POLICY.into(),
        unicode_version: name_policy::UNICODE_VERSION.into(),
    };
    state.histories.insert(
        history_id.clone(),
        HistoryTree {
            history_tree_id: history_id.clone(),
            repository_id: repo_id.clone(),
            root_commit_id: commit_id.clone(),
            main_branch_id: Some(branch_id.clone()),
            generation: 1,
            state: HistoryState::Active,
            provenance: None,
        },
    );
    state.branches.insert(
        branch_id.clone(),
        Branch {
            file_node_id: None,
            branch_id: branch_id.clone(),
            history_tree_id: history_id.clone(),
            name: "main".into(),
            head_commit_id: commit_id.clone(),
            fork_commit_id: commit_id.clone(),
            branch_kind: BranchKind::Full,
            file_id: None,
            pending_file_versions: Default::default(),
            generation: 1,
            state: BranchState::Active,
        },
    );
    state.commits.insert(
        commit_id.clone(),
        Commit {
            namespace_deletions: None,
            commit_id: commit_id.clone(),
            history_tree_id: history_id.clone(),
            kind: "root".into(),
            parent_commit_ids: Vec::new(),
            tree_id,
            sequence: state.sequence,
            operation_id: operation,
            author: principal.into(),
            message: "Create repository".into(),
            entries: Vec::new(),
            pending_file_versions: Default::default(),
            provenance: None,
        },
    );
    state
        .repositories
        .insert(repo_id.clone(), repository.clone());
    Ok(repository)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/repository/tests.rs"]
mod tests;
