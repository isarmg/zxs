//! Original-snapshot verification and a read-only cold node observation.
//! Keeping this loan prevents replacing the original receipt with a visible
//! recovered outcome while its directory/FILE proof is being consumed.
use super::publication::DirectoryPublicationError as Failure;
use super::DirectoryDraftError;
use crate::engine::PlanningContext;
use crate::journal::Candidate;
use crate::persist::directory_draft_commit::native::{self, Envelope};
use crate::publication_evidence::AdmittedSnapshot;
use crate::working_tree::snapshot::{HeldSnapshotNode, PinnedSnapshotRepository};
use crate::{OperationResult, State};
use std::borrow::Cow;
use std::sync::Arc;
use storage_contract::FileId;
use crate::container_storage::{HeldUnbornReader, ObservedContainerReplica};
use storage_contract::file_draft::WorkingContentExpectation;

type Result<T> = std::result::Result<T, Failure>;

/// Full logical reconstruction of this exact borrowed pair. It does not itself
/// hold a root, repair BODYs, install a FILE, or establish a publication barrier.
pub(crate) struct VerifiedDirectoryDraft<'pair> {
    previous: &'pair State,
    next: &'pair State,
    receipt: &'pair OperationResult,
    envelope: Cow<'pair, Envelope>,
    first_sources: Vec<Option<Arc<HeldUnbornReader>>>,
    draft_reads: Vec<Arc<crate::container_storage::ReadDraftDigest>>,
}

impl VerifiedDirectoryDraft<'_> {
    pub(crate) fn require_pair(
        &self, previous: &State, next: &State, result: &OperationResult,
    ) -> Result<()> {
        if !std::ptr::eq(self.previous, previous) || !std::ptr::eq(self.next, next)
            || !std::ptr::eq(self.receipt, result)
        {
            return Err(Failure::Projection);
        }
        self.require_first_sources()
    }

    pub(crate) fn draft_reads(&self)
        -> &[Arc<crate::container_storage::ReadDraftDigest>]
    {
        &self.draft_reads
    }

    pub(crate) fn first_source(&self, file: FileId) -> Option<&Arc<HeldUnbornReader>> {
        self.envelope.directory.files().iter().position(|item| item.target.file == file)
            .and_then(|index| self.first_sources[index].as_ref())
    }

    fn require_first_sources(&self) -> Result<()> {
        for read in &self.draft_reads {
            read.require_source().map_err(Failure::Engine)?;
        }
        for source in self.first_sources.iter().flatten() {
            source.require_source().map_err(Failure::Engine)?;
        }
        Ok(())
    }

    pub(crate) fn working_files(&self) -> impl Iterator<Item = FileId> + '_ {
        self.envelope.directory.files().iter().map(|item| item.target.file)
    }

    pub(super) fn from_live<'a>(
        previous: &'a State, next: &'a State, receipt: &'a OperationResult,
        envelope: &'a Envelope,
        files: &crate::engine::file_draft::WorkingBatchHandoff,
    ) -> Result<VerifiedDirectoryDraft<'a>> {
        // Only the owned proposal calls this after its one complete replay and
        // the actual physical product's same-occurrence checks have succeeded.
        let first_sources = files.request().selected().iter().map(|source| {
            match source.expected.content {
                WorkingContentExpectation::Born { .. } => Ok(None),
                WorkingContentExpectation::UnbornEmpty
                | WorkingContentExpectation::UnbornSaved { .. } => {
                    let full = files.first_material(source.target.file)
                        .ok_or(Failure::Projection)?;
                    full.require_source().map_err(Failure::Engine)?;
                    Ok(Some(Arc::clone(full.reader())))
                }
            }
        }).collect::<Result<Vec<_>>>()?;
        Ok(VerifiedDirectoryDraft {
            previous, next, receipt, envelope: Cow::Borrowed(envelope), first_sources,
            draft_reads: files.draft_reads().iter().flatten().cloned().collect(),
        })
    }
}

