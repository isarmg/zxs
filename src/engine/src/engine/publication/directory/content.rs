//! Budget one authorized original-content preparation before EXCL. This
//! budget is neither a commit decision nor a reusable filesystem capability.
use super::super::*;
use super::Failure;
use crate::business::file_draft::worktree::failure;
use crate::snapshot_worktree::{OriginalContentInput, PreparedOriginalContent};
use crate::working_tree::snapshot::{SnapshotNodeBinding, SnapshotReference};
use storage_contract::working_tree::DirectoryId;

pub(crate) struct BudgetedOriginalContent {
    preparation: DirectoryId,
    source: SnapshotNodeBinding,
    target: SnapshotReference,
    operation: OperationId,
    _descriptors: PayloadProofReservation,
}
impl BudgetedOriginalContent {
    pub(crate) fn require(&self, input: &OriginalContentInput) -> Result<()> {
        if self.preparation != input.preparation() || self.source != input.plan().source_binding
            || self.target != input.manifest().reference() || self.operation != input.plan().publish_operation
        { return Err(Failure::ContainerScope.into()); }
        Ok(())
    }
}

impl crate::engine::PlanningContext<'_> {
    pub(crate) fn prepare_sparse_content_node(&self, input: OriginalContentInput, maximum: usize)
        -> Result<(PreparedOriginalContent, State, Vec<Prepared>)>
    {
        input.require_context(self).map_err(failure)?;
        let owner = self.config.disks.iter().find(|d| d.disk_id == input.plan().host.registration.disk_id)
            .ok_or(Failure::Registration)?;
        let mut required = PhysicalRequirements::new(self);
        // FILE may already hold a verified independent full copy left by an
        // interrupted preparation of this frozen version. The native FILE
        // preparer separately reopens and verifies it; never adopt an unknown
        // temporary directory or use a same-hash file in place of the edit.
        drop(self.observe_existing_payloads(input.prepared(), &mut required)?);
        required.add(owner, input.directory_bytes().map_err(failure)?)?;
        let envelope = input.result_budget().map_err(failure)?;
        if envelope > maximum { return Err(Failure::ContainerBudget.into()); }
        let mut additional = envelope as u64;
        let snapshot = prepare_snapshot(input.state(), maximum)?;
        snapshot.visit_prepared_objects(&mut |_, _, bytes| {
            additional = additional.checked_add(bytes.len() as u64).ok_or(Failure::PhysicalBytesOverflow)?;
            Ok(())
        })?;
        let (_, histories) = crate::history_package::prepare_history_manifests(input.state(), maximum)?;
        for history in histories {
            additional = additional.checked_add(history.bytes().len() as u64).ok_or(Failure::PhysicalBytesOverflow)?;
        }
        for member in self.members() { required.add(&member, additional)?; }
        required.check()?;
        let permit = BudgetedOriginalContent { preparation: input.preparation(), source: input.plan().source_binding,
            target: input.manifest().reference(), operation: input.plan().publish_operation,
            _descriptors: payload_proof_budget().reserve(3).ok_or(Failure::ContainerBudget)? };
        input.stage(self, permit).map_err(failure)
    }
}
