//! The immutable edit's reserved version is consumed only by the real FILE
//! constructor for the same successful directory reservation and exact command.
use crate::engine::PlanningContext;
use crate::persist::snapshot_worktree::{CommitReservation, FileRequestCommitment};
use crate::snapshot_worktree::SnapshotSourceError;
use crate::working_tree::snapshot::SnapshotFileSource;
use crate::{CommitRequest, Error, Result, State, Workspace};
use name_policy::PortablePath;
use std::sync::Arc;
use storage_contract::{ContentHash, VersionId};

enum Consumption { Pending, Admitted, Consumed }

pub(crate) struct ReservedFileVersion {
    source: Arc<State>,
    plan: CommitReservation,
    consumption: Consumption,
}

impl ReservedFileVersion {
    pub(super) fn new(source: Arc<State>, plan: CommitReservation) -> Self {
        Self { source, plan, consumption: Consumption::Pending }
    }

    fn invalid(&self) -> Error {
        Error::SnapshotSource(Box::new(SnapshotSourceError::ReservedVersion(
            self.plan.original.file_id,
        )))
    }

    pub(crate) fn require_request(
        &self,
        context: &PlanningContext<'_>,
        workspace: &Workspace,
        request: &CommitRequest,
    ) -> Result<()> {
        let SnapshotFileSource::Container { branch, .. } = self.plan.original_source else {
            return Err(self.invalid());
        };
        let digest = FileRequestCommitment::capture(workspace, request)
            .map_err(|source| Error::SnapshotSource(Box::new(
                SnapshotSourceError::Encoding(source),
            )))?;
        if !std::ptr::eq(self.source.as_ref(), context.state.as_ref())
            || request.operation_id != self.plan.file_operation
            || digest != self.plan.file_request
            || workspace.branch_id != branch.to_string()
            || workspace.base_head != self.plan.file_base.to_string()
            || workspace.files.len() != 1 || !workspace.namespace_operations.is_empty()
            || !matches!(self.consumption, Consumption::Pending)
        {
            return Err(self.invalid());
        }
        let file = workspace.files.values().next().ok_or_else(|| self.invalid())?;
        if file.path != self.plan.edit.path
            || file.base_version_id.as_deref()
                != Some(self.plan.original.version_id.to_string().as_str())
            || file.size != self.plan.edit.material.size
            || file.content_hash != self.plan.edit.material.content_hash
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
            || self.source.versions.contains_key(&self.plan.edit.material.version_id.to_string())
            || history != self.plan.file_history.to_string()
            || file != self.plan.original.file_id.to_string() || path != &self.plan.edit.path
            || parents != [self.plan.original.version_id.to_string()]
            || size != self.plan.edit.material.size || hash != self.plan.edit.material.content_hash
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
        Ok(self.plan.edit.material.version_id)
    }

    pub(crate) fn require_consumed(&self) -> Result<()> {
        if !matches!(self.consumption, Consumption::Consumed) {
            return Err(self.invalid());
        }
        Ok(())
    }
}