pub(crate) fn verify_original<'pair>(
    context: &PlanningContext<'pair>, candidate: &Candidate,
    original: &'pair AdmittedSnapshot, maximum: usize,
) -> Result<VerifiedDirectoryDraft<'pair>> {
    context.require_manual_directory_pool().map_err(Failure::Engine)?;
    let index = context.snapshot_receipts()
        .map_err(|source| Failure::ReceiptIndex(source.into()))?;
    verify_bound_original(
        context.config, context.disk_resources, context.state.as_ref(), context.snapshot, &index,
        candidate, original, maximum,
    )
}

/// The startup source is issued only by the actual prefix cursor. Both live
/// and replay call the same complete original reconstruction below.
fn verify_replay_original<'pair>(
    source: &crate::engine::DirectoryReplayPrefix<'pair>,
    candidate: &Candidate, original: &'pair AdmittedSnapshot, maximum: usize,
) -> Result<VerifiedDirectoryDraft<'pair>> {
    verify_bound_original(
        source.config(), source.resources(), source.previous(), source.snapshot(), source.index(),
        candidate, original, maximum,
    )
}

fn verify_bound_original<'pair>(
    config: &crate::ValidatedConfig, resources: &Arc<crate::disk::DiskResources>,
    previous: &'pair State, snapshot: &Option<String>,
    index: &crate::snapshot_worktree::ReceiptIndex<'_>,
    candidate: &Candidate, original: &'pair AdmittedSnapshot, maximum: usize,
) -> Result<VerifiedDirectoryDraft<'pair>> {
    config.verify_candidate_membership(candidate).map_err(Failure::Engine)?;
    let next = original.for_candidate(candidate).map_err(Failure::Engine)?.state();
    let result = next.operations.get(&candidate.operation_id).ok_or(Failure::Candidate)?;
    if config.generation().is_some()
        || candidate.generation().is_some() || candidate.pool_id != config.pool_id
        || candidate.previous_snapshot != *snapshot
        || candidate.sequence != next.sequence || result.sequence != candidate.sequence
        || result.request_digest != candidate.request_digest
    {
        return Err(Failure::Candidate);
    }
    let description = native::decode(previous, result, maximum, config.max_entries)
        .map_err(Failure::Native)?;
    let request = description.batch_request();
    let checked = crate::business::file_draft::selected::require_all(
        previous, request.selected(), config.max_entries, config.max_file_bytes,
    ).map_err(Failure::Engine)?;
    let draft_reads = checked.iter().map(|selected| {
        let record = selected.record();
        if record.working.incremental().is_none() { return Ok(None); }
        let disk = config.disks.iter().find(|disk| disk.disk_id == record.owner_disk.to_string())
            .ok_or(Failure::Projection)?;
        let source = crate::engine::fresh_layout::containers::published_source(
            config, resources, previous, selected.request().target.file, disk,
        ).map_err(Failure::Engine)?;
        let observed = Arc::new(ObservedContainerReplica::observe(source).map_err(Failure::Engine)?);
        let (base, _) = record.working.draft().ok_or(Failure::Projection)?;
        let version = previous.versions.get(&base.version_id().to_string())
            .ok_or(Failure::Projection)?;
        let reader = crate::container_storage::HeldManualReader::capture(observed, *base, version)
            .map_err(Failure::Engine)?;
        crate::container_storage::ReadDraftDigest::capture(Arc::new(reader),
            config.checksum_chunk_bytes).map(Some).map_err(Failure::Engine)
    }).collect::<Result<Vec<_>>>()?;
    let first_sources = checked.iter().map(|selected| {
        match selected.request().expected.content {
            WorkingContentExpectation::Born { .. } => Ok(None),
            WorkingContentExpectation::UnbornEmpty
            | WorkingContentExpectation::UnbornSaved { .. } => {
                let record = selected.record();
                let disk = config.disks.iter().find(|disk| {
                    disk.disk_id == record.owner_disk.to_string()
                }).ok_or(Failure::Projection)?;
                let source = crate::engine::fresh_layout::containers::published_source(
                    config, resources, previous, selected.request().target.file, disk,
                ).map_err(Failure::Engine)?;
                let observed = Arc::new(ObservedContainerReplica::observe(source)
                    .map_err(Failure::Engine)?);
                observed.open_unborn_working(config.max_file_bytes)
                    .map(|reader| Some(Arc::new(reader))).map_err(Failure::Engine)
            }
        }
    }).collect::<Result<Vec<_>>>()?;
    let first_reads = first_sources.iter().map(|source| {
        source.as_ref().map(|reader| reader.full_digest()).transpose().map_err(Failure::Engine)
    }).collect::<Result<Vec<_>>>()?;
    let replay = super::reconstruction::replay(
        config, previous, index, result, maximum, &first_reads, &draft_reads,
    )?;
    replay.require_candidate(candidate)?;
    if !crate::publication_shape::same_transition(previous, replay.state(), next) {
        return Err(Failure::Projection);
    }
    let envelope = replay.into_envelope();
    drop(first_reads);
    Ok(VerifiedDirectoryDraft {
        previous,
        next,
        receipt: result,
        envelope: Cow::Owned(envelope), first_sources,
        draft_reads: draft_reads.into_iter().flatten().collect(),
    })
}

