//! Native original decisions own added occurrences. A local frame is never a birth.
use super::edits::LedgerHead;
use super::{CompletionOwner, Publication, Receipt, ReceiptIndex, require_completion};
use crate::directory_worktree::ledger::{AddedFile, LedgerError, revision_commitment_with_files};
use crate::persist::snapshot_worktree::files::{
    WorktreeFileBirth, WorktreeFileCompletion, WorktreeFileSave, WorktreeFileSaveRequest,
};
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::SnapshotEntryContent;
use name_policy::PortablePath;
use storage_contract::file_container::retained::{RetainedKind, RetainedName};
use storage_contract::file_draft::UnbornWorking;
use storage_contract::snapshot_worktree::additions::{CreateWorktreeFile, WorktreeFileTarget};
use storage_contract::snapshot_worktree::{WorktreeHandle, WorktreeRevision};
use storage_contract::working_tree::MaterialCommitment;
use storage_contract::{DiskId, OperationId, Principal};

pub(super) enum FilePlan {
    Birth(WorktreeFileBirth),
    Save(WorktreeFileSave),
}
impl FilePlan {
    pub(super) fn target(&self) -> &WorktreeFileTarget {
        match self {
            Self::Birth(plan) => &plan.target,
            Self::Save(plan) => &plan.request.target,
        }
    }
    pub(super) fn expected(&self) -> WorktreeRevision {
        match self {
            Self::Birth(plan) => plan.request.expected,
            Self::Save(plan) => plan.request.expected,
        }
    }
    pub(super) fn completion(&self) -> OperationId {
        match self {
            Self::Birth(plan) => plan.completion,
            Self::Save(plan) => plan.completion,
        }
    }
    pub(super) fn before(&self) -> crate::directory_worktree::ledger::Checkpoint {
        match self {
            Self::Birth(plan) => plan.before,
            Self::Save(plan) => plan.before,
        }
    }
}
pub(super) struct FileRecord<'a> {
    pub(super) receipt: Receipt<'a>,
    pub(super) plan: FilePlan,
    pub(super) added: AddedFile,
    pub(super) commitment: MaterialCommitment,
    pub(super) publication: Publication<WorktreeFileCompletion>,
}

impl<'a> ReceiptIndex<'a> {
    pub(crate) fn retained_registry(&self) -> Option<&crate::retained_names::RetainedDiskRegistry> {
        self.retained.as_ref()
    }

    pub(in crate::snapshot_worktree) fn retained_names(
        &self,
        disk: DiskId,
    ) -> impl Iterator<Item = &RetainedName> {
        self.files
            .values()
            .filter_map(move |record| match &record.plan {
                FilePlan::Birth(plan) if plan.owner == disk => Some(&plan.allocation.name),
                FilePlan::Birth(_) | FilePlan::Save(_) => None,
            })
    }

