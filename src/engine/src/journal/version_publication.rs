//! Original publication validation, separate from the current physical layout.
//!
//! LocalPublication includes the complete operation result and stays separate
//! from pathless FILE rows. The ordinary reader routes these proof reads through
//! its actual retained data/management roots; their common verification kernel
//! does not infer a source from the payload path or mint a publication.
use crate::disk::{
    DiskHandle, DiskRoot, DiskSeal, MAX_METADATA, decode_seal,
};
use crate::engine::provenance::MetadataReadContext;
use crate::journal::{
    Candidate, CommitProof, ContentTombstone, LocalPublication, PublicationSeal, decode_proof,
    validate_snapshot_schema, verify_local_membership, verify_publication_seal,
};
use crate::persist::layout;
use crate::{ContentState, Error, ErrorCode, FileVersion, Result, object_hash};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

/// Only original proof files cross this boundary. Membership has a separate
/// operation so its shared-authority failures retain their own fault scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VersionProofMaterial {
    Publication,
    Commit,
    DiskIdentity,
}

trait VersionPublicationRead {
    fn read(&mut self, material: VersionProofMaterial) -> Result<Vec<u8>>;

    fn verify_membership(
        &mut self,
        candidate: &Candidate,
        seal: &DiskSeal,
        metadata: Option<&MetadataReadContext<'_>>,
    ) -> Result<()>;
}

/// Local material is source-scoped; disk identity and membership are not.
/// No ancestor of a version address can supply destination disk authority.
struct SourceVersionProofReader<'a, 'source, O> {
    material: &'a crate::disk::VersionMaterial<'source>,
    observe: O,
}

impl<O: FnMut(usize) -> Result<()>> VersionPublicationRead
    for SourceVersionProofReader<'_, '_, O>
{
    fn read(&mut self, material: VersionProofMaterial) -> Result<Vec<u8>> {
        match material {
            VersionProofMaterial::Publication => self.material.root()?.read_bytes_observed(
                &self.material.directory()?.join(layout::VERSION_PUBLICATION_PROOF.as_path()),
                MAX_METADATA, &mut self.observe,
            ),
            VersionProofMaterial::Commit => self.material.root()?.read_bytes_observed(
                &self.material.directory()?.join(layout::VERSION_COMMIT_PROOF.as_path()),
                MAX_METADATA, &mut self.observe,
            ),
            VersionProofMaterial::DiskIdentity => {
                let root = &self.material.disk().root;
                root.read_bytes_observed(
                    &root.path().join(layout::DISK_IDENTITY.as_path()),
                    MAX_METADATA,
                    &mut self.observe,
                )
            }
        }
    }

    fn verify_membership(
        &mut self,
        candidate: &Candidate,
        seal: &DiskSeal,
        metadata: Option<&MetadataReadContext<'_>>,
    ) -> Result<()> {
        let disk = self.material.disk();
        verify_local_membership(
            disk.root.path(), candidate, seal,
            &mut |path| disk.root_for(path)?.read_bytes_observed(
                path, MAX_METADATA, &mut self.observe,
            ),
            metadata,
        )
    }
}

/// Target serving starts only after the genuine native first Publication.
/// Its source-chain entities were bound by that decision, but it is not an
/// Engine Current and must not be converted to an ordinary DiskHandle scope.
struct BackupServingProofReader<'a, 'image, O> {
    material: &'a crate::container_storage::BackupServingVersion<'image>,
    observe: O,
}
impl<O: FnMut(usize) -> Result<()>> VersionPublicationRead
    for BackupServingProofReader<'_, '_, O>
{
    fn read(&mut self, material: VersionProofMaterial) -> Result<Vec<u8>> {
        match material {
            VersionProofMaterial::Publication => self.material.root()?.read_bytes_observed(
                &self.material.directory()?.join(layout::VERSION_PUBLICATION_PROOF.as_path()),
                MAX_METADATA, &mut self.observe,
            ),
            VersionProofMaterial::Commit => self.material.root()?.read_bytes_observed(
                &self.material.directory()?.join(layout::VERSION_COMMIT_PROOF.as_path()),
                MAX_METADATA, &mut self.observe,
            ),
            VersionProofMaterial::DiskIdentity => {
                let root = self.material.global()?;
                root.read_bytes_observed(&root.path().join(layout::DISK_IDENTITY.as_path()),
                    MAX_METADATA, &mut self.observe)
            }
        }
    }
    fn verify_membership(
        &mut self, candidate: &Candidate, seal: &DiskSeal,
        metadata: Option<&MetadataReadContext<'_>>,
    ) -> Result<()> {
        let root = self.material.global()?;
        verify_local_membership(root.path(), candidate, seal,
            &mut |path| root.read_bytes_observed(path, MAX_METADATA, &mut self.observe), metadata)
    }
}

