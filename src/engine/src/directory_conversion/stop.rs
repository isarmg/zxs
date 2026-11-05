//! Explicit termination retains all data and publishes a separate permanent receipt.
use super::*;
use crate::{
    CommitRequest, DiskRegistrationIdentity, OperationId, OperationResult, Workspace,
    WorkspaceState,
};
use json_codec::json;

const STOP_KIND: &str = "directory.import.stop";

/// Build only from the authenticated workflow's durably retained identities.
/// Stopping never cancels an unfinished disk registration or removes its root.
#[derive(Clone, Debug)]
pub struct DirectoryImportStopRequest {
    pub principal: String,
    pub target: DiskRegistrationIdentity,
    pub import_operation_id: OperationId,
    pub root_operation_id: Option<OperationId>,
    pub workspace_id: Option<String>,
    pub commit_request: Option<CommitRequest>,
    pub stop_operation_id: OperationId,
}
#[derive(Debug)]
pub enum DirectoryImportStopOutcome {
    Published(OperationResult),
    Stopped(OperationResult),
}
impl DirectoryImportStopRequest {
    fn input(&self) -> json_codec::Value {
        json!({
            "target": self.target,
            "import_operation_id": self.import_operation_id,
            "root_operation_id": self.root_operation_id,
            "workspace_id": self.workspace_id,
            "commit_request": self.commit_request,
        })
    }
    fn validate(&self) -> Result<()> {
        Engine::require_actor(&self.principal)?;
        if self.import_operation_id == self.stop_operation_id
            || self
                .root_operation_id
                .is_some_and(|id| id == self.import_operation_id || id == self.stop_operation_id)
        {
            return Err(invalid("import, root and stop operations must be distinct"));
        }
        if let Some(id) = &self.workspace_id {
            crate::validate_id(id, "ws")?;
        }
        if let Some(request) = &self.commit_request
            && (request.operation_id != self.import_operation_id
                || request.principal != self.principal
                || Some(&request.workspace_id) != self.workspace_id.as_ref()
                || request.message != "Import ordinary directory contents"
                || request.request_digest.is_some())
        {
            return Err(invalid(
                "stop does not bind the original native import request",
            ));
        }
        Ok(())
    }
}

impl Engine {
    /// Read permanent proof, not a management cache. A past stop remains valid
    /// after later disk exits; this read does not require current write authority.
    pub fn directory_import_stop_result(
        &self,
        request: &DirectoryImportStopRequest,
    ) -> Result<Option<DirectoryImportStopOutcome>> {
        request.validate()?;
        if let Some(result) = self.state.operations.get(&request.import_operation_id) {
            let native = request.commit_request.as_ref().ok_or_else(|| {
                invalid("published import requires its exact retained native request")
            })?;
            if result.kind != "workspace.commit"
                || !self.native_workspace_commit_completed(native)?
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "original import operation is not an exactly completed native save",
                ));
            }
            return Ok(Some(DirectoryImportStopOutcome::Published(result.clone())));
        }
        let Some(result) = self.state.operations.get(&request.stop_operation_id) else {
            return Ok(None);
        };
        let digest = Self::native_request_digest(STOP_KIND, &request.principal, &request.input())?;
        if result.kind != STOP_KIND
            || !matches!(
                result.commit_state,
                crate::CommitState::Published
                    | crate::CommitState::PublishedDegraded
                    | crate::CommitState::Acked
            )
            || result.request_digest != digest
            || self
                .state
                .operation_principals
                .get(&request.stop_operation_id)
                != Some(&request.principal)
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "stop receipt belongs to another actor or retained import request",
            ));
        }
        Ok(Some(DirectoryImportStopOutcome::Stopped(result.clone())))
    }

    /// Close an uncommitted retained workspace without deleting its bytes, then
    /// publish an exact stop receipt. Any failure retains evidence for this same
    /// request. A missing preallocated workspace is never recreated.
    pub fn stop_directory_import(
        &mut self,
        request: &DirectoryImportStopRequest,
    ) -> Result<DirectoryImportStopOutcome> {
        self.require_write()?;
        if let Some(result) = self.directory_import_stop_result(request)? {
            return Ok(result);
        }
        require_import_not_stopped(&self.state, request.import_operation_id)?;
        if super::import::primary_identity(self, &request.target.slot_id)? != request.target {
            return Err(Error::new(
                ErrorCode::PlanStale,
                "import target registration changed",
            ));
        }
        let root = if let Some(operation) = request.root_operation_id {
            if self.state.operations.contains_key(&operation) {
                // Existing-ID replay verifies the canonical creation request and
                // owner and performs no new root write.
                Some(self.create_disk_root(
                    &request.target.slot_id,
                    &request.principal,
                    operation,
                )?)
            } else {
                if request.workspace_id.is_some() {
                    return Err(Error::new(
                        ErrorCode::RecoveryRequired,
                        "workspace intent has no corresponding root creation receipt",
                    ));
                }
                None
            }
        } else {
            None
        };
        let workspace = match &request.workspace_id {
            Some(id) => {
                if self.workspace_has_preparation(id) {
                    return Err(Error::new(
                        ErrorCode::CommitPreparationBusy,
                        "an active upload or commit preparation owns the import workspace",
                    ));
                }
                match self.workspace(id) {
                    Ok(workspace) => {
                        validate_workspace(self, request, &workspace, root.as_ref())?;
                        Some(workspace)
                    }
                    Err(error) if error.code() == ErrorCode::WorkspaceNotFound => {
                        if request.commit_request.is_some() {
                            return Err(Error::new(
                                ErrorCode::RecoveryRequired,
                                "retained native request has no admitted workspace",
                            ));
                        }
                        None
                    }
                    Err(error) => return Err(error),
                }
            }
            None => None,
        };
        if let Some(workspace) = &workspace
            && workspace.state != WorkspaceState::Aborted
        {
            self.abort_workspace(&workspace.workspace_id, &request.principal)?;
        }
        let result = self.mutate(
            STOP_KIND,
            &request.principal,
            request.stop_operation_id,
            request.input(),
            |_, _, _| Ok(json!({
                "import_operation_id": request.import_operation_id,
                "target": request.target,
                "root_operation_id": request.root_operation_id,
                "workspace_id": request.workspace_id,
                "workspace_retention": if workspace.is_some() { "aborted" } else { "not_created" },
                "source_preserved": true,
                "target_preserved": true,
            })),
        )?;
        Ok(DirectoryImportStopOutcome::Stopped(result))
    }
}

