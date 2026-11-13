//! Immutable first-write plan. It carries exact sources, never a disk scan recipe.
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotEntry, SnapshotEntryContent, SnapshotFileSource,
    SnapshotManifestError, SnapshotReference,
};
use data_serialization::{Deserialize, Serialize};
use storage_contract::working_tree::{
    DirectoryTargetRequest, DiskRegistrationIdentity, LocalEntityIdentity, MaterialCommitment, RootBinding,
    SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId,
};
use storage_contract::{BranchId, CommitId, OperationId, Principal, RepositoryId, Sequence};

pub(super) const MAX_INTENT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "directory-snapshot-initialization-intent-v1")]
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
