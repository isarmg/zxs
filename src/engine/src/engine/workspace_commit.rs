//! Owned, non-publishing workspace preparation, followed by one serialized decision.
use super::{DetachedPublication, Engine, MutationPlan};
use crate::*;
use std::{
    fs::File,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct PreparationAdmission(Arc<AtomicBool>);
impl Drop for PreparationAdmission {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Private planning view. It exposes no publication or mutation entry point.
/// The immutable base State is shared with the live Engine; only mutation
/// planning clones it. Payload workers share this view rather than cloning
/// State per file, and one per-engine admission bounds active preparations.
struct PlanningSnapshot {
    root: std::path::PathBuf,
    config: ValidatedConfig,
    state: Arc<State>,
    workspace: Workspace,
    blocked: bool,
    snapshot: Option<String>,
    published_state_base: Option<Arc<super::publication::PublishedStateBase>>,
    next_slot: u64,
    replay: Option<super::OperationReadPlan>,
    locks: Vec<File>,
}
impl PlanningSnapshot {
    fn context(&self) -> super::PlanningContext<'_> {
        super::PlanningContext {
            root: &self.root,
            config: &self.config,
            state: &self.state,
            snapshot: &self.snapshot,
            next_slot: self.next_slot,
            blocked: self.blocked,
            admission: super::planning::Admission::Detached,
            published_state_base: self.published_state_base.as_ref(),
        }
    }
}

pub struct WorkspaceCommitPreparation {
    snapshot: PlanningSnapshot,
    request: CommitRequest,
    wire_body: Option<json_codec::Value>,
    frozen_workspace: Vec<u8>,
    admission: PreparationAdmission,
}

pub struct PreparedWorkspaceCommit {
    publication: Option<DetachedPublication>,
    existing: Option<OperationResult>,
    workspace: Workspace,
    request: CommitRequest,
    wire: Option<crate::wire::WireCommit>,
    expected_digest: String,
    frozen_workspace: Vec<u8>,
    base_root: std::path::PathBuf,
    base_topology: u64,
    base_business: u64,
    base_snapshot: Option<String>,
    base_slot: u64,
    base_sequence: u64,
    frozen_config: Vec<u8>,
    _locks: Vec<File>,
    _admission: Option<PreparationAdmission>,
}

impl Engine {
    /// Structural binding also used during restart. Account authentication is
    /// the server boundary; recovery still completes an accepted publication.
    pub(crate) fn workspace_request_matches_operation(
        &self,
        workspace: &Workspace,
        request: &CommitRequest,
    ) -> bool {
        self.planning_context()
            .workspace_request_matches_operation(workspace, request)
    }

    /// One immutable request boundary for synchronous, wire and detached replay.
    /// The actor, workspace and digest remain bound to the original operation;
    /// network adapters must authenticate before entering the engine.
    pub(crate) fn validate_workspace_commit_replay(
        &self,
        request: &CommitRequest,
        digest: &str,
    ) -> Result<Workspace> {
        let workspace = self.workspace(&request.workspace_id)?;
        self.planning_context()
            .validate_workspace_commit_replay(&workspace, request, digest)?;
        Ok(workspace)
    }

    /// Capture request ownership and metadata under the Engine mutex. No payload
    /// bytes are read here. execute() owns all expensive planning/copy/hash work.
    pub fn prepare_workspace_commit(
        &self,
        request: CommitRequest,
        wire_body: Option<json_codec::Value>,
    ) -> Result<WorkspaceCommitPreparation> {
        let workspace = self.workspace(&request.workspace_id)?;
        if workspace.principal != request.principal {
            return Err(Error::new(
                storage_contract::ErrorCode::PermissionDenied,
                "workspace belongs to another principal",
            ));
        }
        let replay = self.state.operations.contains_key(&request.operation_id);
        self.repository_for_branch(&workspace.branch_id)?;
        if !replay {
            self.require_write()?;
            if workspace.fencing_token != request.fencing_token {
                return Err(Error::new(
                    storage_contract::ErrorCode::FencingTokenInvalid,
                    "workspace fencing token differs",
                ));
            }
        }
        if self
            .commit_preparation
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Error::new(
                ErrorCode::CommitPreparationBusy,
                "another workspace commit is being prepared",
            )
            .retryable());
        }
        let admission = PreparationAdmission(self.commit_preparation.clone());
        // Duplicate the existing open-file descriptions: they keep this pool's
        // process locks alive even if the original Engine is dropped meanwhile.
        let locks = self.retain_writer_locks()?;
        let snapshot = PlanningSnapshot {
            root: self.root.clone(),
            config: self.config.clone(),
            state: Arc::clone(&self.state),
            workspace: workspace.clone(),
            blocked: self.blocked,
            snapshot: self.snapshot.clone(),
            published_state_base: self.published_state_base.clone(),
            next_slot: self.next_slot,
            replay: if replay {
                Some(self.prepare_operation_health(self.operation_record(request.operation_id)?))
            } else {
                None
            },
            locks,
        };
        Ok(WorkspaceCommitPreparation {
            snapshot,
            request,
            wire_body,
            frozen_workspace: canonical_json(&workspace)?,
            admission,
        })
    }

    /// Recheck live authority and the exact planning base before the first
    /// PREPARE/ACCEPT record. A stale preparation is a definite retryable conflict,
    /// never an invented unknown operation or a partially accepted candidate.
    pub fn finish_workspace_commit(
        &mut self,
        mut prepared: PreparedWorkspaceCommit,
    ) -> Result<OperationResult> {
        if self.root != prepared.base_root {
            return Err(Error::new(
                storage_contract::ErrorCode::CommitPreparationStale,
                "prepared commit belongs to another engine",
            )
            .retryable());
        }
        // The caller holds exclusive access to the live Engine again. Release
        // only our own claim before revalidation; no physical preparation remains.
        drop(prepared._admission.take());
        let workspace = self.workspace(&prepared.request.workspace_id)?;
        if workspace.principal != prepared.request.principal {
            return Err(Error::new(
                storage_contract::ErrorCode::PermissionDenied,
                "workspace owner changed",
            ));
        }
        if self
            .state
            .operations
            .contains_key(&prepared.request.operation_id)
        {
            self.validate_workspace_commit_replay(&prepared.request, &prepared.expected_digest)?;
            return self.operation(prepared.request.operation_id);
        }
        self.require_write()?;
        self.repository_for_branch(&workspace.branch_id)?;
        if canonical_json(&workspace)? != prepared.frozen_workspace
            || self.snapshot != prepared.base_snapshot
            || self.next_slot != prepared.base_slot
            || self.state.sequence != prepared.base_sequence
            || self.state.topology_generation != prepared.base_topology
            || self.state.business_generation != prepared.base_business
            || canonical_json(&self.config)? != prepared.frozen_config
        {
            return Err(Error::new(storage_contract::ErrorCode::CommitPreparationStale, "workspace or publication base changed during preparation; retry the same operation").retryable());
        }
        if !matches!(
            workspace.state,
            WorkspaceState::Open | WorkspaceState::Sealed
        ) || workspace.fencing_token != prepared.request.fencing_token
        {
            return Err(Error::new(
                storage_contract::ErrorCode::FencingTokenInvalid,
                "workspace no longer admits this commit",
            ));
        }
        if crate::business::now().saturating_sub(workspace.created_at)
            > self.config.workspace_ttl_seconds
        {
            return Err(Error::new(
                storage_contract::ErrorCode::FencingTokenExpired,
                "workspace lease expired during preparation",
            ));
        }
        // Requests become durable before decision publication, just as in the
        // synchronous path, and retain the original operation id and digest.
        self.write_workspace_commit_request(
            &prepared.workspace,
            &prepared.request,
            prepared.wire.as_ref(),
        )?;
        let result = if let Some(publication) = prepared.publication {
            self.decide_publication(publication)?
        } else {
            prepared.existing.ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::StateInvalid,
                    "prepared commit has no publication",
                )
            })?
        };
        self.complete_workspace_commit(prepared.workspace, result)
    }
}