pub(crate) fn verify_backup_serving_publication(
    material: &crate::container_storage::BackupServingVersion<'_>,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    material.verify()?;
    verify_local_publication_with(material.version(),
        &mut BackupServingProofReader { material, observe },
        &mut || material.root()?.open_optional_read_regular(
            &material.directory()?.join(layout::VERSION_TOMBSTONE.as_path()),
        ).map(|file| file.is_some()), None)?;
    material.verify()
}

pub(crate) fn verify_source_publication(
    material: &crate::disk::VersionMaterial<'_>,
    observe: &mut impl FnMut(usize) -> Result<()>,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    let mut source = SourceVersionProofReader { material, observe };
    verify_local_publication_with(
        material.version(), &mut source,
        &mut || material.root()?.open_optional_read_regular(
            &material.directory()?.join(layout::VERSION_TOMBSTONE.as_path()),
        ).map(|file| file.is_some()),
        metadata,
    )?;
    material.verify()
}

pub(crate) fn verify_source_version_publication(
    material: &crate::disk::VersionMaterial<'_>,
    observe: &mut impl FnMut(usize) -> Result<()>,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    verify_local_version_publication(
        material.version(), &mut SourceVersionProofReader { material, observe }, metadata,
    )?;
    material.verify()
}

/// A tombstone does not move a live container into the Deleted lifecycle.
/// This validates content availability independently of the Current source.
pub(crate) fn verify_source_tombstone(
    material: &crate::disk::VersionMaterial<'_>,
    tombstone: &ContentTombstone,
    observe: &mut impl FnMut(usize) -> Result<()>,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    verify_tombstone_content_with(
        material.version(), tombstone,
        &mut SourceVersionProofReader { material, observe }, metadata,
    )?;
    material.verify()
}

fn read_local_seal(
    source: &mut impl VersionPublicationRead,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<DiskSeal> {
    let loaded = source
        .read(VersionProofMaterial::DiskIdentity)
        .and_then(|bytes| decode_seal(&bytes));
    match metadata {
        Some(metadata) => metadata.seal(loaded),
        None => loaded,
    }
}

// Raw locators deliberately retain no source-proof I/O. Neither a path nor a
// global disk seal authenticates a source chain/lifecycle. These old entry
// points reject; only VersionMaterial enters the unchanged verification kernel.
pub(crate) fn verify_local_publication(_directory: &Path, _version: &FileVersion) -> Result<()> {
    Err(crate::engine::FreshLayoutError::VersionSource.into())
}

pub(crate) fn verify_local_publication_in_context(
    _directory: &Path,
    _version: &FileVersion,
    _metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    Err(crate::engine::FreshLayoutError::VersionSource.into())
}

pub(crate) fn verify_local_publication_observed(
    _directory: &Path,
    _version: &FileVersion,
    _observe: &mut impl FnMut(usize) -> Result<()>,
    _metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    Err(crate::engine::FreshLayoutError::VersionSource.into())
}

pub(crate) fn verify_local_publication_on_disk(
    _disk: &DiskHandle,
    _directory: &Path,
    _version: &FileVersion,
    _observe: &mut impl FnMut(usize) -> Result<()>,
    _metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    Err(crate::engine::FreshLayoutError::VersionSource.into())
}

pub(crate) fn verify_local_publication_at(
    _root: &DiskRoot,
    _directory: &Path,
    _version: &FileVersion,
    _observe: &mut impl FnMut(usize) -> Result<()>,
    _metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    Err(crate::engine::FreshLayoutError::VersionSource.into())
}

fn verify_local_publication_with(
    version: &FileVersion,
    source: &mut impl VersionPublicationRead,
    tombstone_present: &mut impl FnMut() -> Result<bool>,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    verify_local_version_publication(version, source, metadata)?;
    if tombstone_present()? {
        return Err(Error::new(
            ErrorCode::ContentPurged,
            "this version has a local content tombstone",
        ));
    }
    Ok(())
}

/// A held raw root alone cannot admit a retained version source. Actual Current
/// content cleanup uses verify_source_version_publication with its scope.
pub(crate) fn verify_retained_version_publication_at(
    _root: &DiskRoot,
    _directory: &Path,
    _version: &FileVersion,
    _metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    Err(crate::engine::FreshLayoutError::VersionSource.into())
}

/// Verify immutable publication identity independently of current availability.
/// No path or FILE material can stand in for original local publication evidence.
fn verify_local_version_publication(
    version: &FileVersion,
    source: &mut impl VersionPublicationRead,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    let local: LocalPublication = decode_proof(&source.read(VersionProofMaterial::Publication)?)?;
    let proof: CommitProof = decode_proof(&source.read(VersionProofMaterial::Commit)?)?;
    let candidate_hash = object_hash("candidate", &local.candidate)?;
    let c = &local.candidate;
    let p = &local.publication;
    verify_publication_seal(
        c,
        p,
        &PublicationSeal {
            candidate_hash: candidate_hash.clone(),
            publication_hash: object_hash("publication", p)?,
            receipts: local.publication_receipts.clone(),
        },
    )?;
    let seal = read_local_seal(source, metadata)?;
    source.verify_membership(c, &seal, metadata)?;
    let finalize_hash = object_hash("finalize", &p.finalize)?;
    if p.candidate_hash != candidate_hash
        || proof.candidate_hash != candidate_hash
        || object_hash("commit-proof", &proof)? != object_hash("commit-proof", &local.commit_proof)?
        || proof.version_hash != object_hash("version", version)?
        || seal.pool_id != c.pool_id
        || seal.disk_id != proof.disk_id
        || seal.slot_id != proof.slot_id
        || object_hash("finalize", &proof.finalize)? != finalize_hash
    {
        return Err(Error::new(
            storage_contract::ErrorCode::PublicationInvalid,
            "local proof does not bind the version",
        ));
    }
    let evidence = c
        .versions
        .iter()
        .find(|e| e.version.version_id == version.version_id)
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "version is absent from candidate",
            )
        })?;
    if object_hash("version", &evidence.version)? != object_hash("version", version)?
        || evidence.placement.route_slot_ids != proof.route_slot_ids
    {
        return Err(Error::new(
            storage_contract::ErrorCode::PublicationInvalid,
            "candidate placement mismatch",
        ));
    }
    let receipt = p
        .payload_receipts
        .iter()
        .find(|r| {
            r.version_id == version.version_id
                && r.disk_id == proof.disk_id
                && r.slot_id == proof.slot_id
        })
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "local payload receipt absent",
            )
        })?;
    if receipt.proof_hash != object_hash("commit-proof", &proof)? {
        return Err(Error::new(
            storage_contract::ErrorCode::PublicationInvalid,
            "local receipt digest differs",
        ));
    }
    for slot in &evidence.placement.route_slot_ids {
        if !p
            .payload_receipts
            .iter()
            .any(|r| r.version_id == version.version_id && &r.slot_id == slot)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "a required route receipt is absent",
            ));
        }
    }
    Ok(())
}

