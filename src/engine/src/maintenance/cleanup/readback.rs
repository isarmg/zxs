//! Hold the authenticated version directory and the original tombstone across
//! both named leaf observations. Never let proof from a detached parent clear
//! the obligation on its replacement. This object has no mutation entry point.
use super::{
    BodyLeaf, ContentTombstone, DiskRoot, FileVersion, FreshLayoutError, MAX_METADATA,
    Result, VersionMaterial, decode_proof, disk, layout, require_same_directory,
};

pub(crate) struct CleanupReadback {
    directory: DiskRoot,
    tombstone: BodyLeaf,
    descriptor: Option<BodyLeaf>,
    _descriptors: crate::engine::PayloadProofReservation,
}
impl CleanupReadback {
    pub(crate) fn capture(material: &VersionMaterial<'_>) -> Result<Self> {
        // Version directory; held descriptor, tombstone, installed and prepared leaves
        // and their parents; one current leaf reopened for comparison.
        let descriptors = crate::engine::payload_proof_budget()
            .reserve(10).ok_or(FreshLayoutError::ContainerBudget)?;
        let root = material.root()?;
        let directory = root.private_child(&material.directory()?)?;
        let tombstone = BodyLeaf::open(
            &directory, &directory.path().join(layout::VERSION_TOMBSTONE.as_path()),
        )?;
        let descriptor = if tombstone.file.is_some() {
            Some(BodyLeaf::open(
                &directory, &directory.path().join(layout::VERSION_DESCRIPTOR.as_path()),
            )?)
        } else { None };
        Ok(Self { directory, tombstone, descriptor, _descriptors: descriptors })
    }

    pub(crate) fn read_proof(
        &self, expected: &FileVersion, observe: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<Option<ContentTombstone>> {
        let Some(held) = &self.tombstone.file else { return Ok(None) };
        let descriptor = self.descriptor.as_ref().and_then(|leaf| leaf.file.as_ref())
            .ok_or(FreshLayoutError::CleanupDescriptor)?;
        let stored: FileVersion = crate::decode_strict(&disk::read_file_bytes_observed(
            descriptor.try_clone()?, MAX_METADATA, observe,
        )?)?;
        if &stored != expected { return Err(FreshLayoutError::CleanupDescriptor.into()); }
        let bytes = disk::read_file_bytes_observed(held.try_clone()?, MAX_METADATA, observe)?;
        decode_proof(&bytes).map(Some)
    }

    pub(crate) fn observe_bodies(
        self, material: &VersionMaterial<'_>, checkpoint: &mut impl FnMut() -> Result<()>,
    ) -> Result<bool> {
        let installed = material.body_path()?;
        let present = observe_pair(
            material.container()?, &installed, &self.directory, &material.prepared_body_path()?,
            checkpoint,
        )?;
        if material.body_path()? != installed {
            return Err(FreshLayoutError::CleanupChanged.into());
        }
        self.descriptor.as_ref().ok_or(FreshLayoutError::CleanupDescriptor)?
            .recheck(&self.directory)?;
        self.tombstone.recheck(&self.directory)?;
        require_same_directory(material.root()?, &self.directory)?;
        material.verify()?;
        Ok(present)
    }
}

fn observe_pair(
    container: &DiskRoot, installed: &std::path::Path,
    directory: &DiskRoot, prepared: &std::path::Path,
    checkpoint: &mut impl FnMut() -> Result<()>,
) -> Result<bool> {
    let installed = BodyLeaf::open(container, installed)?;
    checkpoint()?;
    let prepared = BodyLeaf::open(directory, prepared)?;
    checkpoint()?;
    installed.recheck(container)?;
    prepared.recheck(directory)?;
    Ok(installed.file.is_some() || prepared.file.is_some())
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/cleanup_readback_tests.rs"]
mod tests;
