//! Added files enter through the same native FILE birth and extent publisher.
//! Native publication precedes the instance frame; independent CompleteFile is
//! the only boundary which makes the new occurrence/save visible to another open.
use crate::business::file_draft::worktree as business;
use crate::container_storage::{HeldUnbornReader, ObservedContainerReplica, PreparedWorkingSave};
use crate::directory_worktree::ledger::{AddedFile, HeldWorktreeLedger};
use crate::engine::MutationPlan;
use crate::persist::snapshot_worktree::files::{
    WorktreeFileBirth, WorktreeFileCompletion, WorktreeFileSave, WorktreeFileSaveRequest,
};
use crate::persist::snapshot_worktree::{PublicationEncoding, Record};
use crate::snapshot_worktree::{RegistrationError, Result, publication};
use crate::{Engine, NativeWorkingFile, OperationResult};
use std::sync::Arc;
use storage_contract::file_draft::UnbornWorking;
use storage_contract::snapshot_worktree::WorktreeRevision;
use storage_contract::snapshot_worktree::additions::{CreateWorktreeFile, WorktreeFileTarget};
use storage_contract::{EntryId, FileId, OperationId, Principal, Sequence};

pub(crate) fn create(
    engine: &mut Engine,
    request: &CreateWorktreeFile,
    principal: &Principal,
) -> Result<WorktreeFileTarget> {
    let receipts = engine.snapshot_receipt_source()?;
    let index = receipts.index()?;
    if let Some(target) = index.retry_file_birth(request, principal)? {
        return Ok(target);
    }
    engine
        .planning_context()
        .require_write()
        .map_err(RegistrationError::Publication)?;
    if engine.current_namespaces.is_some() || engine.config.generation().is_some() {
        return Err(RegistrationError::InitialOnly);
    }
    let current = index.worktree(request.target)?;
    if current.revision() != request.expected
        || current.published_node().is_some()
        || !crate::container_storage::namespace_host::ordinary_path(&request.path)
    {
        return Err(RegistrationError::EditScope);
    }
    let source = index.reopen_node(engine, current.source().handle())?;
    let session = current.reopen(&source)?;
    let mut ledger = session.into_ledger();
    let repository = current.source().host().namespace.repository_id;
    let branch = current.source().host().namespace.branch_id;
    let owner = engine
        .namespace_host_root(repository, branch)
        .map_err(RegistrationError::Publication)?;
    let disk = owner
        .disk()
        .disk_id
        .parse()
        .map_err(RegistrationError::Allocation)?;
    let pool = engine
        .config
        .pool_id
        .parse()
        .map_err(RegistrationError::Allocation)?;
    let empty = crate::retained_names::RetainedDiskRegistry::new(pool);
    let registry = index.retained_registry().unwrap_or(&empty);
    let name = name_policy::Name::parse(request.path.file_name())
        .map_err(|source| RegistrationError::Publication(source.into()))?;
    let allocation = crate::retained_names::allocate_file(
        registry,
        owner.deleted().map_err(RegistrationError::Publication)?,
        disk,
        request.operation_id,
        &name,
        index.retained_names(disk),
        engine.config.max_entries,
    )
    .map_err(RegistrationError::RetainedName)?;
    let target = WorktreeFileTarget {
        worktree: request.target,
        path: request.path.clone(),
        entry: EntryId::generate().map_err(RegistrationError::Allocation)?,
        file: FileId::generate().map_err(RegistrationError::Allocation)?,
    };
    let added = AddedFile {
        target: target.clone(),
        birth: request.operation_id,
        working: UnbornWorking::birth(),
    };
    ledger
        .check_file(request.operation_id, request.expected, &added)
        .map_err(RegistrationError::Ledger)?;
    let root = engine
        .state
        .repositories
        .get(&repository.to_string())
        .ok_or(RegistrationError::Source)?;
    let plan = WorktreeFileBirth {
        request: request.clone(),
        target: target.clone(),
        repository,
        branch,
        history: root
            .history_tree_id
            .parse()
            .map_err(RegistrationError::Allocation)?,
        owner: disk,
        allocation,
        binding: current.binding(),
        before: current.checkpoint(),
        completion: OperationId::generate().map_err(RegistrationError::Allocation)?,
    };
    let mount = current.reservation().clone();
    super::edit::require_mount(engine, &mount)?;
    let record = Record::BirthFile(plan.clone());
    let encoded = PublicationEncoding::new(&record).map_err(RegistrationError::Codec)?;
    let planned = engine
        .plan_mutation_digest(
            record.kind(),
            principal.as_str(),
            request.operation_id,
            encoded.request(),
            None,
            |engine, next, _| {
                *next = business::birth(
                    engine.state(),
                    &engine.config,
                    Sequence::new(engine.planning_context().next_slot),
                    &plan,
                )?;
                Ok(encoded.result())
            },
        )
        .map_err(RegistrationError::Publication)?;
    let MutationPlan::Prepared {
        next,
        prepared,
        result,
    } = planned
    else {
        return Err(RegistrationError::Operation(request.operation_id));
    };
    if !prepared.is_empty() {
        return Err(RegistrationError::PublicationChanged);
    }
    index.require_planned(&next, &result, principal, &record)?;
    engine
        .persist(*next, prepared, result)
        .map_err(RegistrationError::Publication)?;
    complete(
        engine,
        &record,
        plan.completion,
        principal,
        &mut ledger,
        added,
        request.operation_id,
        request.expected,
    )
    .map_err(|source| RegistrationError::DirectoryResponse {
        operation: request.operation_id,
        source: Box::new(source),
    })?;
    Ok(target)
}

