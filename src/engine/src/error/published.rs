//! Publication is settled; only the named local completion remains incomplete.
use super::Error;
use crate::{OperationResult, RecoveryPlan, Workspace};
use std::fmt;

#[derive(Debug)]
pub struct PublishedWorkspaceReconciliation {
    result: OperationResult,
    workspace_id: String,
    principal: String,
    source: Error,
}

impl PublishedWorkspaceReconciliation {
    pub(crate) fn new(result: OperationResult, workspace: &Workspace, source: Error) -> Self {
        Self {
            result,
            workspace_id: workspace.workspace_id.clone(),
            principal: workspace.principal.clone(),
            source,
        }
    }

    pub fn result(&self) -> &OperationResult {
        &self.result
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn principal(&self) -> &str {
        &self.principal
    }

    pub(crate) fn message(&self) -> String {
        format!(
            "operation {} is already published; workspace {} terminal mirrors require reconciliation; retain the original operation and do not submit a new one",
            self.result.operation_id, self.workspace_id
        )
    }
}

impl fmt::Display for PublishedWorkspaceReconciliation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.message(), self.source)
    }
}

impl std::error::Error for PublishedWorkspaceReconciliation {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryReconciliationStage {
    IndexRebuild,
    WorkspaceReconciliation,
}

impl fmt::Display for RecoveryReconciliationStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::IndexRebuild => "index rebuild",
            Self::WorkspaceReconciliation => "workspace reconciliation",
        })
    }
}

#[derive(Debug)]
pub struct PublishedRecoveryReconciliation {
    result: OperationResult,
    recovery_id: String,
    plan_digest: String,
    principal: String,
    isolation_generation: String,
    stage: RecoveryReconciliationStage,
    source: Error,
}

impl PublishedRecoveryReconciliation {
    pub(crate) fn new(
        result: OperationResult,
        plan: &RecoveryPlan,
        stage: RecoveryReconciliationStage,
        source: Error,
    ) -> Self {
        Self {
            result,
            recovery_id: plan.recovery_id.clone(),
            plan_digest: plan.digest.clone(),
            principal: plan.principal.clone(),
            isolation_generation: plan.isolation_generation.clone(),
            stage,
            source,
        }
    }

    pub fn result(&self) -> &OperationResult {
        &self.result
    }

    pub fn recovery_id(&self) -> &str {
        &self.recovery_id
    }

    pub fn plan_digest(&self) -> &str {
        &self.plan_digest
    }

    pub fn principal(&self) -> &str {
        &self.principal
    }

    pub fn isolation_generation(&self) -> &str {
        &self.isolation_generation
    }

    pub fn stage(&self) -> RecoveryReconciliationStage {
        self.stage
    }

    pub(crate) fn message(&self) -> String {
        format!(
            "operation {} is already published; recovery {} still requires {}; writes remain blocked; retain the original recovery plan",
            self.result.operation_id, self.recovery_id, self.stage
        )
    }
}

impl fmt::Display for PublishedRecoveryReconciliation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.message(), self.source)
    }
}

impl std::error::Error for PublishedRecoveryReconciliation {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