    pub(in crate::snapshot_worktree) fn added(&self, target: WorktreeHandle) -> &[AddedFile] {
        self.added_files
            .get(&target.worktree_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(in crate::snapshot_worktree) fn retry_file_birth(
        &self,
        request: &CreateWorktreeFile,
        principal: &Principal,
    ) -> Result<Option<WorktreeFileTarget>> {
        if !self.source.operations.contains_key(&request.operation_id) {
            return Ok(None);
        }
        let record = self
            .files
            .get(&request.operation_id)
            .ok_or(RegistrationError::Operation(request.operation_id))?;
        if record.receipt.principal != *principal
            || !matches!(&record.plan, FilePlan::Birth(plan) if plan.request == *request)
        {
            return Err(RegistrationError::Digest(request.operation_id));
        }
        if !matches!(record.publication, Publication::Complete(_)) {
            return Err(RegistrationError::Pending(request.operation_id));
        }
        Ok(Some(record.plan.target().clone()))
    }

    pub(in crate::snapshot_worktree) fn retry_file_save(
        &self,
        request: &WorktreeFileSaveRequest,
        principal: &Principal,
    ) -> Result<Option<crate::OperationResult>> {
        let operation = request.manifest.operation();
        if !self.source.operations.contains_key(&operation) {
            return Ok(None);
        }
        let record = self
            .files
            .get(&operation)
            .ok_or(RegistrationError::Operation(operation))?;
        if record.receipt.principal != *principal
            || !matches!(&record.plan, FilePlan::Save(plan) if plan.request == *request)
        {
            return Err(RegistrationError::Digest(operation));
        }
        if !matches!(record.publication, Publication::Complete(_)) {
            return Err(RegistrationError::Pending(operation));
        }
        Ok(Some(record.receipt.original.clone()))
    }

    pub(super) fn birth_file(
        &mut self,
        receipt: Receipt<'a>,
        plan: WorktreeFileBirth,
        commitment: MaterialCommitment,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        let current = self.worktree(plan.request.target)?;
        let owner = current.source().host();
        let entries = current.source().manifest().entries();
        let original = |path: &PortablePath| {
            entries
                .binary_search_by(|entry| entry.path.key().cmp(path.key()))
                .ok()
                .map(|index| &entries[index])
        };
        if plan.request.operation_id != operation
            || plan.target.worktree != plan.request.target
            || plan.target.path != plan.request.path
            || !crate::container_storage::namespace_host::ordinary_path(&plan.target.path)
            || current.published_node().is_some()
            || current.binding() != plan.binding
            || current.checkpoint() != plan.before
            || current.revision() != plan.request.expected
            || owner.namespace.repository_id != plan.repository
            || owner.namespace.branch_id != plan.branch
            || owner.registration.disk_id != plan.owner.to_string()
            || owner.registration.pool_id != plan.allocation.registration.pool.to_string()
            || plan.allocation.registration.disk != plan.owner
            || original(&plan.target.path).is_some()
            || plan.target.path.parent().is_some_and(|parent| {
                !original(&parent).is_some_and(|entry| {
                    entry.path == parent
                        && matches!(entry.content, SnapshotEntryContent::Directory {})
                })
            })
            || self.added(plan.request.target).iter().any(|file| {
                file.target.path.key() == plan.target.path.key()
                    || file.target.file == plan.target.file
                    || file.target.entry == plan.target.entry
            })
            || self.files.values().any(|record| {
                matches!(&record.plan, FilePlan::Birth(original)
                if original.target.file == plan.target.file
                    || original.target.entry == plan.target.entry
                    || (original.owner == plan.owner
                        && original.allocation.name == plan.allocation.name))
            })
        {
            return Err(RegistrationError::Source);
        }
        let registry = self.retained.get_or_insert_with(|| {
            crate::retained_names::RetainedDiskRegistry::new(plan.allocation.registration.pool)
        });
        registry
            .observe(operation, &plan.allocation.registration)
            .map_err(RegistrationError::RetainedName)?;
        let name = name_policy::Name::parse(plan.request.path.file_name())
            .map_err(|source| RegistrationError::Publication(source.into()))?;
        registry
            .require_allocation(&plan.allocation, &name, RetainedKind::File)
            .map_err(RegistrationError::RetainedName)?;
        let repository = self
            .source
            .repositories
            .get(&plan.repository.to_string())
            .ok_or(RegistrationError::Source)?;
        if repository.history_tree_id != plan.history.to_string() {
            return Err(RegistrationError::Source);
        }
        let added = AddedFile {
            target: plan.target.clone(),
            birth: operation,
            working: UnbornWorking::birth(),
        };
        self.reserve_file(receipt, FilePlan::Birth(plan), commitment, added)
    }

    pub(super) fn save_file(
        &mut self,
        receipt: Receipt<'a>,
        plan: WorktreeFileSave,
        commitment: MaterialCommitment,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        let request = &plan.request;
        let current = self.worktree(request.target.worktree)?;
        let old = self
            .added(request.target.worktree)
            .iter()
            .find(|file| file.target == request.target)
            .ok_or(RegistrationError::Source)?;
        if request.manifest.operation() != operation
            || current.published_node().is_some()
            || current.binding() != plan.binding
            || current.checkpoint() != plan.before
            || current.revision() != request.expected
            || old.birth != plan.birth
            || old.working.revision() != request.working
        {
            return Err(RegistrationError::Source);
        }
        let working = old
            .working
            .saved(
                request.target.file,
                request.manifest.clone(),
                crate::working_tree::snapshot::MAX_LOGICAL_BYTES,
            )
            .map_err(|source| {
                RegistrationError::Publication(crate::Error::FileDraft(
                    crate::DraftError::WorkingManifest(source),
                ))
            })?;
        let added = AddedFile {
            target: request.target.clone(),
            birth: plan.birth,
            working,
        };
        self.reserve_file(receipt, FilePlan::Save(plan), commitment, added)
    }

    fn reserve_file(
        &mut self,
        receipt: Receipt<'a>,
        plan: FilePlan,
        commitment: MaterialCommitment,
        added: AddedFile,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        self.reserve_completion(
            operation,
            plan.completion(),
            CompletionOwner::File(operation),
        )?;
        self.pending_edits
            .insert(plan.target().worktree.worktree_id, operation);
        self.files.insert(
            operation,
            FileRecord {
                receipt,
                plan,
                added,
                commitment,
                publication: Publication::Reserved,
            },
        );
        Ok(())
    }

    pub(super) fn complete_file(
        &mut self,
        receipt: Receipt<'a>,
        completion: WorktreeFileCompletion,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        let reserved = self
            .files
            .get(&completion.original)
            .ok_or(RegistrationError::Completion(operation))?;
        require_completion(
            &reserved.receipt,
            &receipt,
            reserved.plan.completion(),
            reserved.commitment,
            completion.commitment,
        )?;
        let target = reserved.plan.target().worktree;
        let current = self.worktree_for_commit(target, Some(completion.original))?;
        let mut files = self.added(target).to_vec();
        if let Some(old) = files
            .iter_mut()
            .find(|file| file.target.entry == reserved.added.target.entry)
        {
            *old = reserved.added.clone();
        } else {
            files.push(reserved.added.clone());
        }
        files.sort_by_key(|file| file.target.entry);
        let edits = self
            .active_file_edits
            .get(&target.worktree_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let revision = WorktreeRevision {
            generation: reserved
                .plan
                .expected()
                .generation
                .next()
                .map_err(|source| {
                    RegistrationError::Ledger(LedgerError::Lifecycle(
                        crate::directory_worktree::lifecycle::LifecycleError::Generation(source),
                    ))
                })?,
            manifest: revision_commitment_with_files(
                current.source().manifest(),
                edits,
                &[],
                &files,
            )
            .map_err(RegistrationError::Ledger)?,
        };
        let sequence = reserved
            .plan
            .before()
            .sequence
            .next()
            .map_err(|source| RegistrationError::Ledger(LedgerError::Sequence(source)))?;
        if !matches!(reserved.publication, Publication::Reserved)
            || completion.working != reserved.added.working
            || completion.revision != revision
            || completion.checkpoint.sequence != sequence
            || self.pending_edits.get(&target.worktree_id) != Some(&completion.original)
            || !matches!(self.completions.get(&operation), Some(CompletionOwner::File(original))
                if *original == completion.original)
        {
            return Err(RegistrationError::Completion(operation));
        }
        self.heads.insert(
            target.worktree_id,
            LedgerHead {
                checkpoint: completion.checkpoint,
                revision,
            },
        );
        self.pending_edits.remove(&target.worktree_id);
        self.added_files.insert(target.worktree_id, files);
        let original_operation = completion.original;
        self.files
            .get_mut(&original_operation)
            .ok_or(RegistrationError::Source)?
            .publication = Publication::Complete(completion);
        Ok(())
    }
}