pub(crate) fn reader(
    engine: &Engine,
    target: &WorktreeFileTarget,
    writable: bool,
) -> Result<(HeldUnbornReader, WorktreeRevision, u64)> {
    let context = engine.planning_context();
    if writable {
        context
            .require_write()
            .map_err(RegistrationError::Publication)?;
    }
    if context.current_namespaces.is_some() || context.config.generation().is_some() {
        return Err(RegistrationError::InitialOnly);
    }
    let index = context.snapshot_receipts()?;
    let current = index.worktree(target.worktree)?;
    let source = index.reopen_node(engine, current.source().handle())?;
    let session = current.reopen(&source)?;
    let actual = session
        .added_files()
        .iter()
        .find(|file| file.target == *target)
        .ok_or(RegistrationError::Source)?;
    let record =
        business::selected(context.state, target).map_err(RegistrationError::Publication)?;
    if record.created_by != actual.birth
        || record.working.unborn() != Some(&actual.working)
        || record.namespace.repository != current.source().host().namespace.repository_id
        || record.namespace.branch != current.source().host().namespace.branch_id
    {
        return Err(RegistrationError::Source);
    }
    let revision = session.worktree().revision();
    let ledger = session.into_ledger();
    let maximum = ledger
        .projection()
        .map_err(RegistrationError::Ledger)?
        .added_file_budget(target.entry)
        .map_err(|source| {
            RegistrationError::Ledger(crate::directory_worktree::ledger::LedgerError::Projection(
                source,
            ))
        })?
        .min(context.config.max_file_bytes);
    let disk = context
        .config
        .disks
        .iter()
        .find(|disk| disk.disk_id == record.owner_disk.to_string())
        .ok_or(RegistrationError::Source)?;
    let source = crate::engine::fresh_layout::containers::published_source(
        context.config,
        context.disk_resources,
        context.state,
        target.file,
        disk,
    )
    .map_err(RegistrationError::Publication)?;
    let observed = Arc::new(
        ObservedContainerReplica::observe(source).map_err(RegistrationError::Publication)?,
    );
    let reader = observed
        .open_unborn_working(maximum)
        .map_err(RegistrationError::Publication)?;
    Ok((reader, revision, maximum))
}

