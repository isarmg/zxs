//! Original publication validation, separate from the current physical layout.
//!
//! LocalPublication includes the complete operation result, which can contain
//! exterior paths. A future unified layout must keep it and member authority in
//! the disk proof domain, not the pathless FILE database. This private protocol
//! does not grant a new-layout constructor or a FILE body-read capability.
use crate::disk::{DiskRoot, DiskSeal, MAX_METADATA, absolute_path, decode_seal, read_bytes};
use crate::engine::provenance::MetadataReadContext;
use crate::journal::{
    Candidate, CommitProof, ContentTombstone, LocalPublication, PublicationSeal, decode_proof,
    validate_snapshot_schema, verify_local_membership, verify_publication_seal,
};
use crate::persist::layout;
use crate::{ContentState, Error, ErrorCode, FileVersion, Result, object_hash};
use std::path::{Path, PathBuf};

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

/// This is the sole constructor used by the still-active layout. It proves
/// location identity, not publication or current body availability. There is no
/// arbitrary-path constructor for the unified DISK proof domain.
struct CurrentVersionProofLocation<'a> {
    directory: &'a Path,
    root: &'a Path,
}

impl<'a> CurrentVersionProofLocation<'a> {
    fn admit(directory: &'a Path, version: &FileVersion) -> Result<Self> {
        if directory.file_name().and_then(|s| s.to_str()) != Some(version.version_id.as_str())
            || directory.ancestors().nth(1).and_then(Path::file_name)
                != Some(layout::VERSIONS.as_os_str())
            || directory
                .ancestors()
                .nth(2)
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                != Some(version.history_tree_id.as_str())
            || directory.ancestors().nth(3).and_then(Path::file_name)
                != Some(layout::HISTORIES.as_os_str())
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "version directory identity differs from its descriptor",
            ));
        }
        let root = directory.ancestors().nth(4).ok_or_else(|| {
            Error::new(ErrorCode::PublicationInvalid, "version is outside disk layout")
        })?;
        Ok(Self { directory, root })
    }

    fn material(&self, material: VersionProofMaterial) -> PathBuf {
        match material {
            VersionProofMaterial::Publication => {
                self.directory.join(layout::VERSION_PUBLICATION_PROOF.as_path())
            }
            VersionProofMaterial::Commit => {
                self.directory.join(layout::VERSION_COMMIT_PROOF.as_path())
            }
            VersionProofMaterial::DiskIdentity => self.root.join(layout::DISK_IDENTITY.as_path()),
        }
    }

    fn tombstone(&self) -> PathBuf {
        self.directory.join(layout::VERSION_TOMBSTONE.as_path())
    }
}

struct CurrentVersionProofReader<'a, R> {
    location: CurrentVersionProofLocation<'a>,
    read: R,
}

impl<R: FnMut(&Path) -> Result<Vec<u8>>> VersionPublicationRead
    for CurrentVersionProofReader<'_, R>
{
    fn read(&mut self, material: VersionProofMaterial) -> Result<Vec<u8>> {
        (self.read)(&self.location.material(material))
    }

    fn verify_membership(
        &mut self,
        candidate: &Candidate,
        seal: &DiskSeal,
        metadata: Option<&MetadataReadContext<'_>>,
    ) -> Result<()> {
        verify_local_membership(self.location.root, candidate, seal, &mut self.read, metadata)
    }
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

pub(crate) fn verify_local_publication(directory: &Path, version: &FileVersion) -> Result<()> {
    verify_local_publication_in_context(directory, version, None)
}

pub(crate) fn verify_local_publication_in_context(
    directory: &Path,
    version: &FileVersion,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    verify_local_publication_observed(directory, version, &mut |_| Ok(()), metadata)
}

pub(crate) fn verify_local_publication_observed(
    directory: &Path,
    version: &FileVersion,
    observe: &mut impl FnMut(usize) -> Result<()>,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    let directory = absolute_path(directory)?;
    let root = directory.ancestors().nth(4).ok_or_else(|| {
        Error::new(
            ErrorCode::PublicationInvalid,
            "version is outside disk layout",
        )
    })?;
    let root = DiskRoot::open(root)?;
    verify_local_publication_at(&root, &directory, version, observe, metadata)
}

pub(crate) fn verify_local_publication_at(
    root: &DiskRoot,
    directory: &Path,
    version: &FileVersion,
    observe: &mut impl FnMut(usize) -> Result<()>,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    let location = CurrentVersionProofLocation::admit(directory, version)?;
    let tombstone = location.tombstone();
    let mut source = CurrentVersionProofReader {
        location,
        read: |path: &Path| root.read_bytes_observed(path, MAX_METADATA, observe),
    };
    verify_local_publication_with(
        version,
        &mut source,
        &mut || {
            root.open_optional_read_regular(&tombstone)
                .map(|file| file.is_some())
        },
        metadata,
    )
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

/// Recheck the original version identity on a held root even when a previous
/// cleanup already installed a tombstone. This does not grant body availability.
pub(crate) fn verify_retained_version_publication_at(
    root: &DiskRoot,
    directory: &Path,
    version: &FileVersion,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    let mut source = CurrentVersionProofReader {
        location: CurrentVersionProofLocation::admit(directory, version)?,
        read: |path: &Path| root.read_bytes(path, MAX_METADATA),
    };
    verify_local_version_publication(version, &mut source, metadata)
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
    directory: &Path,
    version: &FileVersion,
    t: &ContentTombstone,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    verify_tombstone_content_with(
        version,
        t,
        &mut CurrentVersionProofReader {
            location: CurrentVersionProofLocation::admit(directory, version)?,
            read: |path: &Path| read_bytes(path, MAX_METADATA),
        },
        metadata,
    )
}

pub(crate) fn verify_tombstone_content_at(
    root: &DiskRoot,
    directory: &Path,
    version: &FileVersion,
    t: &ContentTombstone,
    observe: &mut impl FnMut(usize) -> Result<()>,
    metadata: Option<&MetadataReadContext<'_>>,
) -> Result<()> {
    verify_tombstone_content_with(
        version,
        t,
        &mut CurrentVersionProofReader {
            location: CurrentVersionProofLocation::admit(directory, version)?,
            read: |path: &Path| root.read_bytes_observed(path, MAX_METADATA, observe),
        },
        metadata,
    )
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
