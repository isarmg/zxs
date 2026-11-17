//! Physical cleanup is not the published logical decision. Recheck the original
//! occurrence and both exact body leaves without manufacturing missing evidence.
use crate::disk::{self, DiskRoot, HeldRegular, VersionMaterial, MAX_METADATA};
use crate::engine::{FreshLayoutError, provenance::MetadataReadContext};
use crate::journal::{
    ContentTombstone, decode_proof, verify_source_tombstone, verify_source_version_publication,
};
use crate::persist::layout;
use crate::{ChecksumManifest, ContentState, FileVersion, ReadFile, Result};
use platform_io::fs_safety::EntryName;
use std::{fs::Metadata, os::unix::fs::MetadataExt, path::Path};

/// The exclusive engine writer owns mutations. Also keep the exact parent and
/// file open and reject any changed named entity observed before the unlink.
/// An observed absence only synchronizes that same parent, never unlinks.
struct BodyLeaf {
    parent: DiskRoot,
    name: EntryName,
    file: Option<ReadFile>,
    original: Option<Metadata>,
}

impl BodyLeaf {
    fn open(source: &DiskRoot, path: &Path) -> Result<Self> {
        let parent = source.private_child(path.parent().ok_or(FreshLayoutError::ContainerScope)?)?;
        let name = EntryName::new(path.file_name().ok_or(FreshLayoutError::ContainerScope)?)
            .map_err(disk::fs_error)?;
        let file = parent.open_optional_read_entry(&name)?;
        let original = file.as_ref().map(ReadFile::metadata).transpose()?;
        Ok(Self { parent, name, file, original })
    }

    fn verify_bytes(&mut self, version: &FileVersion, manifest: &ChecksumManifest) -> Result<()> {
        if let Some(file) = self.file.take() {
            self.file = Some(disk::verify_open_payload_observed(
                file, manifest, version, &mut |_| Ok(()),
            )?);
        }
        Ok(())
    }

    fn recheck(&self, source: &DiskRoot) -> Result<()> {
        require_same_directory(source, &self.parent)?;
        let named = self.parent.open_optional_read_entry(&self.name)?;
        match (&self.file, &self.original, named) {
            (None, None, None) => Ok(()),
            (Some(held), Some(original), Some(named)) => {
                held.check_running()?;
                if same_regular(original, &held.metadata()?)
                    && same_regular(original, &named.metadata()?)
                {
                    return Ok(());
                }
                Err(FreshLayoutError::CleanupChanged.into())
            }
            _ => Err(FreshLayoutError::CleanupChanged.into()),
        }
    }

    fn remove(self, source: &DiskRoot) -> Result<()> {
        self.recheck(source)?;
        if self.file.is_some() {
            self.parent.remove_file_synced(&self.parent.path().join(self.name.as_path()))
        } else {
            // A prior unlink may have failed its durability tail.
            self.parent.sync_dir(self.parent.path())
        }
    }
}

fn require_same_directory(source: &DiskRoot, held: &DiskRoot) -> Result<()> {
    if source.directory_identity(held.path())? != held.directory_identity(held.path())? {
        return Err(FreshLayoutError::CleanupChanged.into());
    }
    Ok(())
}

fn same_regular(original: &Metadata, named: &Metadata) -> bool {
    original.is_file() && named.is_file() && original.nlink() == 1 && named.nlink() == 1
        && original.dev() == named.dev() && original.ino() == named.ino()
        && original.len() == named.len() && original.mtime() == named.mtime()
        && original.mtime_nsec() == named.mtime_nsec() && original.ctime() == named.ctime()
        && original.ctime_nsec() == named.ctime_nsec()
}

mod readback;
pub(crate) use readback::CleanupReadback;

/// Never rewrite an equivalent authenticated local decision simply because an
/// unrelated global publication is newer. Only admitted irreversible forwards
/// can replace an older local decision; an occupied unproven slot is not ours.
fn preserve_existing(old: &ContentTombstone, now: &ContentTombstone) -> Result<bool> {
    let same_decision = old.publication.candidate_hash == now.publication.candidate_hash
        && old.publication_seal.publication_hash == now.publication_seal.publication_hash;
    if old.version_id == now.version_id
        && (old.candidate.sequence < now.candidate.sequence
            || (old.candidate.sequence == now.candidate.sequence && same_decision))
    {
        if old.state == now.state {
            return Ok(true);
        }
        if old.candidate.sequence < now.candidate.sequence
            && matches!((old.state, now.state),
                (ContentState::Migrated, ContentState::Purged)
                | (ContentState::Purged, ContentState::Retired))
        {
            return Ok(false);
        }
    }
    Err(FreshLayoutError::CleanupPredecessor.into())
}

pub(super) fn remove_proven_bodies(
    material: &VersionMaterial<'_>,
    decision: &ContentTombstone,
    metadata: &MetadataReadContext<'_>,
) -> Result<()> {
    let source = material.root()?;
    let directory = source.private_child(&material.directory()?)?;
    let version = material.version();
    let stored: FileVersion = directory.read_json_observed(
        &directory.path().join(layout::VERSION_DESCRIPTOR.as_path()), &mut |_| Ok(()),
    )?;
    if &stored != version {
        return Err(FreshLayoutError::CleanupDescriptor.into());
    }
    let name = &*layout::VERSION_TOMBSTONE;
    let mut old = HeldRegular::capture_entry(&directory, name, MAX_METADATA)?;
    let preserve = if let Some(old) = old.as_mut() {
        let prior: ContentTombstone = decode_proof(old.bytes())?;
        old.release_bytes();
        verify_source_tombstone(material, &prior, &mut |_| Ok(()), Some(metadata))?;
        preserve_existing(&prior, decision)?
    } else {
        verify_source_version_publication(material, &mut |_| Ok(()), Some(metadata))?;
        false
    };

    let container = material.container()?;
    let mut installed = BodyLeaf::open(container, &material.body_path()?)?;
    let mut prepared = BodyLeaf::open(&directory, &material.prepared_body_path()?)?;
    if installed.file.is_some() || prepared.file.is_some() {
        let checks = disk::read_authenticated_checksums_at(&directory, directory.path(), version)?;
        let manifest = disk::decode_authenticated_checksums(&checks, version)?;
        installed.verify_bytes(version, &manifest)?;
        prepared.verify_bytes(version, &manifest)?;
    }

    material.verify()?;
    require_same_directory(source, &directory)?;
    installed.recheck(container)?;
    prepared.recheck(&directory)?;
    let named = HeldRegular::capture_entry(&directory, name, MAX_METADATA)?;
    if named.as_ref().map(HeldRegular::stamp) != old.as_ref().map(HeldRegular::stamp) {
        return Err(FreshLayoutError::CleanupChanged.into());
    }
    if preserve {
        directory.sync_dir(directory.path())?;
    } else {
        // A previously absent leaf must never replace a newly arrived object.
        directory.write_entry_json(name, decision, old.is_some())?;
    }
    installed.remove(container)?;
    prepared.remove(&directory)?;
    material.verify()
}
