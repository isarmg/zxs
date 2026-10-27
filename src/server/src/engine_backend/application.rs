//! Typed, transport-independent mutation boundary.
//!
//! HTTP and SMB decode their envelopes before entering this service. Account
//! admission happens at the backend entrance; domain and workspace ownership checks precede
//! delegating durable state changes to the core. Results remain domain records;
//! presentation/redaction belongs to the transport adapter.
use super::{error, repo_for_branch, repo_for_history, repository, workspace_authorize};
use crate::{ApiError, auth::Identity};
use data_serialization::Deserialize;
use storage_engine::{CommitRequest, Engine, FileLocation};

use crate::CommitWorkspace;

pub use crate::StageNamespace;

use crate::CreateBranch;

use crate::CreateMergeLink;

use crate::NamespaceBatch;

#[derive(Debug, Clone, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CommitPlan {
    pub plan_id: String,
    pub operation_id: String,
    pub plan_digest: String,
}

pub struct MergeFileBranches {
    pub file_id: String,
    pub inputs: Vec<storage_contract::FileMergeInput>,
    pub chosen_node_id: String,
    pub surviving_branch_id: String,
    pub confirmed: bool,
    pub location: Option<FileLocation>,
    pub operation_id: storage_contract::OperationId,
}

/// This service borrows a locked engine. It has no transport, JSON dispatch, or
/// caller-supplied principal; the authenticated identity is bound at creation.
pub struct Application<'a> {
    engine: &'a mut Engine,
    identity: &'a Identity,
}

impl<'a> Application<'a> {
    pub(super) fn new(engine: &'a mut Engine, identity: &'a Identity) -> Self {
        Self { engine, identity }
    }

    fn commit_request(&self, command: CommitWorkspace) -> Result<CommitRequest, ApiError> {
        workspace_authorize(self.engine, self.identity, &command.workspace_id)?;
        Ok(CommitRequest {
            workspace_id: command.workspace_id,
            operation_id: command.operation_id,
            principal: self.identity.principal.clone(),
            expected_head: command.expected_head,
            fencing_token: command.fencing_token,
            message: command.message,
            request_digest: command.request_digest,
        })
    }

    pub fn commit_workspace(
        &mut self,
        command: CommitWorkspace,
    ) -> Result<storage_engine::OperationResult, ApiError> {
        let request = self.commit_request(command)?;
        self.engine.commit_workspace(request).map_err(error)
    }

    fn public_commit_request(
        &self,
        command: &storage_contract::PublicCommitEnvelope,
    ) -> Result<CommitRequest, ApiError> {
        workspace_authorize(self.engine, self.identity, command.workspace_id())?;
        Ok(CommitRequest {
            workspace_id: command.workspace_id().into(),
            operation_id: command.operation_id(),
            principal: self.identity.principal.clone(),
            expected_head: command.expected_head().into(),
            fencing_token: command.fencing_token(),
            message: command.message().into(),
            request_digest: Some(command.claimed_digest().into()),
        })
    }

    pub fn commit_workspace_public(
        &mut self,
        command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_engine::OperationResult, ApiError> {
        let request = self.public_commit_request(&command)?;
        self.engine
            .commit_workspace_wire(request, command)
            .map_err(error)
    }

    pub fn prepare_commit_workspace(
        &mut self,
        command: CommitWorkspace,
    ) -> Result<storage_engine::WorkspaceCommitPreparation, ApiError> {
        let request = self.commit_request(command)?;
        self.engine
            .prepare_workspace_commit(request, None)
            .map_err(error)
    }

    pub fn prepare_commit_workspace_public(
        &mut self,
        command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_engine::WorkspaceCommitPreparation, ApiError> {
        let request = self.public_commit_request(&command)?;
        self.engine
            .prepare_workspace_commit(request, Some(command))
            .map_err(error)
    }

    pub fn finish_workspace_commit(
        &mut self,
        workspace: &str,
        prepared: storage_engine::PreparedWorkspaceCommit,
    ) -> Result<storage_engine::OperationResult, ApiError> {
        workspace_authorize(self.engine, self.identity, workspace)?;
        self.engine.finish_workspace_commit(prepared).map_err(error)
    }

    pub fn stage_namespace(
        &mut self,
        command: StageNamespace,
    ) -> Result<storage_engine::Workspace, ApiError> {
        workspace_authorize(self.engine, self.identity, &command.workspace_id)?;
        self.engine
            .namespace_operation(&command.workspace_id, command.change)
            .map_err(error)
    }

    pub fn create_branch(
        &mut self,
        command: CreateBranch,
    ) -> Result<storage_contract::Branch, ApiError> {
        let repo = repository(self.engine, &command.repository_id)?;
        if self
            .engine
            .state()
            .commits
            .get(&command.source_commit_id)
            .is_none_or(|commit| commit.history_tree_id != repo.history_tree_id)
        {
            return Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::HistoryNodeNotFound,
                "source commit is outside the repository",
            ));
        }
        self.engine
            .create_branch(
                &command.source_commit_id,
                &command.name,
                &self.identity.principal,
                command.operation_id,
            )
            .map_err(error)
    }

