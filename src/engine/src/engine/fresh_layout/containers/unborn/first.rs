//! A first historical node requires the actual predecessor's merged material.
//! Receipt descriptions cannot turn truncated extent hashes into a full digest.
use crate::business::file_draft::{invalid, unborn};
use crate::container_storage::{HeldUnbornReader, ObservedContainerReplica, PreparedUnbornFull};
use crate::engine::{PlanningContext, Prepared};
use crate::persist::file_draft::unborn_first as codec;
use crate::{OperationResult, Result, State, ValidatedConfig};
use std::sync::Arc;
use storage_contract::file_container::FileContainerRecord;
use storage_contract::{DiskId, FileId, Principal};

pub(super) fn is_first(kind: &str) -> bool {
    kind == codec::KIND || kind == crate::persist::snapshot_worktree::ADDED_FIRST
}

fn description(
    previous: &State, result: &OperationResult,
) -> Result<(FileId, crate::business::file_draft::PublishedContent,
    Option<crate::persist::snapshot_worktree::added_commit::AddedFull>)>
{
    if result.kind == codec::KIND {
        let decoded = codec::decode(previous, result)?;
        return Ok((decoded.selection().target.file, decoded.content().clone(), None));
    }
    let decoded = crate::persist::snapshot_worktree::decode(result)
        .map_err(|source| crate::business::file_draft::worktree::failure(
            crate::snapshot_worktree::RegistrationError::Codec(source),
        ))?.ok_or_else(invalid)?;
    let crate::persist::snapshot_worktree::Record::PublishAddedFile(first) = decoded.record
        else { return Err(invalid()); };
    let principal = previous.operation_principals.get(&first.reserve)
        .ok_or_else(invalid)?.parse::<Principal>().map_err(|_| invalid())?;
    let plan =
        crate::business::file_draft::worktree::first::original(previous, &first, &principal)?;
    let record = crate::business::file_draft::worktree::selected(previous, &plan.added.target)?;
    if record.working.unborn() != Some(&plan.added.working)
        || result.operation_id != plan.file_operation
    { return Err(invalid()); }
    Ok((plan.added.target.file, first.content, Some(first.full)))
}

enum Source {
    Live(Arc<PreparedUnbornFull>),
    Recovery(Arc<HeldUnbornReader>),
}

impl Source {
    fn digest(&self) -> Result<crate::container_storage::ReadWorkingDigest<'_>> {
        match self {
            Self::Live(full) => full.digest(),
            Self::Recovery(reader) => reader.full_digest(),
        }
    }

    fn verify(&self) -> Result<()> {
        match self {
            Self::Live(full) => full.require_source(),
            Self::Recovery(reader) => reader.require_source(),
        }
    }
}

/// Only the complete exact transition plus actual predecessor bytes creates
/// this proof. It retains those immutable extents through installation/recovery.
pub(crate) struct VerifiedUnbornFirst {
    file: FileId,
    record: FileContainerRecord,
    source: Source,
}

impl VerifiedUnbornFirst {
    pub(crate) fn file(&self) -> FileId { self.file }
    pub(crate) fn owner(&self) -> DiskId { self.record.owner_disk }

    pub(crate) fn require_record(&self, file: FileId, record: &FileContainerRecord) -> Result<()> {
        if file != self.file || record != &self.record { return Err(invalid()); }
        self.verify()
    }

    pub(crate) fn verify(&self) -> Result<()> { self.source.verify() }
}

fn verified(
    config: &ValidatedConfig,
    previous: &State,
    next: &State,
    result: &OperationResult,
    source: Source,
) -> Result<Arc<VerifiedUnbornFirst>> {
    let principal = next.operation_principals.get(&result.operation_id)
        .ok_or_else(invalid)?        .parse::<Principal>()        .map_err(|_| invalid())?;
    let read = source.digest()?;
    let (expected, file) = if result.kind == crate::persist::snapshot_worktree::ADDED_FIRST {
        crate::business::file_draft::worktree::first::replay(
            config, previous, result, &principal, &read,
        )?
    } else {
        let projected = unborn::replay_first(config, previous, result, &principal, &read)?;
        let file = projected.target().file;
        (projected.into_state(), file)
    };
    if !crate::publication_shape::same_transition(previous, &expected, next) {
        return Err(invalid());
    }
    let record = next.file_containers.get(&file).ok_or_else(invalid)?.clone();
    read.require_source()?;
    drop(read);
    let result = Arc::new(VerifiedUnbornFirst { file, record, source });
    result.verify()?;
    Ok(result)
}

