//! Retain a snapshot's exact physical parent for a later budgeted preparation.
//! Publication authority stays with the caller's independently admitted source;
//! this read product exposes no directory handle or mutation operation.
use crate::disk::DiskRoot;
use crate::persist::layout::{SNAPSHOT_REPOSITORY, snapshot as layout};
use crate::working_tree::records::file_identity;
use crate::working_tree::snapshot::store::{
    HeldSnapshotNode, SnapshotNodeBinding, SnapshotStoreError,
};

pub(crate) struct PinnedSnapshotRepository {
    source: SnapshotNodeBinding,
    host: DiskRoot,
    repository: DiskRoot,
}

impl HeldSnapshotNode {
    /// The namespace consumer supplies its actually admitted held host. The
    /// repository and source node must still match its independent binding.
    pub(crate) fn pin_repository(
        &self,
        host: &DiskRoot,
    ) -> Result<PinnedSnapshotRepository, SnapshotStoreError> {
        let repository = host.private_entry_matching(
            &SNAPSHOT_REPOSITORY, file_identity(self.binding.repository_entity),
        ).map_err(SnapshotStoreError::Directory)?;
        // Reopen "." below the already held host, with its exact identity. This
        // is not an ambient pathname open or authority over a different host.
        let identity = host.held_identity().map_err(SnapshotStoreError::Directory)?;
        let retained_host = host.private_child_matching(host.path(), identity)
            .map_err(SnapshotStoreError::Disk)?;
        let pinned = PinnedSnapshotRepository {
            source: self.binding, host: retained_host, repository,
        };
        pinned.require_source(self, host)?;
        Ok(pinned)
    }
}

impl PinnedSnapshotRepository {
    fn require_host(&self) -> Result<(), SnapshotStoreError> {
        self.host.private_entry_matching(&SNAPSHOT_REPOSITORY, file_identity(self.source.repository_entity))
            .map(|_| ()).map_err(SnapshotStoreError::Directory)
    }
    pub(super) fn batch_repository(&self, source: &HeldSnapshotNode) -> Result<&DiskRoot, SnapshotStoreError> {
        self.require_host()?;
        self.require_held_source(source)?;
        Ok(&self.repository)
    }

    /// Recheck the named relationship as well as the already-held descriptor.
    /// Equal bytes at a replacement path cannot supply a preparation parent.
    pub(crate) fn require_source(
        &self,
        node: &HeldSnapshotNode,
        host: &DiskRoot,
    ) -> Result<(), SnapshotStoreError> {
        if node.binding != self.source
            || host.held_identity().map_err(SnapshotStoreError::Directory)?
                != self.host.held_identity().map_err(SnapshotStoreError::Directory)?
            || self.repository.held_identity().map_err(SnapshotStoreError::Directory)?
                != file_identity(self.source.repository_entity)
            || node.node.held_identity().map_err(SnapshotStoreError::Directory)?
                != file_identity(self.source.node_entity)
        {
            return Err(SnapshotStoreError::Repository);
        }
        host.private_entry_matching(
            &SNAPSHOT_REPOSITORY, file_identity(self.source.repository_entity),
        ).map_err(SnapshotStoreError::Directory)?;
        self.repository.private_entry_matching(
            &layout::node(self.source.reference.node), file_identity(self.source.node_entity),
        ).map_err(SnapshotStoreError::Directory)?;
        Ok(())
    }

    pub(crate) fn require_held_source(
        &self,
        node: &HeldSnapshotNode,
    ) -> Result<(), SnapshotStoreError> {
        self.require_source(node, &self.host)
    }

    pub(crate) fn prepare_directory<R: std::io::Read>(
        &self,
        manifest: crate::working_tree::snapshot::AdmittedSnapshotManifest,
        permit: &crate::engine::BudgetedDirectoryPreparation,
        source: impl FnMut(&crate::working_tree::snapshot::SnapshotFile)
            -> Result<R, SnapshotStoreError>,
    ) -> Result<HeldSnapshotNode, crate::working_tree::snapshot::SnapshotPreparationError> {
        permit.require_node(manifest.reference(), self.source)
            .map_err(SnapshotStoreError::Disk)
            .map_err(crate::working_tree::snapshot::SnapshotPreparationError::Rejected)?;
        HeldSnapshotNode::prepare(&self.repository, manifest, source)
    }

    pub(crate) fn require_free_content_node(&self, node: storage_contract::working_tree::SnapshotNodeId)
        -> Result<(), SnapshotStoreError>
    {
        let name = layout::node(node);
        if self.repository.optional_metadata(&self.repository.path().join(name.as_path()))
            .map_err(SnapshotStoreError::Disk)?.is_some()
        { return Err(SnapshotStoreError::Entries); }
        Ok(())
    }

    pub(crate) fn require_prepared(&self, node: &HeldSnapshotNode)
        -> Result<(), SnapshotStoreError>
    {
        if !node.binding.same_repository(self.source) {
            return Err(SnapshotStoreError::Repository);
        }
        self.host.private_entry_matching(
            &SNAPSHOT_REPOSITORY, file_identity(self.source.repository_entity),
        ).map_err(SnapshotStoreError::Directory)?;
        let reread = HeldSnapshotNode::reopen(&self.repository, node.binding)?;
        if reread.manifest.bytes() != node.manifest.bytes() {
            return Err(SnapshotStoreError::Entries);
        }
        Ok(())
    }
}

/// Encode-only maximum width for the not-yet-created node's three entities.
/// This is neither a SnapshotNodeBinding nor a decodable owner record. The
/// repository entity remains the exact independently published source.
impl SnapshotNodeBinding {
    pub(crate) fn budget_encoding(
        self,
        reference: crate::working_tree::snapshot::SnapshotReference,
    ) -> impl data_serialization::Serialize {
        #[derive(data_serialization::Serialize)]
        struct WideEntity {
            #[serialization(with = "storage_contract::u64_string")]
            device: u64,
            #[serialization(with = "storage_contract::u64_string")]
            inode: u64,
        }
        #[derive(data_serialization::Serialize)]
        struct Encoding {
            reference: crate::working_tree::snapshot::SnapshotReference,
            repository_entity: storage_contract::working_tree::LocalEntityIdentity,
            node_entity: WideEntity,
            files_entity: WideEntity,
            worktrees_entity: WideEntity,
        }
        let widest = || WideEntity { device: u64::MAX, inode: u64::MAX };
        Encoding {
            reference,
            repository_entity: self.repository_entity,
            node_entity: widest(),
            files_entity: widest(),
            worktrees_entity: widest(),
        }
    }
}