/// Existing Accepted/Published recovery provides its actual Engine context and
/// original admitted Candidate, not a State manufactured from receipt fields.
/// This only reopens exact decided entities. A missing or foreign node is an
/// error; there is no mkdir, same-name adoption, quarantine, or physical repair.
pub(crate) struct ObservedOriginalDirectory<'original> {
    previous: Arc<State>,
    next: &'original State,
    receipt: &'original OperationResult,
    envelope: Envelope,
    first_sources: Vec<Option<Arc<HeldUnbornReader>>>,
    draft_reads: Vec<Arc<crate::container_storage::ReadDraftDigest>>,
    candidate: Candidate,
    config: crate::ValidatedConfig,
    resources: Arc<crate::disk::DiskResources>,
    snapshot: Option<String>,
    material: ObservedDirectoryMaterial,
}

impl ObservedOriginalDirectory<'_> {
    /// Reborrow the same successful full reconstruction. Owning the previous
    /// State Arc, rather than borrowing Engine, allows its actual completion
    /// method to take &mut Engine while this original snapshot stays pinned.
    pub(crate) fn proof(&self) -> VerifiedDirectoryDraft<'_> {
        VerifiedDirectoryDraft {
            previous: self.previous.as_ref(), next: self.next, receipt: self.receipt,
            envelope: Cow::Borrowed(&self.envelope),
            first_sources: self.first_sources.clone(),
            draft_reads: self.draft_reads.clone(),
        }
    }

    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        if !Arc::ptr_eq(&self.previous, context.state)
            || !self.config.same_admission(context.config)
            || !Arc::ptr_eq(&self.resources, context.disk_resources)
            || self.snapshot != *context.snapshot || context.current_namespaces.is_some()
        {
            return Err(Failure::Projection);
        }
        self.require_material()
    }

    pub(crate) fn require_candidate(&self, candidate: &Candidate) -> Result<()> {
        let expected = storage_contract::object_hash("candidate", &self.candidate)
            .map_err(|source| Failure::Native(native::NativeError::Encoding(source)))?;
        let actual = storage_contract::object_hash("candidate", candidate)
            .map_err(|source| Failure::Native(native::NativeError::Encoding(source)))?;
        if actual != expected { return Err(Failure::Candidate); }
        self.require_material()
    }

    pub(crate) fn require_material(&self) -> Result<()> {
        for read in &self.draft_reads {
            read.require_source().map_err(Failure::Engine)?;
        }
        for source in self.first_sources.iter().flatten() {
            source.require_source().map_err(Failure::Engine)?;
        }
        self.material.require_material()
    }
}

struct ObservedDirectoryMaterial {
    original: HeldSnapshotNode,
    repository: PinnedSnapshotRepository,
    node: Option<HeldSnapshotNode>,
}

impl ObservedDirectoryMaterial {
    fn require_material(&self) -> Result<()> {
        self.repository.require_held_source(&self.original)
            .map_err(|source| Failure::Source(DirectoryDraftError::OriginalRead(source)))?;
        self.repository.require_prepared(match &self.node {
            Some(node) => node, None => &self.original,
        })
            .map_err(|source| Failure::Source(DirectoryDraftError::OriginalRead(source)))
    }
}