pub(crate) fn save(
    engine: &mut Engine,
    request: &WorktreeFileSaveRequest,
    principal: &Principal,
    material: Arc<PreparedWorkingSave>,
) -> Result<OperationResult> {
    let operation = request.manifest.operation();
    let receipts = engine.snapshot_receipt_source()?;
    let index = receipts.index()?;
    if let Some(result) = index.retry_file_save(request, principal)? {
        return Ok(result);
    }
    material
        .require_context(&engine.planning_context())
        .map_err(RegistrationError::Publication)?;
    if material
        .worktree_request(&request.target, request.expected)
        .map_err(RegistrationError::Publication)?
        != *request
    {
        return Err(RegistrationError::Source);
    }
    let current = index.worktree(request.target.worktree)?;
    if current.revision() != request.expected || current.published_node().is_some() {
        return Err(RegistrationError::EditScope);
    }
    let source = index.reopen_node(engine, current.source().handle())?;
    let session = current.reopen(&source)?;
    let old = session
        .added_files()
        .iter()
        .find(|file| file.target == request.target)
        .ok_or(RegistrationError::Source)?;
    let added = AddedFile {
        target: request.target.clone(),
        birth: old.birth,
        working: old
            .working
            .saved(
                request.target.file,
                request.manifest.clone(),
                engine.config.max_file_bytes,
            )
            .map_err(|source| {
                RegistrationError::Publication(crate::Error::FileDraft(
                    crate::DraftError::WorkingManifest(source),
                ))
            })?,
    };
    let plan = WorktreeFileSave {
        request: request.clone(),
        birth: old.birth,
        binding: current.binding(),
        before: current.checkpoint(),
        completion: OperationId::generate().map_err(RegistrationError::Allocation)?,
    };
    let mut ledger = session.into_ledger();
    ledger
        .check_file(operation, request.expected, &added)
        .map_err(RegistrationError::Ledger)?;
    super::edit::require_mount(engine, current.reservation())?;
    let record = Record::SaveFile(plan.clone());
    let encoded = PublicationEncoding::new(&record).map_err(RegistrationError::Codec)?;
    let planned = engine
        .plan_mutation_digest(
            record.kind(),
            principal.as_str(),
            operation,
            encoded.request(),
            None,
            |engine, next, _| {
                material.require_context(&engine.planning_context())?;
                *next = business::save(
                    engine.state(),
                    &engine.config,
                    Sequence::new(engine.planning_context().next_slot),
                    &plan,
                )?;
                Ok(encoded.result())
            },
        )
        .map_err(RegistrationError::Publication)?;
    let MutationPlan::Prepared {
        next,
        prepared,
        result,
    } = planned
    else {
        return Err(RegistrationError::Operation(operation));
    };
    if !prepared.is_empty() {
        return Err(RegistrationError::PublicationChanged);
    }
    index.require_planned(&next, &result, principal, &record)?;
    engine
        .persist_worktree_file_save(*next, result.clone(), material)
        .map_err(RegistrationError::Publication)?;
    complete(
        engine,
        &record,
        plan.completion,
        principal,
        &mut ledger,
        added,
        operation,
        request.expected,
    )
    .map_err(|source| RegistrationError::DirectoryResponse {
        operation,
        source: Box::new(source),
    })?;
    Ok(result)
}

fn complete(
    engine: &mut Engine,
    record: &Record,
    completion: OperationId,
    principal: &Principal,
    ledger: &mut HeldWorktreeLedger<'_>,
    added: AddedFile,
    operation: OperationId,
    expected: WorktreeRevision,
) -> Result<()> {
    // The native receipt is already published. Re-observe its exact FILE inode
    // and immutable extents before the ledger can advertise them.
    let target = added.target.clone();
    let current = engine
        .state
        .file_containers
        .get(&target.file)
        .ok_or(RegistrationError::Source)?;
    if current.working.unborn() != Some(&added.working) || current.created_by != added.birth {
        return Err(RegistrationError::PublicationChanged);
    }
    let disk = engine
        .config
        .disks
        .iter()
        .find(|disk| disk.disk_id == current.owner_disk.to_string())
        .ok_or(RegistrationError::Source)?;
    let held = crate::engine::fresh_layout::containers::observe_replica(
        &engine.config,
        &engine.disk_resources,
        engine.state(),
        target.file,
        disk,
    )
    .map_err(RegistrationError::Publication)?;
    held.container().map_err(RegistrationError::Publication)?;
    let checkpoint = ledger
        .file(operation, expected, added.clone())
        .map_err(RegistrationError::Ledger)?;
    let revision = ledger.worktree().revision();
    let completion_record = Record::CompleteFile(WorktreeFileCompletion {
        original: operation,
        commitment: record.commitment().map_err(RegistrationError::Codec)?,
        working: added.working,
        checkpoint,
        revision,
    });
    publication::publish_new(engine, completion, principal, &completion_record)?;
    reader(engine, &target, false)?;
    Ok(())
}

impl Engine {
    pub fn create_directory_worktree_file(
        &mut self,
        request: &CreateWorktreeFile,
        principal: &Principal,
    ) -> std::result::Result<WorktreeFileTarget, super::SnapshotWorktreeError> {
        create(self, request, principal).map_err(Into::into)
    }

    pub fn open_directory_worktree_working(
        &self,
        target: &WorktreeFileTarget,
        principal: &Principal,
        writable: bool,
    ) -> std::result::Result<NativeWorkingFile, super::SnapshotWorktreeError> {
        let (reader, revision, maximum) = reader(self, target, writable)?;
        Ok(NativeWorkingFile::worktree(
            target.clone(),
            principal.clone(),
            reader,
            writable,
            revision,
            maximum,
        ))
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/snapshot_worktree/added_files.rs"]
mod tests;
