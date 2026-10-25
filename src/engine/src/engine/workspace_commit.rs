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
    engine: Engine,
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

    /// One immutable request boundary for synchronous, wire and detached replay.
    /// The actor, workspace and digest remain bound to the original operation;
    /// network adapters must authenticate before entering the engine.
    pub(crate) fn validate_workspace_commit_replay(
        &self,
        request: &CommitRequest,
        digest: &str,
    ) -> Result<Workspace> {
        let workspace = self.workspace(&request.workspace_id)?;
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
        if !self.workspace_request_matches_operation(&workspace, request)
            || existing.request_digest != digest
        {
            return Err(Error::new(
                storage_contract::ErrorCode::IdempotencyKeyReused,
                "commit retry differs from the published request",
            ));
        }
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
        self.commit_preparation
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                Error::new(
                    storage_contract::ErrorCode::CommitPreparationBusy,
                    "another workspace commit is being prepared",
                )
                .retryable()
            })?;
        let admission = PreparationAdmission(self.commit_preparation.clone());
        // Duplicate the existing open-file descriptions: they keep this pool's
        // process locks alive even if the original Engine is dropped meanwhile.
        let locks = self.retain_writer_locks()?;
        let engine = Engine {
            root: self.root.clone(),
            config: self.config.clone(),
            state: Arc::clone(&self.state),
            // Planning reads only this workspace, with no workspace admission or recovery.
            workspaces: [(workspace.workspace_id.clone(), workspace.clone())].into(),
            workspace_budget: self.workspace_budget.clone(),
            retained_workspace_limits: self.retained_workspace_limits,
            blocked: self.blocked,
            snapshot: self.snapshot.clone(),
            published_state_base: self.published_state_base.clone(),
            next_slot: self.next_slot,
            reads: self.reads.clone(),
            upload_preparations: self.upload_preparations.clone(),
            // The frozen view may plan writes; only the live Engine is fenced.
            commit_preparation: Arc::new(AtomicBool::new(false)),
            history_query_proof: std::sync::Mutex::new(super::index::QueryProof::default()),
            _locks: locks,
            #[cfg(feature = "fault-injection")]
            fault: None,
        };
        Ok(WorkspaceCommitPreparation {
            snapshot: PlanningSnapshot { engine },
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
            return self.operation(&prepared.request.operation_id);
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
        if !matches!(workspace.state.as_str(), "open" | "sealed")
            || workspace.fencing_token != prepared.request.fencing_token
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
        let engine = &self.snapshot.engine;
        let plan = if let Some(body) = self.wire_body.take() {
            engine.plan_workspace_commit_wire(self.request, body)?
        } else {
            engine.plan_workspace_commit_inner(self.request, None)?
        };
        let (publication, existing, expected_digest) = match plan.mutation {
            MutationPlan::Existing(result) => (None, Some(result.clone()), result.request_digest),
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
        let base_root = engine.root.clone();
        let base_topology = engine.state.topology_generation;
        let base_business = engine.state.business_generation;
        let base_snapshot = engine.snapshot.clone();
        let base_slot = engine.next_slot;
        let base_sequence = engine.state.sequence;
        let frozen_config = canonical_json(&engine.config)?;
        let locks = std::mem::take(&mut self.snapshot.engine._locks);
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
