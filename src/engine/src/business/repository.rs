//! Repository ownership and explicit SMB binding.
use crate::engine::MutationPlan;
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
                Ok(json!({
                    "repository_id": repository,
                    "branch_id": branch_id,
                    "smb_enabled": enabled,
                    "generation": history.generation.to_string(),
                    "smb_binding_generation": binding_generation.to_string()
                }))
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
        if self.find_receipt(operation)?.is_none()
            && let Some(repo) = self
                .state
                .repositories
                .values()
                .find(|r| r.file_manager_root && r.primary_slot_id == primary)
        {
            return Ok(repo.clone());
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
        self.route(primary, copies)?;
        let mut input =
            json!({"name":name.as_str(),"primary_slot_id":primary,"target_copies":copies});
        if file_manager_root {
            input["file_manager_root"] = json!(true);
        }
        let plan = self.plan_mutation_digest(
            "repository.create",
            principal,
            operation,
            input,
            None,
            |engine, state, scratch| {
                if state
                    .repositories
                    .values()
                    .any(|r| name_policy::lookup_key(&r.name) == name.key())
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::NameCollision,
                        "repository name already exists",
                    ));
                }
                let repo_id = new_id("repo");
                let history_id = new_id("history");
                let branch_id = new_id("br");
                let commit_id = new_id("commit");
                let repository = Repository {
                    repository_id: repo_id.clone(),
                    smb_enabled: true,
                    smb_binding_generation: 1,
                    file_manager_root,
                    name: name.as_str().into(),
                    history_tree_id: history_id.clone(),
                    main_branch_id: branch_id.clone(),
                    primary_slot_id: engine.disk(primary)?.slot_id.clone(),
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
                let empty_namespace =
                    crate::namespace::build_from_entries(&[], &mut scratch.namespace)?;
                state.commits.insert(
                    commit_id.clone(),
                    Commit {
                        namespace_deletions: None,
                        commit_id: commit_id.clone(),
                        history_tree_id: history_id.clone(),
                        kind: "root".into(),
                        parent_commit_ids: Vec::new(),
                        tree_id: new_id("tree"),
                        sequence: state.sequence,
                        operation_id: operation,
                        author: principal.into(),
                        message: "Create repository".into(),
                        namespace: empty_namespace,
                        pending_file_versions: Default::default(),
                        provenance: None,
                    },
                );
                state
                    .repositories
                    .insert(repo_id.clone(), repository.clone());
                Ok(json!({
                    "repository_id": repo_id,
                    "history_tree_id": history_id,
                    "branch_id": branch_id,
                    "commit_id": commit_id,
                    "created_repository": repository
                }))
            },
        )?;
        match plan {
            MutationPlan::Existing(result) => {
                // Later SMB binding changes do not change a creation response.
                json_codec::from_value(result.result["created_repository"].clone()).map_err(
                    |source| {
                        Error::from(source).context(
                            ErrorCode::StateInvalid,
                            "created repository receipt is invalid",
                        )
                    },
                )
            }
            MutationPlan::Prepared {
                next,
                prepared,
                namespace,
                result,
            } => {
                let history = result
                    .history_tree_id
                    .as_deref()
                    .and_then(|id| next.histories.get(id))
                    .expect("invariant: repository creation candidate contains its history");
                let created = next
                    .repositories
                    .get(&history.repository_id)
                    .expect("invariant: repository creation candidate contains its repository")
                    .clone();
                self.persist_with_objects(*next, prepared, namespace, result)?;
                Ok(created)
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/repository/tests.rs"]
mod tests;
