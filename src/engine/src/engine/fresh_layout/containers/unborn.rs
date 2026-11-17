//! Complete logical birth, before its one owner occurrence receives I/O feedback.
use crate::business::file_draft::{DraftError as Failure, invalid, unborn};
use crate::engine::{PlanningContext, Prepared};
use crate::journal::Candidate;
use crate::persist::file_draft::birth as codec;
use crate::{Error, OperationResult, Result, State, ValidatedConfig};
use storage_contract::file_container::{ContainerEntity, FileContainerRecord};
use storage_contract::working_tree::RootId;
use storage_contract::{DiskId, FileId, Principal, Sequence};

mod first;
pub(crate) use first::VerifiedUnbornFirst;
pub(super) use first::{
    candidate as first_candidate, plan as first_plan,
    working_batch as first_working_batch, directory as first_directory,
};

pub(super) struct VerifiedUnbornSave {
    file: FileId,
}

impl VerifiedUnbornSave {
    pub(super) fn file(&self) -> FileId { self.file }
}

fn save_transition(
    config: &ValidatedConfig,
    previous: &State,
    next: &State,
    result: &OperationResult,
) -> Result<Option<VerifiedUnbornSave>> {
    if result.kind != crate::persist::file_draft::unborn_save::KIND { return Ok(None); }
    let principal = next.operation_principals.get(&result.operation_id)
        .ok_or_else(invalid)?        .parse::<Principal>()        .map_err(|_| invalid())?;
    let expected = unborn::replay_save(config, previous, result, &principal)?;
    if !crate::publication_shape::same_transition(previous, expected.state(), next) {
        return Err(invalid());
    }
    Ok(Some(VerifiedUnbornSave { file: expected.target().file }))
}

