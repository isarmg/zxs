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
    /// Check a prospective path before allocating its missing disk root.
    /// Every component is new here; existing namespaces retain their spelling.
    /// This observation grants no write authority and is not a replay rule.
    pub fn validate_new_disk_root_path(
        &self,
        primary: &name_policy::SlotId,
        path: &name_policy::PortablePath,
    ) -> Result<()> {
        self.require_write()?;
        // A pending registry is not the pre-registry genesis configuration.
        let disk = self.disk(primary.as_str())?;
        if let Some(current) = self.admitted_current_namespaces()? {
            current.require_write_slot(&disk.slot_id)?;
        }
        self.route(&disk.slot_id, 1)?;
        let maximum = self.disk_resources.get(disk)?.root.component_name_maximum()?;
        crate::name_budget::NameBudget::from_maximum(maximum)
            .map_err(Error::ConversionNameBudget)?
            .path(path.as_str())
            .map_err(Error::ConversionNameBudget)?;
        Ok(())
    }

    /// Reject an impossible prospective upload before allocating a private
    /// workspace. Publication independently checks the actual replica targets.
    pub fn validate_disk_root_upload_path(
        &self,
        repository: storage_contract::RepositoryId,
        path: &name_policy::PortablePath,
    ) -> Result<()> {
        let repo = self.state.repositories.get(&repository.to_string())
            .filter(|repo| repo.file_manager_root)
            .ok_or(crate::engine::FreshLayoutError::NamespaceUnmapped)?;
        self.validate_disk_root_upload_paths(repo, std::iter::once(path))
    }

    pub(super) fn validate_disk_root_upload_paths<'a>(
        &self,
        repo: &Repository,
        paths: impl IntoIterator<Item = &'a name_policy::PortablePath>,
    ) -> Result<()> {
        use std::borrow::Cow;

        self.require_write()?;
        if let Some(current) = self.admitted_current_namespaces()? {
            current.require_branch_write(&self.state, &repo.main_branch_id)?;
        }
        let disk = self.disk(&repo.primary_slot_id)?;
        self.route(&disk.slot_id, 1)?;
        let root = self.disk_resources.get(disk)?.content_root()?;
        let budget = crate::name_budget::NameBudget::from_maximum(root.component_name_maximum()?)
            .map_err(Error::ConversionNameBudget)?;
        let current = self.namespace_tree(&repo.main_branch_id)?;
        for path in paths {
            let mut next = Some(Cow::Borrowed(path));
            while let Some(component) = next {
                if current.get_key(component.key()).is_some() {
                    // Existing ancestors keep their admitted spelling.
                    break;
                }
                budget.component(component.file_name(), path.as_str())
                    .map_err(Error::ConversionNameBudget)?;
                next = component.parent().map(Cow::Owned);
            }
        }
        Ok(())
    }

    // A logical candidate, not authority to enumerate or read it. Both the
    // live and the stopped-media metadata paths validate their own origin.
    pub(crate) fn ordinary_root_candidate(&self, slot: &str) -> Result<Option<&Repository>> {
        let mut matches = self.state.repositories.values()
            .filter(|repository| repository.primary_slot_id == slot);
        let selected = matches.next();
        if selected.is_some_and(|repo| !repo.file_manager_root) || matches.next().is_some() {
            return Err(crate::engine::FreshLayoutError::NamespaceSource.into());
        }
        Ok(selected)
    }

    /// Read the one ordinary namespace registered on this enabled storage slot.
    /// This never allocates a repository, creates a workspace, or authorizes a
    /// write. An offline root, duplicate registration or broken original
    /// remains an error; a Backup read must use its own published source.
    pub fn ordinary_root_for_slot(&self, slot: &str) -> Result<Option<&Repository>> {
        use crate::engine::FreshLayoutError as Failure;
        let disk = self.disk(slot)?;
        if let Some(current) = self.admitted_current_namespaces()? {
            // This predicate checks only the registry's live Enabled slot. It
            // does not perform a write and confers no branch or user rights.
            current.require_write_slot(&disk.slot_id)?;
        }
        if let Some(repository) = self.ordinary_root_candidate(&disk.slot_id)? {
            let source = self.namespace_host_root(
                repository.repository_id.parse()?,
                repository.main_branch_id.parse()?,
            )?;
            if source.disk().disk_id != disk.disk_id {
                return Err(Failure::Registration.into());
            }
            self.namespace_tree(&repository.main_branch_id)?;
            return Ok(Some(repository));
        }

        // A present native receipt without its current owner is not an
        // unregistered slot. Strict opening authenticates these original
        // receipts; a partial or missing FILE namespace cannot be recreated
        // merely because its display-name row was absent.
        for receipt in self.state.operations.values() {
            if let Some(origin) = crate::persist::namespace_root::origin(receipt)?
                && (origin.genesis.registration.disk_id == disk.disk_id
                    || origin.genesis.registration.slot_id == disk.slot_id)
            {
                return Err(Failure::NamespaceSource.into());
            }
            if receipt.kind == "repository.create" {
                let created = crate::persist::namespace_root::created_repository(receipt)?;
                if created.file_manager_root && created.primary_slot_id == disk.slot_id {
                    return Err(Failure::NamespaceSource.into());
                }
            }
        }
        let held = self.disk_resources.get(disk)?;
        crate::engine::fresh_layout::verify_root(&self.config, disk, &held.root)?;
        crate::engine::provenance::validate_owned_seal(
            &self.disk_resources, disk, &self.metadata_integrity,
        )?;
        let common = held.content_root()?;
        if common.entries(common.path())?.next().transpose()?.is_some() {
            return Err(Failure::NamespaceSource.into());
        }
        Ok(None)
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
                self.namespace_host_root(
                    repo.repository_id.parse()?, repo.main_branch_id.parse()?,
                )?;
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
                let origin = if file_manager_root {
                    Some(engine.planning_context().prepare_namespace_root_origin(
                        state, &repository, operation,
                    )?)
                } else {
                    None
                };
                let repo_id = &repository.repository_id;
                let history_id = &repository.history_tree_id;
                let branch_id = &repository.main_branch_id;
                let commit_id = &state.histories[history_id].root_commit_id;
                let mut result = json!({
                    "repository_id": repo_id,
                    "history_tree_id": history_id,
                    "branch_id": branch_id,
                    "commit_id": commit_id,
                    "created_repository": repository
                });
                if let Some(origin) = origin {
                    crate::persist::namespace_root::attach(&mut result, &origin)?;
                }
                Ok(result)
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
