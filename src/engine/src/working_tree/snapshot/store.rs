use crate::disk::{DirectoryCreateOutcome, DirectoryIoError, DiskRoot};
use crate::persist::layout::snapshot as layout;
use crate::working_tree::records::{file_identity, local_entity};
use crate::working_tree::snapshot::manifest::MAX_MANIFEST_BYTES;
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotFile, SnapshotManifestError, SnapshotReference,
};
use crate::ReadFile;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use std::{collections::BTreeSet, io::{Read, Seek}};
use storage_contract::VersionId;
use storage_contract::working_tree::LocalEntityIdentity;

#[derive(Debug, error_definition::Error)]
pub(crate) enum SnapshotStoreError {
    #[error("directory snapshot manifest admission failed")]
    Manifest(#[source] SnapshotManifestError),
    #[error("directory snapshot directory access failed")]
    Directory(#[source] DirectoryIoError),
    #[error("directory snapshot disk access failed")]
    Disk(#[source] crate::Error),
    #[error("directory snapshot payload access failed")]
    Io(#[source] std::io::Error),
    #[error("directory snapshot payload verification failed")]
    Payload(#[source] platform_io::payload::Error),
    #[error("directory snapshot inventory access failed")]
    Inventory(#[source] platform_io::fs_safety::Error),
    #[error("directory snapshot repository entity differs from its permanent binding")]
    Repository,
    #[error("directory snapshot has missing, extra or conflicting immutable entries")]
    Entries,
    #[error("directory snapshot payload differs from its complete-body commitment")]
    Content,
    #[error("directory snapshot does not contain the requested version")]
    Version,
}

/// Incomplete and uncertain preparation never grants a publication receipt. Its
/// original evidence is retained; a retry cannot adopt an existing node by name.
#[derive(Debug, error_definition::Error)]
pub(crate) enum SnapshotPreparationError {
    #[error("directory snapshot preparation was rejected before creating the node")]
    Rejected(#[source] SnapshotStoreError),
    #[error("directory snapshot preparation is incomplete; preserve its original material")]
    Incomplete {
        reference: SnapshotReference,
        entity: Option<LocalEntityIdentity>,
        #[source]
        source: SnapshotStoreError,
    },
}

/// Local node identity plus the independent immutable material commitment. This
/// record must itself be covered by the original publication decision; decoding
/// a copy from inside `.zxs` is never sufficient authority to call `reopen`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct SnapshotNodeBinding {
    reference: SnapshotReference,
    repository_entity: LocalEntityIdentity,
    node_entity: LocalEntityIdentity,
    files_entity: LocalEntityIdentity,
    worktrees_entity: LocalEntityIdentity,
}
impl SnapshotNodeBinding {
    pub(crate) fn reference(self) -> SnapshotReference {
        self.reference
    }
}

pub(crate) struct HeldSnapshotNode {
    binding: SnapshotNodeBinding,
    manifest: AdmittedSnapshotManifest,
    node: DiskRoot,
    files: DiskRoot,
    worktrees: DiskRoot,
}
impl HeldSnapshotNode {
    /// The caller owns the repository writer fence and has independently
    /// authorized this exact source. This creates only new, unregistered node
    /// material. All output bodies are complete copies, never hard links.
    pub(crate) fn prepare<R: Read>(
        repository: &DiskRoot,
        manifest: AdmittedSnapshotManifest,
        mut source: impl FnMut(&SnapshotFile) -> Result<R, SnapshotStoreError>,
    ) -> Result<Self, SnapshotPreparationError> {
        let _activity = repository.retain_io_activity()
            .map_err(SnapshotStoreError::Disk)
            .map_err(SnapshotPreparationError::Rejected)?;
        let repository_entity = repository.held_identity()
            .map(local_entity)
            .map_err(SnapshotStoreError::Directory)
            .map_err(SnapshotPreparationError::Rejected)?;
        let reference = manifest.reference();
        let name = layout::node(manifest.node_id());
        let (node, node_entity) = match repository.create_private_entry(&name) {
            DirectoryCreateOutcome::Rejected(source) => {
                return Err(SnapshotPreparationError::Rejected(
                    SnapshotStoreError::Directory(source),
                ));
            }
            DirectoryCreateOutcome::Uncertain { identity, source } => {
                return Err(incomplete(reference, identity, SnapshotStoreError::Directory(source)));
            }
            DirectoryCreateOutcome::Created { directory, identity, durability } => {
                durability.map_err(|source| {
                    incomplete(reference, Some(identity), SnapshotStoreError::Directory(source))
                })?;
                (directory, identity)
            }
        };
        Self::prepare_contents(
            repository, repository_entity, node, node_entity, manifest, &mut source,
        ).map_err(|source| incomplete(reference, Some(node_entity), source))
    }

    fn prepare_contents<R: Read>(
        repository: &DiskRoot,
        repository_entity: LocalEntityIdentity,
        node: DiskRoot,
        node_entity: FileIdentity,
        manifest: AdmittedSnapshotManifest,
        source: &mut impl FnMut(&SnapshotFile) -> Result<R, SnapshotStoreError>,
    ) -> Result<Self, SnapshotStoreError> {
        let (files, files_entity) = create_child(&node, &layout::FILES)?;
        let (worktrees, worktrees_entity) = create_child(&node, &layout::WORKTREES)?;
        for file in manifest.files().values() {
            let bytes = read_source(&mut source(file)?, file)?;
            files.write_relative_atomic(&layout::file(file.version_id).as_relative(), &bytes, false)
                .map_err(SnapshotStoreError::Disk)?;
        }
        // Only a complete manifest after every full body can be accepted by recovery.
        node.write_relative_atomic(&layout::MANIFEST.as_relative(), manifest.bytes(), false)
            .map_err(SnapshotStoreError::Disk)?;
        node.sync_private_entry_matching(&layout::FILES, files_entity)
            .map_err(SnapshotStoreError::Directory)?;
        node.sync_private_entry_matching(&layout::WORKTREES, worktrees_entity)
            .map_err(SnapshotStoreError::Directory)?;
        repository.sync_private_entry_matching(&layout::node(manifest.node_id()), node_entity)
            .map_err(SnapshotStoreError::Directory)?;
        let binding = SnapshotNodeBinding {
            reference: manifest.reference(),
            repository_entity,
            node_entity: local_entity(node_entity),
            files_entity: local_entity(files_entity),
            worktrees_entity: local_entity(worktrees_entity),
        };
        let prepared = Self { binding, manifest, node, files, worktrees };
        prepared.verify_inventory()?;
        Ok(prepared)
    }

    /// Recovery is exact, read-only and fail-closed. The binding comes from the
    /// independently admitted decision, never directory scanning or this node.
    pub(crate) fn reopen(
        repository: &DiskRoot,
        expected: SnapshotNodeBinding,
    ) -> Result<Self, SnapshotStoreError> {
        if repository.held_identity().map_err(SnapshotStoreError::Directory)?
            != file_identity(expected.repository_entity)
        {
            return Err(SnapshotStoreError::Repository);
        }
        let node = repository.private_entry_matching(
            &layout::node(expected.reference.node),
            file_identity(expected.node_entity),
        ).map_err(SnapshotStoreError::Directory)?;
        let bytes = node.read_entry_bytes(&layout::MANIFEST, MAX_MANIFEST_BYTES)
            .map_err(SnapshotStoreError::Disk)?;
        let manifest = AdmittedSnapshotManifest::decode(&bytes, expected.reference)
            .map_err(SnapshotStoreError::Manifest)?;
        let files = node.private_entry_matching(
            &layout::FILES, file_identity(expected.files_entity),
        ).map_err(SnapshotStoreError::Directory)?;
        let worktrees = node.private_entry_matching(
            &layout::WORKTREES, file_identity(expected.worktrees_entity),
        ).map_err(SnapshotStoreError::Directory)?;
        let held = Self { binding: expected, manifest, node, files, worktrees };
        held.verify_inventory()?;
        for version in held.manifest.files().keys() {
            // Each source is opened and closed sequentially, not retained as a
            // full-snapshot FD set. Every later open crosses admission again.
            held.open_payload(*version)?;
        }
        repository.private_entry_matching(
            &layout::node(expected.reference.node), file_identity(expected.node_entity),
        ).map_err(SnapshotStoreError::Directory)?;
        Ok(held)
    }

    pub(crate) fn manifest(&self) -> &AdmittedSnapshotManifest {
        &self.manifest
    }
    pub(crate) fn binding(&self) -> SnapshotNodeBinding {
        self.binding
    }
    /// An admitted mutable parent, not authority over any existing instance.
    /// Instance creation and recovery require their own permanent identity binding.
    pub(crate) fn worktrees_directory(&self) -> &DiskRoot {
        &self.worktrees
    }

    /// Return the held, gated original source at offset zero after complete
    /// verification. A consumer must still verify the complete destination copy;
    /// a read FD cannot freeze bytes against a privileged concurrent writer.
    pub(crate) fn open_payload(&self, version: VersionId) -> Result<ReadFile, SnapshotStoreError> {
        let file = self.manifest.files().get(&version).ok_or(SnapshotStoreError::Version)?;
        let mut source = self.files.open_read_entry(&layout::file(version))
            .map_err(SnapshotStoreError::Disk)?;
        source.verify_full(platform_io::payload::Expectation {
            bytes: file.size,
            blake3: Some(file.content_hash.digest()),
        }).map_err(SnapshotStoreError::Payload)?;
        source.rewind().map_err(SnapshotStoreError::Io)?;
        Ok(source)
    }

    fn verify_inventory(&self) -> Result<(), SnapshotStoreError> {
        let mut required = BTreeSet::from([
            layout::MANIFEST.clone(), layout::FILES.clone(), layout::WORKTREES.clone(),
        ]);
        for (index, name) in self.node.root_entries()
            .map_err(SnapshotStoreError::Directory)?.enumerate()
        {
            if index == 3 {
                return Err(SnapshotStoreError::Entries);
            }
            let name = name.map_err(SnapshotStoreError::Inventory)?;
            // Worktree contents have independent authority and are never scanned
            // or used to repair an immutable source snapshot.
            if !required.remove(&name) {
                return Err(SnapshotStoreError::Entries);
            }
        }
        if !required.is_empty() {
            return Err(SnapshotStoreError::Entries);
        }
        let mut files: BTreeSet<EntryName> = self.manifest.files().keys()
            .map(|version| layout::file(*version)).collect();
        for name in self.files.root_entries().map_err(SnapshotStoreError::Directory)? {
            if !files.remove(&name.map_err(SnapshotStoreError::Inventory)?) {
                return Err(SnapshotStoreError::Entries);
            }
        }
        if !files.is_empty() {
            return Err(SnapshotStoreError::Entries);
        }
        Ok(())
    }
}

fn create_child(
    node: &DiskRoot,
    name: &EntryName,
) -> Result<(DiskRoot, FileIdentity), SnapshotStoreError> {
    match node.create_private_entry(name) {
        DirectoryCreateOutcome::Rejected(source)
        | DirectoryCreateOutcome::Uncertain { source, .. } => {
            Err(SnapshotStoreError::Directory(source))
        }
        DirectoryCreateOutcome::Created { directory, identity, durability } => {
            durability.map_err(SnapshotStoreError::Directory)?;
            Ok((directory, identity))
        }
    }
}

fn read_source(input: &mut impl Read, file: &SnapshotFile) -> Result<Vec<u8>, SnapshotStoreError> {
    // Manifest admission already bounds every file and the aggregate to 64 MiB.
    // A single extra byte detects trailing source data without unbounded reads.
    let mut bytes = Vec::new();
    input.take(file.size + 1).read_to_end(&mut bytes).map_err(SnapshotStoreError::Io)?;
    if bytes.len() as u64 != file.size || blake3::hash(&bytes) != file.content_hash.digest() {
        return Err(SnapshotStoreError::Content);
    }
    Ok(bytes)
}

fn incomplete(
    reference: SnapshotReference,
    entity: Option<FileIdentity>,
    source: SnapshotStoreError,
) -> SnapshotPreparationError {
    SnapshotPreparationError::Incomplete { reference, entity: entity.map(local_entity), source }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/working_tree/snapshot/store_tests.rs"]
mod tests;