pub(super) fn save_plan(
    context: &PlanningContext<'_>, next: &State, prepared: &[Prepared], result: &OperationResult,
) -> Result<Option<VerifiedUnbornSave>> {
    let Some(proof) = save_transition(context.config, context.state, next, result)? else {
        return Ok(None);
    };
    if context.current_namespaces.is_some() || !prepared.is_empty() {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    let record = context.state.file_containers.get(&proof.file).ok_or_else(invalid)?;
    let source = context.namespace_host_root(record.namespace.repository, record.namespace.branch)?;
    if source.disk().disk_id.parse::<DiskId>()? != record.owner_disk {
        return Err(invalid());
    }
    source.common()?;
    Ok(Some(proof))
}

pub(super) fn save_candidate(
    config: &ValidatedConfig, previous: &State, next: &State, candidate: &Candidate,
) -> Result<Option<VerifiedUnbornSave>> {
    let result = next.operations.get(&candidate.operation_id).ok_or_else(invalid)?;
    let proof = save_transition(config, previous, next, result)?;
    if proof.is_some() && (candidate.generation().is_some() || !candidate.versions.is_empty()
        || next.sequence != candidate.sequence || result.sequence != candidate.sequence
        || result.request_digest != candidate.request_digest)
    {
        return Err(invalid());
    }
    Ok(proof)
}

/// Issued only after reconstructing the complete request/receipt transition.
/// It is not a root or install permission; the ordinary EXCL/held consumer must
/// attach and subsequently recheck the actual owner occurrence.
pub(crate) struct VerifiedUnbornBirth {
    file: FileId,
    logical: FileContainerRecord,
    root: RootId,
    receipt: OperationResult,
}

impl VerifiedUnbornBirth {
    pub(crate) fn file(&self) -> FileId { self.file }
    pub(crate) fn owner(&self) -> DiskId { self.logical.owner_disk }
    pub(crate) fn root(&self) -> RootId { self.root }
    pub(crate) fn sequence(&self) -> Sequence { Sequence::new(self.receipt.sequence) }

    pub(crate) fn require_record(&self, record: &FileContainerRecord) -> Result<()> {
        let mut logical = record.clone();
        logical.replicas.clear();
        if logical != self.logical { return Err(invalid()); }
        Ok(())
    }

    pub(crate) fn require_material(&self, state: &State, file: FileId) -> Result<()> {
        if file != self.file || state.sequence != self.receipt.sequence
            || state.operations.get(&self.receipt.operation_id) != Some(&self.receipt)
        {
            return Err(invalid());
        }
        let record = state.file_containers.get(&file).ok_or_else(invalid)?;
        self.require_record(record)?;
        let replica = record.replicas.get(&self.owner()).ok_or_else(invalid)?;
        if record.replicas.len() != 1 || replica.root_id != self.root
            || !replica.prefixes.is_empty() || replica.entity.inode() == 0
        {
            return Err(invalid());
        }
        Ok(())
    }
}

fn reconstruct(
    config: &ValidatedConfig,
    previous: &State,
    next: &State,
    result: &OperationResult,
) -> Result<(State, VerifiedUnbornBirth)> {
    let principal = next.operation_principals.get(&result.operation_id)
        .ok_or_else(invalid)?        .parse::<Principal>()        .map_err(|_| invalid())?;
    let projected = unborn::replay_birth(config, previous, result, &principal)?;
    let file = projected.target().file;
    let record = projected.state().file_containers.get(&file).ok_or_else(invalid)?.clone();
    let root = config.storage_roots.get(&record.owner_disk).ok_or_else(invalid)?.root_id;
    Ok((projected.into_state(), VerifiedUnbornBirth {
        file, logical: record, root, receipt: result.clone(),
    }))
}

pub(super) fn plan(
    context: &PlanningContext<'_>,
    next: &State,
    prepared: &[Prepared],
    result: &OperationResult,
) -> Result<Option<VerifiedUnbornBirth>> {
    if result.kind != codec::KIND { return Ok(None); }
    if context.current_namespaces.is_some() || !prepared.is_empty() {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    let (expected, proof) = reconstruct(context.config, context.state, next, result)?;
    if !crate::publication_shape::same_transition(context.state, &expected, next) {
        return Err(invalid());
    }
    let source = context.namespace_host_root(
        proof.logical.namespace.repository, proof.logical.namespace.branch,
    )?;
    if source.disk().disk_id.parse::<DiskId>()? != proof.owner()
        || source.binding().root_id != proof.root
    {
        return Err(invalid());
    }
    source.common()?;
    Ok(Some(proof))
}

pub(super) fn candidate(
    config: &ValidatedConfig,
    previous: &State,
    next: &State,
    candidate: &Candidate,
) -> Result<Option<VerifiedUnbornBirth>> {
    let result = next.operations.get(&candidate.operation_id).ok_or_else(invalid)?;
    if result.kind != codec::KIND { return Ok(None); }
    if candidate.generation().is_some() || !candidate.versions.is_empty()
        || next.sequence != candidate.sequence || result.sequence != candidate.sequence
        || result.request_digest != candidate.request_digest
    {
        return Err(invalid());
    }
    let (mut expected, proof) = reconstruct(config, previous, next, result)?;
    proof.require_material(next, proof.file)?;
    let actual = next.file_containers.get(&proof.file).ok_or_else(invalid)?;
    // Only this newly created, one-owner occurrence receives physical feedback.
    // All old records and every other State field stay in the canonical result.
    expected.file_containers.get_mut(&proof.file).ok_or_else(invalid)?.replicas =
        actual.replicas.clone();
    if !crate::publication_shape::same_transition(previous, &expected, next) {
        return Err(invalid());
    }
    Ok(Some(proof))
}

pub(super) fn add_owner(
    context: &PlanningContext<'_>,
    next: &mut State,
    proof: &VerifiedUnbornBirth,
) -> Result<()> {
    let record = next.file_containers.get_mut(&proof.file).ok_or_else(invalid)?;
    proof.require_record(record)?;
    if !record.replicas.is_empty() { return Err(invalid()); }
    let root = context.config.storage_roots.get(&proof.owner()).ok_or_else(invalid)?;
    if root.root_id != proof.root { return Err(invalid()); }
    record.replicas.insert(proof.owner(), storage_contract::file_container::ContainerReplica {
        root_id: proof.root,
        prefixes: Vec::new(),
        preparation: storage_contract::working_tree::DirectoryId::generate()?,
        // Only a budget row. ContainerPreparation::finish replaces this from
        // the actual EXCL descriptor before Candidate/snapshot construction.
        entity: ContainerEntity::new(0, 0),
    });
    Ok(())
}
