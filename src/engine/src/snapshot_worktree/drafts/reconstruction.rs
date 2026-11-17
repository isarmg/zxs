//! Original complete logical reconstruction. A visible recovered receipt is
//! never an input, and this module by itself supplies no physical root.
use super::scope::{self, DirectoryOccurrence};
use super::publication::DirectoryPublicationError as Failure;
use super::{DirectoryDraftError, DirectoryPhysicalError};
use crate::business::file_draft::batch;
use crate::persist::directory_draft_commit::native::{self, Envelope};
use crate::snapshot_worktree::registry::ReceiptIndex;
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotEntry, SnapshotEntryContent, SnapshotFileSource,
};
use crate::{OperationResult, State, ValidatedConfig};
use storage_contract::{DiskId, Event, Sequence};

type Result<T> = std::result::Result<T, Failure>;

pub(super) struct ReconstructedDirectory {
    next: State,
    files: batch::WorkingBatchProjection,
    envelope: Envelope,
}

impl ReconstructedDirectory {
    pub(super) fn require_file_projection(&self, previous: &State, actual: &State) -> Result<()> {
        if !crate::publication_shape::same_transition(previous, self.files.state(), actual) {
            return Err(Failure::Projection);
        }
        Ok(())
    }

    pub(super) fn into_state(self) -> State { self.next }

    pub(super) fn into_envelope(self) -> Envelope { self.envelope }

    pub(super) fn state(&self) -> &State { &self.next }

    pub(super) fn require_candidate(&self, candidate: &crate::journal::Candidate) -> Result<()> {
        self.files.require_versions(&candidate.versions).map_err(Failure::Engine)
    }
}

fn source_manifest(
    config: &ValidatedConfig, previous: &State, index: &ReceiptIndex<'_>, envelope: &Envelope,
) -> Result<AdmittedSnapshotManifest> {
    let request = envelope.directory.request();
    let source = &request.source;
    let published = index.draft_head(source.handle)
        .map_err(|error| Failure::Source(DirectoryDraftError::Registration(error)))?;
    if published.binding() != source.binding || published.host() != &source.host
        || published.initialization().source.root.is_some()
    {
        return Err(Failure::Projection);
    }
    let host = published.host();
    let owner = host.registration.disk_id.parse::<DiskId>()
        .map_err(|error| Failure::Source(DirectoryDraftError::Identity(error)))?;
    let root = config.storage_roots.get(&owner).ok_or(Failure::Projection)?;
    let branch = previous.branches.get(&host.namespace.branch_id.to_string())
        .ok_or(Failure::Projection)?;
    // These are logical registration checks only. Before reading or preparing
    // any material, the actual context must use namespace_host_root/common().
    if root.registration != host.registration || root.root_entity != host.root_entity
        || root.roots.common_data != host.common_data
        || root.root_id != source.root.root_id
        || root.generation != source.root.expected_root_generation
        || root.roots.common_data.directory_id != source.root.directory_id
        || branch.generation != source.namespace_generation.get()
    {
        return Err(Failure::Projection);
    }
    let selected = envelope.batch_request();
    scope::validate_selection(selected.selected(), config.max_entries)
        .map_err(Failure::Source)?;
    for target in selected.selected() {
        scope::require_owner(previous, host, owner, &target.target).map_err(Failure::Source)?;
    }
    reconstruct_indexed_manifest(published.manifest(), &envelope.directory, config.max_entries)
}