fn validate_workspace(
    engine: &Engine,
    request: &DirectoryImportStopRequest,
    workspace: &Workspace,
    root: Option<&crate::Repository>,
) -> Result<()> {
    if workspace.principal != request.principal {
        return Err(Error::new(
            ErrorCode::PermissionDenied,
            "import workspace belongs to another principal",
        ));
    }
    let repository = engine.repository_for_branch(&workspace.branch_id)?;
    if !repository.file_manager_root
        || repository.primary_slot_id != request.target.slot_id
        || repository.main_branch_id != workspace.branch_id
        || root.is_some_and(|root| {
            root.repository_id != repository.repository_id
                || root.main_branch_id != workspace.branch_id
        })
    {
        return Err(invalid(
            "retained workspace does not belong to the original target disk root",
        ));
    }
    if workspace.state == WorkspaceState::Committed
        || engine.state.operations.values().any(|result| {
            result.kind == "workspace.commit"
                && result
                    .result
                    .get("workspace_id")
                    .and_then(json_codec::Value::as_str)
                    == Some(workspace.workspace_id.as_str())
        })
    {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "workspace has a publication other than the exact original import receipt",
        ));
    }
    if let Some(native) = &request.commit_request {
        let fence_matches = match workspace.state {
            WorkspaceState::Open | WorkspaceState::Sealed => {
                workspace.fencing_token == native.fencing_token
            }
            WorkspaceState::Aborted => native
                .fencing_token
                .checked_add(1)
                .is_some_and(|fence| workspace.fencing_token >= fence),
            WorkspaceState::Committed => false,
        };
        if native.expected_head != workspace.base_head || !fence_matches {
            return Err(invalid(
                "retained native request differs from the import workspace baseline",
            ));
        }
    }
    Ok(())
}

pub(crate) fn require_import_not_stopped(
    state: &crate::State,
    operation: OperationId,
) -> Result<()> {
    for result in state
        .operations
        .values()
        .filter(|result| result.kind == STOP_KIND)
    {
        let stopped: OperationId = json_codec::from_value(
            result
                .result
                .get("import_operation_id")
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::StateInvalid,
                        "stop receipt has no import operation",
                    )
                })?
                .clone(),
        )?;
        if stopped == operation {
            return Err(Error::new(
                ErrorCode::OperationIdConflict,
                format!(
                    "import was permanently stopped by {}; use its original stop receipt",
                    result.operation_id
                ),
            ));
        }
    }
    Ok(())
}