    pub fn merge_link(
        &mut self,
        command: CreateMergeLink,
    ) -> Result<storage_contract::MergeLink, ApiError> {
        let operation_id = command.operation_id;
        repo_for_history(self.engine, &command.history_tree_id)?;
        // A split can move both original endpoints after this operation was
        // published. Admit only its original owner/history to core replay;
        // the core still requires the exact original request digest.
        let replay = self
            .engine
            .state()
            .operations
            .get(&operation_id)
            .is_some_and(|operation| {
                operation.kind == "merge-link.create"
                    && operation.history_tree_id.as_deref()
                        == Some(command.history_tree_id.as_str())
            })
            && self
                .engine
                .operation_owner(operation_id)
                .map_err(crate::engine_backend::error)?
                .as_deref()
                == Some(self.identity.principal.as_str());
        if !replay {
            for branch in [&command.source_branch_id, &command.target_branch_id] {
                if repo_for_branch(self.engine, branch)?.history_tree_id != command.history_tree_id
                {
                    return Err(ApiError::forbidden());
                }
            }
        }
        self.engine
            .add_merge_link(
                &command.source_branch_id,
                &command.source_commit_id,
                &command.target_branch_id,
                &command.target_commit_id,
                &self.identity.principal,
                operation_id,
            )
            .map_err(error)
    }

    pub fn namespace_batch(
        &mut self,
        command: NamespaceBatch,
    ) -> Result<storage_engine::OperationResult, ApiError> {
        let disk = self
            .engine
            .config()
            .disks
            .iter()
            .find(|disk| disk.slot_id == command.slot_id)
            .ok_or_else(|| {
                ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::DiskNotFound,
                    "disk not found",
                )
            })?;
        if disk.level != 1 {
            return Err(ApiError::new(
                ::http::StatusCode::FORBIDDEN,
                storage_contract::ErrorCode::DiskReadOnly,
                "backup disks are read only",
            ));
        }
        let mut repository_id = None;
        for item in &command.request.items {
            for branch_id in std::iter::once(item.source.directory_branch_id.as_str()).chain(
                item.target
                    .iter()
                    .map(|target| target.directory_branch_id.as_str()),
            ) {
                let branch = self
                    .engine
                    .state()
                    .directory_branches
                    .get(branch_id)
                    .ok_or_else(|| {
                        ApiError::new(
                            ::http::StatusCode::NOT_FOUND,
                            storage_contract::ErrorCode::DirectoryBranchNotFound,
                            "directory branch missing",
                        )
                    })?;
                let repo = repository(self.engine, &branch.repository_id)?;
                if repo.primary_slot_id != command.slot_id {
                    return Err(ApiError::invalid(
                        "batch branches must belong to this primary disk",
                    ));
                }
                if repository_id
                    .as_ref()
                    .is_some_and(|id| id != &repo.repository_id)
                {
                    return Err(ApiError::invalid(
                        "batch branches must belong to one repository",
                    ));
                }
                repository_id = Some(repo.repository_id.clone());
            }
        }
        let repository_id =
            repository_id.ok_or_else(|| ApiError::invalid("batch items cannot be empty"))?;
        self.engine
            .directory_namespace_batch(
                &repository_id,
                &command.request,
                &self.identity.principal,
                command.operation_id,
            )
            .map_err(error)
    }

    pub(crate) fn authorize_file(&self, file: &str) -> Result<(), ApiError> {
        let version = self
            .engine
            .state()
            .versions
            .values()
            .find(|version| version.file_id == file)
            .ok_or_else(|| {
                ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::FileNotFound,
                    "file not found",
                )
            })?;
        repo_for_history(self.engine, &version.history_tree_id)?;
        Ok(())
    }

    pub fn merge_file_branches(
        &mut self,
        command: MergeFileBranches,
    ) -> Result<storage_engine::OperationResult, ApiError> {
        self.authorize_file(&command.file_id)?;
        if !command.confirmed {
            return Err(ApiError::invalid(
                "confirm the chosen result and surviving branch before merging",
            ));
        }
        self.engine
            .merge_file_branches(storage_engine::FileBranchMergeRequest {
                file_id: &command.file_id,
                inputs: &command.inputs,
                chosen_node_id: &command.chosen_node_id,
                surviving_branch_id: &command.surviving_branch_id,
                location: command.location.as_ref(),
                principal: &self.identity.principal,
                operation_id: command.operation_id,
            })
            .map_err(error)
    }

    /// Destructive purge/clear and split plans retain owner, digest and domain
    /// checks at execution time, even when prepared by a different adapter.
    pub fn commit_plan(
        &mut self,
        command: CommitPlan,
    ) -> Result<storage_engine::OperationResult, ApiError> {
        let plan = self.engine.plan(&command.plan_id).map_err(error)?;
        if plan.principal != self.identity.principal {
            return Err(ApiError::forbidden());
        }
        if command.plan_digest != plan.digest {
            return Err(ApiError::conflict(
                if plan.kind == "split" {
                    storage_contract::ErrorCode::SplitPlanStale
                } else {
                    storage_contract::ErrorCode::PlanStale
                },
                "plan digest does not match",
            ));
        }
        let history = super::field(&plan.input, "history_tree_id")?;
        repo_for_history(self.engine, history)?;
        self.engine
            .commit_plan(
                &command.plan_id,
                &self.identity.principal,
                command
                    .operation_id
                    .parse::<storage_contract::OperationId>()
                    .map_err(|source| error(source.into()))?,
            )
            .map_err(error)
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/application/tests.rs"]
mod tests;
