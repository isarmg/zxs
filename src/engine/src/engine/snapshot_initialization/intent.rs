//! Immutable first-write plan. It carries exact sources, never a disk scan recipe.
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotEntry, SnapshotEntryContent, SnapshotFileSource,
    SnapshotManifestError, SnapshotReference,
};
use data_serialization::{Deserialize, Serialize};
use storage_contract::working_tree::{
    BindingLifecycle, DirectoryBinding, DirectoryId, DirectoryTargetPath, DirectoryTargetRequest,
    DiskRegistrationIdentity, LocalEntityIdentity, MaterialCommitment, RootBinding,
    SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId,
};
use storage_contract::{BranchId, CommitId, OperationId, Principal, RepositoryId, Sequence};

pub(super) const MAX_INTENT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "directory-snapshot-initialization-intent-v2")]
    FreshDirectorySnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Source {
    pub(super) repository: RepositoryId,
    pub(super) branch: BranchId,
    pub(super) head: CommitId,
    pub(super) scope: Option<name_policy::PortablePath>,
    pub(super) publication: MaterialCommitment,
    pub(super) sequence: Sequence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct SnapshotInitIntent {
    format: Format,
    pub(super) operation: OperationId,
    pub(super) principal: Principal,
    pub(super) source: Source,
    pub(super) target: DirectoryTargetRequest,
    pub(super) root: RootBinding,
    pub(super) host_entity: LocalEntityIdentity,
    pub(super) host_directory_id: DirectoryId,
    /// Exact root-to-host chain, not a recipe for assigning identities on recovery.
    pub(super) directories: Vec<DirectoryBinding>,
    pub(super) metadata_members: Vec<DiskRegistrationIdentity>,
    pub(super) repository: SnapshotRepositoryId,
    pub(super) branch: SnapshotBranchId,
    pub(super) node: SnapshotNodeId,
    pub(super) entries: Vec<SnapshotEntry>,
    pub(super) material: SnapshotReference,
}

#[derive(Debug, error_definition::Error)]
pub(super) enum IntentError {
    #[error("directory initialization intent exceeds its byte budget")]
    Budget,
    #[error("directory initialization intent is not canonical")]
    Canonical,
    #[error("directory initialization intent does not bind its exact target")]
    Target,
    #[error("directory initialization intent does not bind its exact material")]
    Material,
    #[error("first directory initialization requires explicit FILE provenance")]
    Source,
    #[error("decoding directory initialization intent")]
    Decode(#[source] storage_contract::Error),
    #[error("encoding directory initialization intent")]
    Encode(#[source] storage_contract::Error),
    #[error("directory initialization snapshot inventory is invalid")]
    Manifest(#[source] SnapshotManifestError),
}

/// Canonical syntax and internal closure, not disk authorization or a decision.
/// Construction belongs to the initializer after it captures the live source.
pub(super) struct AdmittedIntent {
    record: SnapshotInitIntent,
    bytes: Vec<u8>,
}

impl AdmittedIntent {
    pub(super) fn prepare(
        operation: OperationId,
        principal: Principal,
        source: Source,
        target: DirectoryTargetRequest,
        root: RootBinding,
        host_entity: LocalEntityIdentity,
        host_directory_id: DirectoryId,
        directories: Vec<DirectoryBinding>,
        metadata_members: Vec<DiskRegistrationIdentity>,
        repository: SnapshotRepositoryId,
        branch: SnapshotBranchId,
        node: SnapshotNodeId,
        entries: Vec<SnapshotEntry>,
    ) -> Result<Self, IntentError> {
        let manifest = AdmittedSnapshotManifest::prepare(
            repository, node, operation, entries.clone(),
        ).map_err(IntentError::Manifest)?;
        Self::admit(SnapshotInitIntent {
            format: Format::FreshDirectorySnapshot,
            operation,
            principal,
            source,
            target,
            root,
            host_entity,
            host_directory_id,
            directories,
            metadata_members,
            repository,
            branch,
            node,
            entries,
            material: manifest.reference(),
        })
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self, IntentError> {
        if bytes.len() > MAX_INTENT_BYTES {
            return Err(IntentError::Budget);
        }
        let record = storage_contract::decode_strict(bytes).map_err(IntentError::Decode)?;
        let admitted = Self::admit(record)?;
        if admitted.bytes != bytes {
            return Err(IntentError::Canonical);
        }
        Ok(admitted)
    }

    fn admit(record: SnapshotInitIntent) -> Result<Self, IntentError> {
        if record.target.operation_id != record.operation
            || record.target.root_id != record.root.root_id
            || record.target.expected_root_generation != record.root.generation
            || record.host_entity.device() != record.root.root_entity.device()
            || !record.root.source_chain.is_empty()
            || record.metadata_members.is_empty()
            || record.metadata_members.len() > 256
            || record.metadata_members.windows(2).any(|pair| pair[0].disk_id >= pair[1].disk_id)
            || record.metadata_members.iter().any(|member| {
                member.pool_id != record.root.registration.pool_id
                    || member.binding_generation == 0
            })
        {
            return Err(IntentError::Target);
        }
        require_directory_chain(&record)?;
        // A missing/offline container is not permission to invent SnapshotOnly.
        // That variant is for a later authoritatively proven absence workflow.
        if record.entries.iter().any(|entry| matches!(entry.content,
            SnapshotEntryContent::File { source: SnapshotFileSource::SnapshotOnly, .. }
        )) {
            return Err(IntentError::Source);
        }
        let manifest = AdmittedSnapshotManifest::prepare(
            record.repository, record.node, record.operation, record.entries.clone(),
        ).map_err(IntentError::Manifest)?;
        if manifest.reference() != record.material {
            return Err(IntentError::Material);
        }
        let bytes = storage_contract::canonical_json(&record).map_err(IntentError::Encode)?;
        if bytes.len() > MAX_INTENT_BYTES {
            return Err(IntentError::Budget);
        }
        Ok(Self { record, bytes })
    }

    pub(super) fn record(&self) -> &SnapshotInitIntent {
        &self.record
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(super) fn manifest(&self) -> Result<AdmittedSnapshotManifest, IntentError> {
        AdmittedSnapshotManifest::prepare(
            self.record.repository,
            self.record.node,
            self.record.operation,
            self.record.entries.clone(),
        ).map_err(IntentError::Manifest)
    }
}

/// This only validates the original persisted chain. Physical admission still
/// requires every held ancestor and the independent permanent root decision.
fn require_directory_chain(record: &SnapshotInitIntent) -> Result<(), IntentError> {
    let components = match &record.target.path {
        DirectoryTargetPath::Root => &[][..],
        DirectoryTargetPath::Relative(path) => path.components(),
    };
    if components.len() != record.directories.len() {
        return Err(IntentError::Target);
    }
    let mut parent = record.root.roots.common_data.directory_id;
    let mut entity = record.root.roots.common_data.entity;
    let mut ids = std::collections::BTreeSet::from([
        parent,
        record.root.roots.deleted_data.directory_id,
        record.root.roots.zxs_data.directory_id,
    ]);
    let mut entities = std::collections::BTreeSet::from([
        record.root.root_entity,
        entity,
        record.root.roots.deleted_data.entity,
        record.root.roots.zxs_data.entity,
    ]);
    for (name, directory) in components.iter().zip(&record.directories) {
        if directory.root_id != record.root.root_id
            || directory.parent_directory_id != parent
            || directory.name != *name
            || directory.lifecycle != BindingLifecycle::Current
            || directory.entity.device() != record.root.root_entity.device()
            || !ids.insert(directory.directory_id)
            || !entities.insert(directory.entity)
        {
            return Err(IntentError::Target);
        }
        parent = directory.directory_id;
        entity = directory.entity;
    }
    if parent != record.host_directory_id || entity != record.host_entity {
        return Err(IntentError::Target);
    }
    Ok(())
}