pub(crate) fn verify_tombstone_content_in_context(
    _directory: &Path,
    _version: &FileVersion,
    _t: &ContentTombstone,
    _metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    Err(crate::engine::FreshLayoutError::VersionSource.into())
}

pub(crate) fn verify_tombstone_content_at(
    _root: &DiskRoot,
    _directory: &Path,
    _version: &FileVersion,
    _t: &ContentTombstone,
    _observe: &mut impl FnMut(usize) -> Result<()>,
    _metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    Err(crate::engine::FreshLayoutError::VersionSource.into())
}

fn verify_tombstone_content_with(
    version: &FileVersion,
    t: &ContentTombstone,
    source: &mut impl VersionPublicationRead,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    // Intentional removal proves the content state, not the retained descriptor.
    // Its immutable identity and location must still bind to the local publication.
    verify_local_version_publication(version, source, metadata)?;
    verify_publication_seal(&t.candidate, &t.publication, &t.publication_seal)?;
    let seal = read_local_seal(source, metadata)?;
    source.verify_membership(&t.candidate, &seal, metadata)?;
    validate_snapshot_schema(&t.snapshot)?;
    let content_state = t
        .content_state_proof
        .verify(&t.snapshot.maps["content_states"], &t.version_id)?;
    if t.version_id != version.version_id
        || !matches!(
            t.state,
            ContentState::Purged | ContentState::Migrated | ContentState::Retired
        )
        || t.candidate.pool_id != seal.pool_id
        || t.publication.candidate_hash != object_hash("candidate", &t.candidate)?
        || object_hash("snapshot", &t.snapshot)? != t.candidate.snapshot_hash
        || t.snapshot.sequence != t.candidate.sequence
        || content_state.as_ref().and_then(json_codec::Value::as_str) != Some(t.state.as_str())
    {
        return Err(Error::new(
            storage_contract::ErrorCode::TombstoneInvalid,
            "content tombstone is not covered by a published state",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/journal/version_publication_tests.rs"]
mod tests;