/// Relationship/manifest validation over an already admitted receipt stream.
/// It does not reconstruct FILE state or issue the full original-pair proof.
pub(in crate::snapshot_worktree) fn reconstruct_indexed_manifest(
    original: &AdmittedSnapshotManifest,
    directory: &crate::persist::directory_draft_commit::DirectoryResult,
    maximum_entries: usize,
) -> Result<AdmittedSnapshotManifest> {
    directory.validate(maximum_entries)
        .map_err(|source| Failure::Native(native::NativeError::Description(source)))?;
    let request = directory.request();
    let selected = request.selected.iter().map(|selected| {
        storage_contract::file_draft::WorkingSelection {
            target: selected.target.clone(), expected: selected.expectation(),
        }
    }).collect::<Vec<_>>();
    scope::validate_selection(&selected, maximum_entries).map_err(Failure::Source)?;
    scope::require_original(original.entries(), &selected).map_err(Failure::Source)?;
    let projection = scope::merge_selection(original.entries(), &selected)
        .map_err(Failure::Source)?;
    let entries: Vec<_> = projection.iter().map(|entry| match *entry {
        DirectoryOccurrence::Original(index) => original.entries()[index].clone(),
        DirectoryOccurrence::Selected(index) => {
            let item = &directory.files()[index];
            let (branch, node) = item.outcome.source();
            SnapshotEntry {
                path: item.target.path.clone(),
                content: SnapshotEntryContent::File {
                    material: item.outcome.material(),
                    source: SnapshotFileSource::Container { branch, node },
                },
            }
        }
    }).collect();
    let unchanged = entries.as_slice() == original.entries();
    if unchanged != directory.is_unchanged() || entries.as_slice() != directory.entries() {
        return Err(Failure::Projection);
    }
    let manifest = if unchanged {
        Ok(original.clone())
    } else {
        AdmittedSnapshotManifest::prepare(
            request.source.handle.repository_id, directory.material().node,
            request.operation, entries,
        )
    }.map_err(|source| Failure::Physical(DirectoryPhysicalError::Manifest(source)))?;
    if manifest.reference() != directory.material() {
        return Err(Failure::Projection);
    }
    Ok(manifest)
}

pub(super) fn replay(
    config: &ValidatedConfig, previous: &State, index: &ReceiptIndex<'_>,
    result: &OperationResult, maximum: usize,
    first_reads: &[Option<crate::container_storage::ReadWorkingDigest<'_>>],
) -> Result<ReconstructedDirectory> {
    index.require_source(previous)
        .map_err(|error| Failure::Source(DirectoryDraftError::Registration(error)))?;
    if config.generation().is_some() || previous.operations.contains_key(&result.operation_id)
        || result.sequence <= previous.sequence
    {
        return Err(Failure::Projection);
    }
    crate::directory_conversion::require_import_not_stopped(previous, result.operation_id)
        .map_err(Failure::Engine)?;
    let envelope = native::decode(previous, result, maximum, config.max_entries)
        .map_err(Failure::Native)?;
    if envelope.membership_epoch != config.membership_epoch() {
        return Err(Failure::Projection);
    }
    let request = envelope.batch_request();
    let files = batch::replay(
        previous, config, &request, Sequence::new(result.sequence), &envelope.trace(), first_reads,
    ).map_err(Failure::Engine)?;
    let described = super::batch::describe_items(files.items()).map_err(Failure::Working)?;
    if described.as_slice() != envelope.directory.files()
        || files.items().iter().map(|item| item.working_after())
            .ne(envelope.working_after.iter())
    {
        return Err(Failure::Projection);
    }
    let manifest = source_manifest(config, previous, index, &envelope)?;
    if manifest.entries() != envelope.directory.entries()
        || manifest.reference() != envelope.directory.material()
    {
        return Err(Failure::Projection);
    }
    let mut next = files.state().clone();
    crate::file_deletion::freeze::verify(previous, &next).map_err(Failure::Engine)?;
    crate::business::file_tags::preserve_existing(previous, &next).map_err(Failure::Engine)?;
    next.operations.insert(result.operation_id, result.clone());
    next.operation_principals.insert(result.operation_id, request.principal().as_str().into());
    next.events.push(Event {
        sequence: result.sequence,
        operation_id: result.operation_id,
        kind: result.kind.clone(),
        object_id: result.operation_id.to_string(),
    });
    crate::node_retirement_engine::verify(previous, &next).map_err(Failure::Engine)?;
    crate::file_deletion::publication::verify(previous, &next, None).map_err(Failure::Engine)?;
    Ok(ReconstructedDirectory { next, files, envelope })
}