pub(in crate::engine::fresh_layout::containers) fn plan(
    context: &PlanningContext<'_>, next: &State, prepared: &[Prepared], result: &OperationResult,
) -> Result<Option<Arc<VerifiedUnbornFirst>>> {
    if !is_first(&result.kind) { return Ok(None); }
    let [prepared] = prepared else { return Err(invalid()); };
    let (_, content, full) = description(context.state, result)?;
    if prepared.version != content.version
        || prepared.placement != content.placement
        || context.current_namespaces.is_some()
    {
        return Err(invalid());
    }
    let material = prepared.source.require_working_full()?;
    material.require_context(context)?;
    if let Some(full) = full {
        if material.worktree_evidence()? != full { return Err(invalid()); }
    }
    verified(context.config, context.state, next, result, Source::Live(Arc::clone(material)))
        .map(Some)
}

pub(in crate::engine::fresh_layout::containers) fn candidate(
    config: &ValidatedConfig,
    resources: &Arc<crate::disk::DiskResources>,
    previous: &State,
    next: &State,
    candidate: &crate::journal::Candidate,
    locations: Option<&super::super::RecoveryRenameLocations<'_>>,
) -> Result<Option<Arc<VerifiedUnbornFirst>>> {
    let result = next.operations.get(&candidate.operation_id).ok_or_else(invalid)?;
    if !is_first(&result.kind) { return Ok(None); }
    let [prepared] = candidate.versions.as_slice() else { return Err(invalid()); };
    let (file, content, _) = description(previous, result)?;
    if config.generation().is_some() || candidate.generation().is_some()
        || candidate.sequence != next.sequence || result.sequence != candidate.sequence
        || result.request_digest != candidate.request_digest
        || prepared.version != content.version
        || prepared.placement != content.placement
    {
        return Err(invalid());
    }
    if result.kind == codec::KIND {
        let selection = codec::decode(previous, result)?.selection();
        crate::business::file_draft::selected::require_all(
            previous, std::slice::from_ref(&selection), config.max_entries, config.max_file_bytes,
        )?;
    }
    let record = previous.file_containers.get(&file).ok_or_else(invalid)?;
    let disk = config.disks.iter().find(|disk| disk.disk_id == record.owner_disk.to_string())
        .ok_or_else(invalid)?;
    // This new Initial one-owner mode requires its actual predecessor extents.
    // Unavailable is not empty, a different copy, or permission to infer bytes.
    let observed = match locations {
        Some(locations) => locations.observe_previous(
            config, resources, previous, next, candidate, file, disk,
        )?,
        None => {
            let source = super::super::published_source(
                config, resources, previous, file, disk,
            )?;
            ObservedContainerReplica::observe(source)?
        }
    };
    let observed = Arc::new(observed);
    let reader = Arc::new(observed.open_unborn_working(config.max_file_bytes)?);
    verified(config, previous, next, result, Source::Recovery(reader)).map(Some)
}

/// The complete batch already owns one canonical accumulated projection and
/// exact prepared set. Retain each real FULL on its existing owner occurrence;
/// this does not invoke or manufacture a single-file operation receipt.
pub(in crate::engine::fresh_layout::containers) fn working_batch(
    context: &PlanningContext<'_>,
    source: &crate::engine::file_draft::WorkingBatchHandoff,
    prepared: &[Prepared],
) -> Result<std::collections::BTreeMap<FileId, Arc<VerifiedUnbornFirst>>> {
    source.require_context(context)?;
    source.require_prepared(prepared)?;
    let mut first = std::collections::BTreeMap::new();
    for item in source.items() {
        match item.origin() {
            crate::engine::file_draft::WorkingBatchOrigin::Born(_) => {}
            crate::engine::file_draft::WorkingBatchOrigin::First { .. } => {
                let file = item.target().file;
                let full = source.first_material(file).ok_or_else(invalid)?;
                let record = source.state().file_containers.get(&file).ok_or_else(invalid)?;
                let proof = Arc::new(VerifiedUnbornFirst {
                    file, record: record.clone(), source: Source::Live(Arc::clone(full)),
                });
                proof.verify()?;
                first.insert(file, proof);
            }
        }
    }
    Ok(first)
}

/// Only the full directory verifier can provide these predecessor readers.
/// Its proof remains bound to the exact original before/after pair and receipt.
pub(in crate::engine::fresh_layout::containers) fn directory(
    previous: &State, next: &State, result: &OperationResult,
    directory: &crate::snapshot_worktree::drafts::VerifiedDirectoryDraft<'_>,
) -> Result<std::collections::BTreeMap<FileId, Arc<VerifiedUnbornFirst>>> {
    directory.require_pair(previous, next, result)
        .map_err(crate::snapshot_worktree::drafts::DirectoryPublicationError::into_engine)?;
    let mut first = std::collections::BTreeMap::new();
    for file in directory.working_files() {
        if let Some(reader) = directory.first_source(file) {
            let record = next.file_containers.get(&file).ok_or_else(invalid)?;
            let proof = Arc::new(VerifiedUnbornFirst {
                file, record: record.clone(), source: Source::Recovery(Arc::clone(reader)),
            });
            proof.verify()?;
            first.insert(file, proof);
        }
    }
    Ok(first)
}
