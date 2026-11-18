//! The immutable edit's reserved version is consumed only by the real FILE
//! constructor for the same successful directory reservation and exact command.
use crate::engine::PlanningContext;
use crate::persist::snapshot_worktree::{CommitReservation, FileRequestCommitment};
use crate::snapshot_worktree::SnapshotSourceError;
use crate::{CommitRequest, Error, Result, State, Workspace};
use name_policy::PortablePath;
use std::sync::Arc;
use storage_contract::{ContentHash, VersionId};

enum Consumption { Pending, Admitted, Consumed }

enum Command {
    Workspace {
        operation: storage_contract::OperationId,
        request: FileRequestCommitment,
        branch: storage_contract::BranchId,
        base: storage_contract::CommitId,
    },
    ContentBatch,
}

pub(crate) struct ReservedFileVersion {
    source: Arc<State>,
    edit: crate::directory_worktree::ledger::EditRecord,
    original: crate::working_tree::snapshot::SnapshotFile,
    history: storage_contract::HistoryTreeId,
    command: Command,
    consumption: Consumption,
}

impl ReservedFileVersion {
    pub(super) fn new(
        source: Arc<State>, plan: CommitReservation, branch: storage_contract::BranchId,
    ) -> Self {
        Self { source, original: plan.original, history: plan.file_history, edit: plan.edit,
            command: Command::Workspace { operation: plan.file_operation, request: plan.file_request,
                branch, base: plan.file_base }, consumption: Consumption::Pending }
    }

    pub(super) fn for_original(source: &Arc<State>, row: &super::content::OriginalRow) -> Self {
        Self { source: Arc::clone(source), original: row.original, history: row.history,
            edit: row.edit.clone(), command: Command::ContentBatch, consumption: Consumption::Pending }
    }

    fn invalid(&self) -> Error {
        Error::SnapshotSource(Box::new(SnapshotSourceError::ReservedVersion(
            self.original.file_id,
        )))
    }

    pub(crate) fn require_request(
        &self,
        context: &PlanningContext<'_>,
        workspace: &Workspace,
        request: &CommitRequest,
    ) -> Result<()> {
        let Command::Workspace { operation, request: original, branch, base } = &self.command
            else { return Err(self.invalid()); };
        let digest = FileRequestCommitment::capture(workspace, request)
            .map_err(|source| Error::SnapshotSource(Box::new(
                SnapshotSourceError::Encoding(source),
            )))?;
        if !std::ptr::eq(self.source.as_ref(), context.state.as_ref())
            || request.operation_id != *operation
            || digest != *original
            || workspace.branch_id != branch.to_string()
            || workspace.base_head != base.to_string()
            || workspace.files.len() != 1 || !workspace.namespace_operations.is_empty()
            || !matches!(self.consumption, Consumption::Pending)
        {
            return Err(self.invalid());
        }
        let file = workspace.files.values().next().ok_or_else(|| self.invalid())?;
        if file.path != self.edit.path
            || file.base_version_id.as_deref()
                != Some(self.original.version_id.to_string().as_str())
            || file.size != self.edit.material.size
            || file.content_hash != self.edit.material.content_hash
        {
            return Err(self.invalid());
        }
        Ok(())
    }

    pub(crate) fn require_build(
        &mut self,
        context: &PlanningContext<'_>,
        history: &str,
        file: &str,
        path: &PortablePath,
        parents: &[String],
        size: u64,
        hash: ContentHash,
    ) -> Result<()> {
        if !std::ptr::eq(self.source.as_ref(), context.state.as_ref())
            || !matches!(self.consumption, Consumption::Pending)
            || self.source.versions.contains_key(&self.edit.material.version_id.to_string())
            || history != self.history.to_string()
            || file != self.original.file_id.to_string() || path != &self.edit.path
            || parents != [self.original.version_id.to_string()]
            || size != self.edit.material.size || hash != self.edit.material.content_hash
        {
            return Err(self.invalid());
        }
        self.consumption = Consumption::Admitted;
        Ok(())
    }

    pub(crate) fn consume_version(&mut self) -> Result<VersionId> {
        if !matches!(self.consumption, Consumption::Admitted) {
            return Err(self.invalid());
        }
        self.consumption = Consumption::Consumed;
        Ok(self.edit.material.version_id)
    }

    pub(crate) fn require_consumed(&self) -> Result<()> {
        if !matches!(self.consumption, Consumption::Consumed) {
            return Err(self.invalid());
        }
        Ok(())
    }
}
