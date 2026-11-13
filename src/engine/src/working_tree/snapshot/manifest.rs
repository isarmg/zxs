use crate::persist::layout;
use data_serialization::{Deserialize, Deserializer, Serialize};
use name_policy::PortablePath;
use std::collections::BTreeMap;
use storage_contract::working_tree::{
    MaterialCommitment, SnapshotNodeId, SnapshotRepositoryId,
};
use storage_contract::{
    BranchId, ContentHash, FileId, FileNodeId, OperationId, VersionId, VersionStorageId,
};

pub(crate) const MAX_ENTRIES: usize = 2048;
pub(crate) const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 1024 * 1024;
const MAX_VERSIONS: usize = 512;
pub(crate) const MAX_LOGICAL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, error_definition::Error)]
pub(crate) enum SnapshotManifestError {
    #[error("directory snapshot manifest exceeds its admission budget")]
    Budget,
    #[error("decoding directory snapshot manifest")]
    Decode(#[source] storage_contract::Error),
    #[error("encoding directory snapshot manifest")]
    Encode(#[source] storage_contract::Error),
    #[error("directory snapshot manifest differs from its committed canonical bytes")]
    Commitment,
    #[error("directory snapshot manifest is not bound to the selected repository and node")]
    Identity,
    #[error("directory snapshot paths are not in strict lookup-key order")]
    Order,
    #[error("directory snapshot path has an absent, non-directory or differently spelled parent")]
    Parent,
    #[error("directory snapshot contains protected management material")]
    Management,
    #[error("directory snapshot version has conflicting immutable file identities")]
    Version,
    #[error("directory snapshot selected version node differs from its file material")]
    SourceVersion,
    #[error("directory snapshot payload mappings do not exactly close the file inventory")]
    Payloads,
    #[error("directory snapshot short storage identities collide")]
    StorageIdentity,
    #[error("directory snapshot stored filename exceeds the portable component budget")]
    Filename,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serialization(rename_all = "snake_case")]
enum ManifestFormat {
    FullDirectorySnapshot,
}
impl<'de> Deserialize<'de> for ManifestFormat {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "full_directory_snapshot" => Ok(Self::FullDirectorySnapshot),
            _ => Err(data_serialization::de::Error::custom(
                "unsupported directory snapshot format",
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct SnapshotFile {
    pub(crate) file_id: FileId,
    pub(crate) version_id: VersionId,
    #[serialization(with = "storage_contract::u64_string")]
    pub(crate) size: u64,
    pub(crate) content_hash: ContentHash,
}

/// Frozen per-occurrence provenance, not proof of current physical presence.
/// Different aliases of the same version can select different FILE branches.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum SnapshotFileSource {
    Container { branch: BranchId, node: FileNodeId },
    /// A previously associated container is authoritatively absent. Unknown
    /// imports instead receive a new independent container after write sealing;
    /// matching bytes, names or versions can never associate their original.
    SnapshotOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum SnapshotEntryContent {
    Directory {},
    File { material: SnapshotFile, source: SnapshotFileSource },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct SnapshotEntry {
    pub(crate) path: PortablePath,
    pub(crate) content: SnapshotEntryContent,
}

/// The short leaf is only an address. Full version identity and source spelling
/// remain committed here; the first canonical alias supplies the physical suffix.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct SnapshotPayload {
    pub(crate) version_id: VersionId,
    pub(crate) storage_id: VersionStorageId,
    pub(crate) source_path: PortablePath,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct ManifestRecord {
    format: ManifestFormat,
    repository_id: SnapshotRepositoryId,
    node_id: SnapshotNodeId,
    operation_id: OperationId,
    entries: Vec<SnapshotEntry>,
    payloads: Vec<SnapshotPayload>,
}

/// Expected immutable material, not proof that a branch or repository is published.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct SnapshotReference {
    pub(crate) repository: SnapshotRepositoryId,
    pub(crate) node: SnapshotNodeId,
    pub(crate) manifest: MaterialCommitment,
}

/// Syntax, complete parent closure, bounded inventory and canonical commitment.
/// Its construction does not prove source authorization or file-body availability.
pub(crate) struct AdmittedSnapshotManifest {
    record: ManifestRecord,
    reference: SnapshotReference,
    files: BTreeMap<VersionId, SnapshotFile>,
    payloads: BTreeMap<VersionId, SnapshotPayload>,
    bytes: Vec<u8>,
}
impl AdmittedSnapshotManifest {
    pub(crate) fn prepare(
        repository_id: SnapshotRepositoryId,
        node_id: SnapshotNodeId,
        operation_id: OperationId,
        entries: Vec<SnapshotEntry>,
    ) -> Result<Self, SnapshotManifestError> {
        if entries.len() > MAX_ENTRIES {
            return Err(SnapshotManifestError::Budget);
        }
        Self::admit(ManifestRecord {
            format: ManifestFormat::FullDirectorySnapshot,
            repository_id,
            node_id,
            operation_id,
            payloads: default_payloads(&entries),
            entries,
        })
    }

    pub(crate) fn decode(
        bytes: &[u8],
        expected: SnapshotReference,
    ) -> Result<Self, SnapshotManifestError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(SnapshotManifestError::Budget);
        }
        if commitment(bytes) != expected.manifest {
            return Err(SnapshotManifestError::Commitment);
        }
        let record = storage_contract::decode_strict(bytes)
            .map_err(SnapshotManifestError::Decode)?;
        let admitted = Self::admit(record)?;
        if admitted.reference.repository != expected.repository
            || admitted.reference.node != expected.node
        {
            return Err(SnapshotManifestError::Identity);
        }
        if admitted.bytes != bytes {
            return Err(SnapshotManifestError::Commitment);
        }
        Ok(admitted)
    }

    fn admit(record: ManifestRecord) -> Result<Self, SnapshotManifestError> {
        admit_sources(&record.entries)?;
        let files = admit_entries(&record.entries)?;
        let payloads = admit_payloads(&record, &files)?;
        let bytes = storage_contract::canonical_json(&record)
            .map_err(SnapshotManifestError::Encode)?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(SnapshotManifestError::Budget);
        }
        let reference = SnapshotReference {
            repository: record.repository_id,
            node: record.node_id,
            manifest: commitment(&bytes),
        };
        Ok(Self { record, reference, files, payloads, bytes })
    }

    pub(crate) fn repository_id(&self) -> SnapshotRepositoryId {
        self.reference.repository
    }
    pub(crate) fn node_id(&self) -> SnapshotNodeId {
        self.reference.node
    }
    pub(crate) fn commitment(&self) -> MaterialCommitment {
        self.reference.manifest
    }
    pub(crate) fn reference(&self) -> SnapshotReference {
        self.reference
    }
    pub(crate) fn entries(&self) -> &[SnapshotEntry] {
        &self.record.entries
    }
    pub(crate) fn files(&self) -> &BTreeMap<VersionId, SnapshotFile> {
        &self.files
    }
    pub(crate) fn payloads(&self) -> &BTreeMap<VersionId, SnapshotPayload> {
        &self.payloads
    }
    /// Preflight a complete replacement view without inventing future FILE
    /// branch/node results. Provenance is checked separately at final admission.
    pub(crate) fn validate_material_replacements(
        &self,
        replacements: &[(PortablePath, SnapshotFile)],
    ) -> Result<(), SnapshotManifestError> {
        let mut record = self.record.clone();
        for (path, replacement) in replacements {
            let index = record.entries.binary_search_by(|entry| entry.path.key().cmp(path.key()))
                .map_err(|_| SnapshotManifestError::Identity)?;
            let entry = &mut record.entries[index];
            let SnapshotEntryContent::File { material, .. } = &mut entry.content else {
                return Err(SnapshotManifestError::Identity);
            };
            if entry.path != *path || material.file_id != replacement.file_id {
                return Err(SnapshotManifestError::Identity);
            }
            *material = *replacement;
        }
        self.validate_material_entries(record.entries)
    }

    /// Admit a complete logical worktree view using the snapshot's structural
    /// limits. FILE source nodes can still describe the pre-edit baseline, so
    /// this does not admit publication provenance, disk ownership or body I/O.
    pub(crate) fn validate_material_entries(
        &self,
        entries: Vec<SnapshotEntry>,
    ) -> Result<(), SnapshotManifestError> {
        let files = admit_entries(&entries)?;
        let record = ManifestRecord {
            format: self.record.format,
            repository_id: self.record.repository_id,
            node_id: self.record.node_id,
            operation_id: self.record.operation_id,
            payloads: default_payloads(&entries),
            entries,
        };
        admit_payloads(&record, &files)?;
        if storage_contract::canonical_json(&record).map_err(SnapshotManifestError::Encode)?.len()
            > MAX_MANIFEST_BYTES
        {
            return Err(SnapshotManifestError::Budget);
        }
        Ok(())
    }
    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

fn commitment(bytes: &[u8]) -> MaterialCommitment {
    let mut hasher = blake3::Hasher::new_derive_key("ZXS full directory snapshot manifest");
    hasher.update(bytes);
    MaterialCommitment::from_digest(hasher.finalize())
}

fn default_payloads(entries: &[SnapshotEntry]) -> Vec<SnapshotPayload> {
    let mut payloads = BTreeMap::new();
    for entry in entries {
        if let SnapshotEntryContent::File { material: file, .. } = entry.content {
            payloads.entry(file.version_id).or_insert_with(|| SnapshotPayload {
                version_id: file.version_id,
                storage_id: VersionStorageId::from_version(file.version_id),
                source_path: entry.path.clone(),
            });
        }
    }
    payloads.into_values().collect()
}
fn admit_sources(entries: &[SnapshotEntry]) -> Result<(), SnapshotManifestError> {
    for entry in entries {
        if let SnapshotEntryContent::File {
            material,
            source: SnapshotFileSource::Container { node: FileNodeId::Version(version), .. },
        } = entry.content {
            if version != material.version_id {
                return Err(SnapshotManifestError::SourceVersion);
            }
        }
    }
    Ok(())
}

fn admit_entries(
    entries: &[SnapshotEntry],
) -> Result<BTreeMap<VersionId, SnapshotFile>, SnapshotManifestError> {
    if entries.len() > MAX_ENTRIES {
        return Err(SnapshotManifestError::Budget);
    }
    let management = name_policy::lookup_key(
        layout::SNAPSHOT_REPOSITORY.as_os_str().to_str()
            .expect("invariant: snapshot repository layout name is ASCII"),
    );
    let mut path_bytes = 0usize;
    let mut logical_bytes = 0u64;
    let mut files = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        path_bytes = path_bytes.checked_add(entry.path.as_str().len())
            .filter(|total| *total <= MAX_PATH_BYTES)
            .ok_or(SnapshotManifestError::Budget)?;
        if index > 0 && entries[index - 1].path.key() >= entry.path.key() {
            return Err(SnapshotManifestError::Order);
        }
        if entry.path.key().split('/').any(|component| component == management) {
            return Err(SnapshotManifestError::Management);
        }
        if let Some(parent) = entry.path.parent() {
            let position = entries[..index].binary_search_by(|entry| {
                entry.path.key().cmp(parent.key())
            }).map_err(|_| SnapshotManifestError::Parent)?;
            let ancestor = &entries[position];
            if ancestor.path.as_str() != parent.as_str()
                || !matches!(ancestor.content, SnapshotEntryContent::Directory {})
            {
                return Err(SnapshotManifestError::Parent);
            }
        }
        if let SnapshotEntryContent::File { material: file, .. } = entry.content {
            logical_bytes = logical_bytes.checked_add(file.size)
                .filter(|total| *total <= MAX_LOGICAL_BYTES)
                .ok_or(SnapshotManifestError::Budget)?;
            if files.insert(file.version_id, file).is_some_and(|previous| previous != file) {
                return Err(SnapshotManifestError::Version);
            }
            if files.len() > MAX_VERSIONS {
                return Err(SnapshotManifestError::Budget);
            }
        }
    }
    Ok(files)
}

fn admit_payloads(
    record: &ManifestRecord,
    files: &BTreeMap<VersionId, SnapshotFile>,
) -> Result<BTreeMap<VersionId, SnapshotPayload>, SnapshotManifestError> {
    if record.payloads.len() != files.len() {
        return Err(SnapshotManifestError::Payloads);
    }
    let mut origins = BTreeMap::new();
    for entry in &record.entries {
        if let SnapshotEntryContent::File { material: file, .. } = entry.content {
            origins.entry(file.version_id).or_insert(&entry.path);
        }
    }
    let mut identities = std::collections::BTreeSet::new();
    let mut payloads = BTreeMap::new();
    for (payload, version) in record.payloads.iter().zip(files.keys()) {
        if payload.version_id != *version
            || origins.get(version).copied() != Some(&payload.source_path)
        {
            return Err(SnapshotManifestError::Payloads);
        }
        if !identities.insert(payload.storage_id) {
            return Err(SnapshotManifestError::StorageIdentity);
        }
        // The physical prefix consumes sixteen bytes in addition to the user
        // name. Reject before any I/O rather than silently truncate a spelling.
        if payload.source_path.file_name().len() > 255 - 16 {
            return Err(SnapshotManifestError::Filename);
        }
        payloads.insert(*version, payload.clone());
    }
    Ok(payloads)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/working_tree/snapshot/manifest_tests.rs"]
mod tests;