pub(crate) struct ObservedReplayDirectory<'pair> {
    proof: VerifiedDirectoryDraft<'pair>,
    material: ObservedDirectoryMaterial,
}
impl ObservedReplayDirectory<'_> {
    pub(crate) fn proof(&self) -> &VerifiedDirectoryDraft<'_> { &self.proof }
    pub(crate) fn require_material(&self) -> Result<()> {
        self.proof.require_first_sources()?;
        self.material.require_material()
    }
}

pub(crate) fn observe_replay_original<'pair>(
    source: &crate::engine::DirectoryReplayPrefix<'pair>, candidate: &Candidate,
    original: &'pair AdmittedSnapshot, maximum: usize,
) -> Result<ObservedReplayDirectory<'pair>> {
    let proof = verify_replay_original(source, candidate, original, maximum)?;
    let request = &proof.envelope.directory.request().source;
    let root = source.namespace_host_root(
        request.host.namespace.repository_id, request.host.namespace.branch_id,
    ).map_err(Failure::Engine)?;
    let material = observe_material(&root, candidate, &proof.envelope)?;
    Ok(ObservedReplayDirectory { proof, material })
}

pub(crate) fn observe_original<'original>(
    context: &PlanningContext<'_>, candidate: &Candidate,
    original_snapshot: &'original AdmittedSnapshot, maximum: usize,
) -> Result<ObservedOriginalDirectory<'original>> {
    let proof = verify_original(context, candidate, original_snapshot, maximum)?;
    let first_sources = proof.first_sources;
    let draft_reads = proof.draft_reads;
    let envelope = proof.envelope.into_owned();
    let source = &envelope.directory.request().source;
    let root = context.namespace_host_root(
        source.host.namespace.repository_id, source.host.namespace.branch_id,
    ).map_err(Failure::Engine)?;
    let material = observe_material(&root, candidate, &envelope)?;
    let next = original_snapshot.for_candidate(candidate).map_err(Failure::Engine)?.state();
    let receipt = next.operations.get(&candidate.operation_id).ok_or(Failure::Candidate)?;
    let observed = ObservedOriginalDirectory {
        previous: Arc::clone(context.state), next, receipt, envelope, first_sources,
        draft_reads,
        candidate: candidate.clone(), config: context.config.clone(),
        resources: Arc::clone(context.disk_resources), snapshot: context.snapshot.clone(),
        material,
    };
    observed.require_material()?;
    Ok(observed)
}

fn observe_material(
    root: &crate::engine::RegisteredNamespaceRoot<'_>, candidate: &Candidate,
    envelope: &Envelope,
) -> Result<ObservedDirectoryMaterial> {
    let source = &envelope.directory.request().source;
    if root.binding().registration != source.host.registration
        || root.binding().root_entity != source.host.root_entity
        || root.binding().roots.common_data != source.host.common_data
        || root.binding().root_id != source.root.root_id
        || root.binding().generation != source.root.expected_root_generation
    {
        return Err(Failure::Projection);
    }
    let host = root.common().map_err(Failure::Engine)?;
    let original = HeldSnapshotNode::reopen_at_host(host, source.binding)
        .map_err(|source| Failure::Source(DirectoryDraftError::OriginalRead(source)))?;
    let repository = original.pin_repository(host)
        .map_err(|source| Failure::Source(DirectoryDraftError::OriginalRead(source)))?;
    let node = if envelope.directory.is_unchanged() {
        if envelope.directory.binding() != original.binding()
            || envelope.directory.material() != original.manifest().reference()
            || envelope.directory.entries() != original.manifest().entries()
        {
            return Err(Failure::Projection);
        }
        None
    } else {
        let node = HeldSnapshotNode::reopen_at_host(host, envelope.directory.binding())
            .map_err(|source| Failure::Source(DirectoryDraftError::OriginalRead(source)))?;
        if node.manifest().operation_id() != candidate.operation_id
            || node.manifest().entries() != envelope.directory.entries()
            || node.manifest().reference() != envelope.directory.material()
        {
            return Err(Failure::Projection);
        }
        Some(node)
    };
    let observed = ObservedDirectoryMaterial { original, repository, node };
    observed.require_material()?;
    Ok(observed)
}
