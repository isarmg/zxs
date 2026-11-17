//! The owning SYSTEM receipt, not an observed ledger tail, selects each revision.
use crate::directory_worktree::ledger::{EditLeaf, LedgerError, revision_commitment};
use crate::persist::snapshot_worktree::{EditCompletion, EditReservation};
use crate::snapshot_worktree::registry::{
    Allocations, CompletionOwner, Publication, Receipt, ReceiptIndex, require_completion,
};
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::{SnapshotEntryContent, SnapshotFile, SnapshotFileSource};
use storage_contract::snapshot_worktree::{EditWorktree, WorktreeRevision};
use storage_contract::working_tree::MaterialCommitment;
use storage_contract::{Generation, Principal};

pub(super) struct LedgerHead {
    pub(super) checkpoint: crate::directory_worktree::ledger::Checkpoint,
    pub(super) revision: WorktreeRevision,
}

pub(super) struct EditRecordEntry<'a> {
    pub(super) receipt: Receipt<'a>,
    pub(super) plan: EditReservation,
    pub(super) commitment: MaterialCommitment,
    pub(super) publication: Publication<EditCompletion>,
}

impl<'a> ReceiptIndex<'a> {
    /// A new sealed revision may replace only the same original occurrence.
    /// Namespace/multi-file edits and post-commit rebase have separate contracts.
    pub(in crate::snapshot_worktree) fn require_edit_scope(
        &self,
        request: &EditWorktree,
    ) -> Result<()> {
        let current = self.worktree(request.target)?;
        if current.revision() != request.expected
            || request.path.parent().is_some()
            || current.publication.is_some()
        {
            return Err(RegistrationError::EditScope);
        }
        if request.expected.generation != Generation::new(0) {
            let (previous, _) = self.completed_edit(request.target)?;
            if previous.request.path != request.path {
                return Err(RegistrationError::EditScope);
            }
        }
        Ok(())
    }

    pub(in crate::snapshot_worktree) fn retry_edit(
        &self,
        request: &EditWorktree,
        principal: &Principal,
    ) -> Result<Option<WorktreeRevision>> {
        if !self.source.operations.contains_key(&request.operation_id) {
            return Ok(None);
        }
        let edit = self.edits.get(&request.operation_id)
            .ok_or(RegistrationError::Operation(request.operation_id))?;
        if &edit.plan.request != request || &edit.receipt.principal != principal {
            return Err(RegistrationError::Digest(request.operation_id));
        }
        let Publication::Complete(completion) = &edit.publication else {
            return Err(RegistrationError::Pending(request.operation_id));
        };
        Ok(Some(completion.revision))
    }

    pub(super) fn reserve_edit(
        &mut self,
        receipt: Receipt<'a>,
        plan: EditReservation,
        commitment: MaterialCommitment,
        allocations: &mut Allocations,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        if plan.request.operation_id != operation {
            return Err(RegistrationError::Operation(operation));
        }
        self.require_edit_scope(&plan.request)?;
        let current = self.worktree(plan.request.target)?;
        if current.binding() != plan.binding || current.checkpoint() != plan.before
        {
            return Err(RegistrationError::EditScope);
        }
        let original = current.source().manifest().entries().iter()
            .find(|entry| entry.path == plan.request.path)
            .ok_or(RegistrationError::Source)?;
        if original.content != (SnapshotEntryContent::File {
            material: plan.original, source: plan.original_source,
        }) || !matches!(plan.original_source, SnapshotFileSource::Container { .. })
            || plan.original.version_id == plan.version
            || self.source.versions.get(&plan.version.to_string())
                .is_some_and(|version| version.version_sequence <= receipt.original.sequence)
            || !allocations.edit_versions.insert(plan.version)
        {
            return Err(RegistrationError::Source);
        }
        current.source().manifest().validate_material_replacements(&[(
            plan.request.path.clone(), SnapshotFile {
                file_id: plan.original.file_id, version_id: plan.version,
                size: plan.request.size, content_hash: plan.request.content_hash,
            },
        )]).map_err(RegistrationError::Manifest)?;
        let worktree = plan.request.target.worktree_id;
        self.reserve_completion(
            operation, plan.completion, CompletionOwner::Edit(operation),
        )?;
        self.pending_edits.insert(worktree, operation);
        self.edits.insert(operation, EditRecordEntry {
            receipt, plan, commitment, publication: Publication::Reserved,
        });
        Ok(())
    }

    pub(super) fn complete_edit(
        &mut self,
        receipt: Receipt<'a>,
        completion: EditCompletion,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        let reserved = self.edits.get(&completion.reserve)
            .ok_or(RegistrationError::Completion(operation))?;
        require_completion(
            &reserved.receipt, &receipt, reserved.plan.completion,
            reserved.commitment, completion.reservation,
        )?;
        let plan = &reserved.plan;
        let Some(CompletionOwner::Worktree(worktree_reserve)) =
            self.completions.get(&plan.request.target.publication)
        else {
            return Err(RegistrationError::Source);
        };
        let worktree_source = self.worktrees.get(worktree_reserve)
            .ok_or(RegistrationError::Source)?.plan.request.source;
        let source = self.node(worktree_source)?;
        let expected = WorktreeRevision {
            generation: plan.request.expected.generation.next()
                .map_err(|source| RegistrationError::Ledger(LedgerError::Lifecycle(
                    crate::directory_worktree::lifecycle::LifecycleError::Generation(source),
                )))?,
            manifest: revision_commitment(
                source.manifest(), std::slice::from_ref(&completion.edit), &[],
            ).map_err(RegistrationError::Ledger)?,
        };
        let next = plan.before.sequence.next()
            .map_err(|source| RegistrationError::Ledger(LedgerError::Sequence(source)))?;
        if !matches!(reserved.publication, Publication::Reserved)
            || !matches!(self.completions.get(&operation),
                Some(CompletionOwner::Edit(reserve)) if *reserve == completion.reserve)
            || self.pending_edits.get(&plan.request.target.worktree_id) != Some(&completion.reserve)
            || completion.edit.path != plan.request.path
            || completion.edit.material.file_id != plan.original.file_id
            || completion.edit.material.version_id != plan.version
            || completion.edit.material.size != plan.request.size
            || completion.edit.material.content_hash != plan.request.content_hash
            || completion.edit.leaf != EditLeaf::for_version(plan.version, &plan.request.path)
            || completion.edit.entity.inode() == 0
            || completion.checkpoint.sequence != next || completion.revision != expected
        {
            return Err(RegistrationError::Completion(operation));
        }
        let worktree = plan.request.target.worktree_id;
        let reserved = self.edits.get_mut(&completion.reserve)
            .ok_or(RegistrationError::Completion(operation))?;
        self.heads.insert(worktree, LedgerHead {
            checkpoint: completion.checkpoint, revision: completion.revision,
        });
        self.pending_edits.remove(&worktree);
        reserved.publication = Publication::Complete(completion);
        Ok(())
    }
}