impl WorkspaceCommitPreparation {
    /// Run on a bounded blocking worker, outside the live Engine mutex.
    pub fn execute(mut self) -> Result<PreparedWorkspaceCommit> {
        let replay = self.snapshot.replay.take();
        let engine = self.snapshot.context();
        let plan = if let Some(body) = self.wire_body.take() {
            let envelope =
                crate::wire::ParsedWireEnvelope::parse(&self.request, &self.snapshot.state, body)?;
            engine.plan_workspace_commit_wire(
                self.snapshot.workspace.clone(),
                self.request,
                envelope,
                replay,
            )?
        } else {
            engine.plan_workspace_commit_inner(
                self.snapshot.workspace.clone(),
                self.request,
                None,
                replay,
            )?
        };
        let (publication, existing, expected_digest) = match plan.mutation {
            MutationPlan::Existing(result) => {
                let digest = result.request_digest.clone();
                (None, Some(result), digest)
            }
            MutationPlan::Prepared {
                next,
                prepared,
                result,
            } => {
                let digest = result.request_digest.clone();
                (
                    Some(engine.prepare_publication(
                        *next,
                        prepared,
                        result,
                        crate::disk::MAX_METADATA,
                    )?),
                    None,
                    digest,
                )
            }
        };
        let base_root = engine.root.to_path_buf();
        let base_topology = engine.state.topology_generation;
        let base_business = engine.state.business_generation;
        let base_snapshot = engine.snapshot.clone();
        let base_slot = engine.next_slot;
        let base_sequence = engine.state.sequence;
        let frozen_config = canonical_json(&engine.config)?;
        let locks = self.snapshot.locks;
        Ok(PreparedWorkspaceCommit {
            publication,
            existing,
            expected_digest,
            workspace: plan.workspace,
            request: plan.request,
            wire: plan.wire,
            frozen_workspace: self.frozen_workspace,
            base_root,
            base_topology,
            base_business,
            base_snapshot,
            base_slot,
            base_sequence,
            frozen_config,
            _locks: locks,
            _admission: Some(self.admission),
        })
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/workspace_commit/tests.rs"]
mod tests;

impl super::PlanningContext<'_> {
    pub(crate) fn workspace_request_matches_operation(
        &self,
        workspace: &Workspace,
        request: &CommitRequest,
    ) -> bool {
        self.state
            .operations
            .get(&request.operation_id)
            .is_some_and(|operation| {
                operation.kind == "workspace.commit"
                    && workspace.workspace_id == request.workspace_id
                    && workspace.principal == request.principal
                    && workspace.base_head == request.expected_head
                    && operation.branch_id.as_deref() == Some(workspace.branch_id.as_str())
                    && self.state.operation_principals.get(&request.operation_id)
                        == Some(&request.principal)
                    && request
                        .request_digest
                        .as_deref()
                        .is_none_or(|digest| digest == operation.request_digest)
            })
    }
}

impl super::PlanningContext<'_> {
    pub(crate) fn validate_workspace_commit_replay(
        &self,
        workspace: &Workspace,
        request: &CommitRequest,
        digest: &str,
    ) -> Result<()> {
        if workspace.principal != request.principal {
            return Err(Error::new(
                storage_contract::ErrorCode::PermissionDenied,
                "workspace belongs to another principal",
            ));
        }
        self.repository_for_branch(&workspace.branch_id)?;
        let existing = self
            .state
            .operations
            .get(&request.operation_id)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::OperationNotFound,
                    "published workspace operation is absent",
                )
            })?;
        if !self.workspace_request_matches_operation(workspace, request)
            || existing.request_digest != digest
        {
            return Err(Error::new(
                storage_contract::ErrorCode::IdempotencyKeyReused,
                "commit retry differs from the published request",
            ));
        }
        Ok(())
    }
}
