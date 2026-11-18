//! The owning SYSTEM receipt, not an observed ledger tail, selects each revision.
use crate::directory_worktree::ledger::{
    EditLeaf, EditRecord, LedgerError, merge_original_edits, revision_commitment_with_files,
};
use crate::persist::snapshot_worktree::{EditCompletion, EditReservation};
use crate::snapshot_worktree::registry::{
    CompletionOwner, Publication, Receipt, ReceiptIndex, require_completion,
};
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::{SnapshotEntryContent, SnapshotFile, SnapshotFileSource};
use storage_contract::snapshot_worktree::{EditWorktree, WorktreeHandle, WorktreeRevision};
use storage_contract::working_tree::MaterialCommitment;
use storage_contract::Principal;

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

pub(in crate::snapshot_worktree) enum EditRetry {
    Reserved,
    Complete(WorktreeRevision),
}

impl<'a> ReceiptIndex<'a> {
    /// Each explicit save addresses a real original occurrence at its exact
    /// spelling. Its parents come from the immutable source, not a path search.
    pub(in crate::snapshot_worktree) fn require_edit_scope(
        &self,
        target: WorktreeHandle,
        path: &name_policy::PortablePath,
        expected: WorktreeRevision,
    ) -> Result<()> {
        let current = self.worktree(target)?;
        if current.revision() != expected || current.published_node().is_some() {
            return Err(RegistrationError::EditScope);
        }
        let entries = current.source().manifest().entries();
        let selected = entries.binary_search_by(|entry| entry.path.key().cmp(path.key()))
            .map_err(|_| RegistrationError::EditScope)?;
        if entries[selected].path != *path || !matches!(entries[selected].content,
            SnapshotEntryContent::File { source: SnapshotFileSource::Container { .. }, .. })
        {
            return Err(RegistrationError::EditScope);
        }
        Ok(())
    }

    pub(in crate::snapshot_worktree) fn retry_edit(
        &self,
        request: &EditWorktree,
        principal: &Principal,
    ) -> Result<Option<EditRetry>> {
        if !self.source.operations.contains_key(&request.operation_id) {
            return Ok(None);
        }
        let edit = self.edits.get(&request.operation_id)
            .ok_or(RegistrationError::Operation(request.operation_id))?;
        if &edit.plan.request != request || &edit.receipt.principal != principal {
            return Err(RegistrationError::Digest(request.operation_id));
        }
        Ok(Some(match &edit.publication {
            Publication::Reserved => EditRetry::Reserved,
            Publication::Complete(completion) => EditRetry::Complete(completion.revision),
        }))
    }

    pub(super) fn reserve_edit(
        &mut self,
        receipt: Receipt<'a>,
        plan: EditReservation,
        commitment: MaterialCommitment,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        if plan.request.operation_id != operation {
            return Err(RegistrationError::Operation(operation));
        }
        self.require_edit_scope(
            plan.request.target, &plan.request.path, plan.request.expected,
        )?;
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
        {
            return Err(RegistrationError::Source);
        }
        let mut replacements = self.active_edits(plan.request.target).iter()
            .filter(|edit| edit.path != plan.request.path)
            .map(|edit| (edit.path.clone(), edit.material)).collect::<Vec<_>>();
        replacements.push((plan.request.path.clone(), SnapshotFile {
            file_id: plan.original.file_id, version_id: plan.version,
            size: plan.request.size, content_hash: plan.request.content_hash,
        }));
        current.source().manifest().validate_material_replacements(&replacements)
            .map_err(RegistrationError::Manifest)?;
        if self.edit_versions.insert(plan.version, operation).is_some() {
            return Err(RegistrationError::Source);
        }
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

    pub(in crate::snapshot_worktree) fn active_edits(&self, target: WorktreeHandle) -> &[EditRecord] {
        self.active_file_edits.get(&target.worktree_id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Look up the actual reserve and completion of this active version. The
    /// version allocation index is built from the real ordered original receipts.
    pub(in crate::snapshot_worktree) fn original_edit(
        &self, target: WorktreeHandle, edit: &EditRecord,
    ) -> Result<(&EditReservation, &EditCompletion)> {
        let entry = self.edit_versions.get(&edit.material.version_id)
            .and_then(|reserve| self.edits.get(reserve)).ok_or(RegistrationError::Source)?;
        match &entry.publication {
            Publication::Complete(complete) if entry.plan.request.target == target
                && complete.edit == *edit => Ok((&entry.plan, complete)),
            _ => Err(RegistrationError::Source),
        }
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
        let merged = merge_original_edits(
            source.manifest(), self.active_edits(plan.request.target),
            std::slice::from_ref(&completion.edit),
        ).map_err(RegistrationError::Ledger)?;
        let expected = WorktreeRevision {
            generation: plan.request.expected.generation.next()
                .map_err(|source| RegistrationError::Ledger(LedgerError::Lifecycle(
                    crate::directory_worktree::lifecycle::LifecycleError::Generation(source),
                )))?,
            manifest: revision_commitment_with_files(
                source.manifest(), &merged, &[],
                self.added(plan.request.target),
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
        self.active_file_edits.insert(worktree, merged);
        self.pending_edits.remove(&worktree);
        reserved.publication = Publication::Complete(completion);
        Ok(())
    }
}
