//! Source-scoped version metadata. The source occurrence and lifecycle are
//! supplied only by the actual container/prefix admission, never a pathname.
use crate::container_storage::ObservedContainerReplica;
use crate::disk::{DiskHandle, DiskRoot};
use crate::persist::layout;
use crate::{FileVersion, Result};
use std::path::PathBuf;
use std::sync::Arc;
use storage_contract::{HistoryTreeId, OperationId, VersionId};

/// This borrow keeps the same prepared or published source occurrence alive.
/// A prefix rename changes its named address, not the retained lifecycle FD.
/// Old versions of a live file remain Current; this type cannot mint Deleted.
pub(crate) struct VersionMaterial<'a> {
    source: &'a ObservedContainerReplica,
    version: &'a FileVersion,
    history: HistoryTreeId,
    id: VersionId,
}

impl<'a> VersionMaterial<'a> {
    pub(crate) fn current(
        source: &'a ObservedContainerReplica,
        version: &'a FileVersion,
    ) -> Result<Self> {
        source.require_version(version)?;
        let history = version.history_tree_id.parse()?;
        let id = version.version_id.parse()?;
        source.current_zxs()?;
        Ok(Self { source, version, history, id })
    }

    pub(crate) fn root(&self) -> Result<&DiskRoot> {
        self.source.current_zxs()
    }

    pub(crate) fn disk(&self) -> &Arc<DiskHandle> {
        self.source.disk()
    }

    pub(crate) fn version(&self) -> &'a FileVersion {
        self.version
    }

    pub(crate) fn directory(&self) -> Result<PathBuf> {
        Ok(self.root()?.path().join(layout::source_version(self.history, self.id)))
    }

    pub(crate) fn preparation(&self, operation: OperationId) -> Result<PathBuf> {
        Ok(self.root()?.path().join(layout::source_version_preparation(self.history, operation)))
    }

    pub(crate) fn container(&self) -> Result<&DiskRoot> {
        self.source.container()
    }

    pub(crate) fn body_path(&self) -> Result<PathBuf> {
        self.source.body_path(self.version)
    }

    /// The source-side leaf left by an interrupted install is separate from the
    /// installed container body. Its original creation name never follows a rename.
    pub(crate) fn prepared_body_path(&self) -> Result<PathBuf> {
        Ok(self
            .directory()?
            .join(layout::PREPARED_CONTENT.as_path())
            .join(&self.version.creation_name))
    }

    pub(crate) fn open_payload(
        &self,
    ) -> Result<(crate::engine::ReadFile, Vec<u8>, crate::ChecksumManifest)> {
        let checks = crate::disk::read_authenticated_checksums_at(
            self.root()?, &self.directory()?, self.version,
        )?;
        let manifest = crate::disk::decode_authenticated_checksums(&checks, self.version)?;
        let file = self.container()?.open_leased_regular(&self.body_path()?)?;
        self.verify()?;
        Ok((crate::engine::ReadFile::from(file), checks, manifest))
    }

    pub(crate) fn verify_payload(&self) -> Result<crate::engine::ReadFile> {
        let (file, _, manifest) = self.open_payload()?;
        let file = crate::disk::verify_open_payload(
            file, manifest, self.version, usize::MAX, &mut |_| Ok(()),
        )?;
        self.verify()?;
        Ok(file)
    }

    pub(crate) fn verify(&self) -> Result<()> {
        self.source.verify()
    }
}
