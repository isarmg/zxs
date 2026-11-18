//! A pack has one private inode and a bounded checksum trailer. No constructor
//! below turns it into an accepted save, a history version or a disk authority.
use super::changed;
use crate::business::file_draft::DraftError as Failure;
use crate::disk::DiskRoot;
use crate::engine::{PayloadProofReservation, payload_proof_budget};
use crate::file_stamp::FileStamp;
use crate::persist::layout::container::working_pack;
use crate::{DiskFile, Error, ReadFile, Result};
use platform_io::fs_safety::linux::FileIdentity;
use std::os::unix::fs::{FileExt, MetadataExt};
use storage_contract::file_container::ContainerEntity;
use storage_contract::file_draft::manifest::{ManifestError, packed::{
    AppendIndex, CheckedIndex, PackId, PackOrigin, VERIFY_CHUNK, WorkingPack,
}};

fn manifest_error(error: ManifestError) -> Error { Error::FileDraft(Failure::WorkingManifest(error)) }
fn descriptor() -> Result<PayloadProofReservation> {
    payload_proof_budget().reserve(1).ok_or(Error::FileDraft(Failure::Size))
}

struct Location {
    parent: FileIdentity,
    id: PackId,
}
impl Location {
    fn capture(root: &DiskRoot, id: PackId) -> Result<Self> {
        crate::container_storage::version_names::check_target_name(root, &working_pack(id))?;
        Ok(Self { parent: root.directory_identity(root.path())?, id })
    }
    fn matches(&self, root: &DiskRoot, actual: &std::fs::Metadata) -> Result<()> {
        if root.directory_identity(root.path())? != self.parent || !actual.is_file()
            || actual.nlink() != 1
        { return Err(changed()); }
        let named = root.open_read_entry(&working_pack(self.id))?;
        if FileStamp::from_metadata(&named.metadata()?) != FileStamp::from_metadata(actual) {
            return Err(changed());
        }
        Ok(())
    }
}

/// Connection-owned bytes and their input-derived partial checksum; an I/O
/// failure poisons this pack. Observing the name can never recreate this type.
pub(super) struct PendingPack {
    location: Location,
    file: DiskFile,
    stamp: FileStamp,
    index: Option<AppendIndex>,
    origin: PackOrigin,
    _fd: PayloadProofReservation,
}
impl PendingPack {
    pub(super) fn create(root: &DiskRoot, id: PackId, origin: PackOrigin) -> Result<Self> {
        let location = Location::capture(root, id)?;
        let fd = descriptor()?;
        let file = root.create_private_working_pack(id)?;
        let metadata = file.metadata()?;
        if metadata.len() != 0 { return Err(changed()); }
        location.matches(root, &metadata)?;
        Ok(Self { location, file, origin, stamp: FileStamp::from_metadata(&metadata),
            index: Some(AppendIndex::default()), _fd: fd })
    }
    fn index(&self) -> Result<&AppendIndex> {
        self.index.as_ref().ok_or(Error::FileDraft(Failure::WorkingEditFailed))
    }
    fn validate(&self, root: &DiskRoot) -> Result<()> {
        let metadata = self.file.metadata()?;
        if self.stamp != FileStamp::from_metadata(&metadata) { return Err(changed()); }
        self.location.matches(root, &metadata)
    }
    pub(super) fn id(&self) -> PackId { self.location.id }
    pub(super) fn remaining(&self) -> Result<u64> { Ok(self.index()?.remaining()) }
    pub(super) fn append(&mut self, root: &DiskRoot, bytes: &[u8]) -> Result<u64> {
        self.validate(root)?;
        let mut index = self.index.take().ok_or(Error::FileDraft(Failure::WorkingEditFailed))?;
        if bytes.is_empty() || bytes.len() as u64 > index.remaining() || self.stamp.size != index.bytes() {
            return Err(Error::FileDraft(Failure::Size));
        }
        let start = index.bytes();
        self.file.write_all_at(bytes, start)?;
        index.append(bytes).map_err(manifest_error)?;
        let metadata = self.file.metadata()?;
        if metadata.len() != index.bytes() { return Err(changed()); }
        self.location.matches(root, &metadata)?;
        self.stamp = FileStamp::from_metadata(&metadata);
        self.index = Some(index);
        Ok(start)
    }
    pub(super) fn read_at(&self, root: &DiskRoot, output: &mut [u8], offset: u64) -> Result<()> {
        self.validate(root)?;
        let index = self.index()?;
        read_checked(index.bytes(), offset, output, |n| index.expected_chunk(n),
            |buffer, position| self.file.read_exact_at(buffer, position))?;
        self.validate(root)
    }
    pub(super) fn seal(self, root: &DiskRoot) -> Result<HeldPack> {
        self.validate(root)?;
        let metadata = self.file.metadata()?;
        let entity = ContainerEntity::new(metadata.dev(), metadata.ino());
        let index = self.index.ok_or(Error::FileDraft(Failure::WorkingEditFailed))?;
        let (pack, trailer) = index.finish(self.location.id, self.origin, entity).map_err(manifest_error)?;
        self.file.write_all_at(&trailer, pack.data_bytes())?;
        self.file.sync_all()?;
        root.sync_dir(root.path())?;
        let after = self.file.metadata()?;
        if after.len() != pack.physical_bytes() { return Err(changed()); }
        self.location.matches(root, &after)?;
        // Pin the original even while opening its immutable, separately held
        // read descriptor. The checked index must come from the actual trailer.
        let held = HeldPack::capture(root, pack)?;
        held.verify_full(root)?;
        Ok(held)
    }
}

pub(in crate::container_storage::ordinary_preparation) struct HeldPack {
    location: Location,
    pack: WorkingPack,
    file: ReadFile,
    stamp: FileStamp,
    index: CheckedIndex,
    _fd: PayloadProofReservation,
}
impl HeldPack {
    pub(in crate::container_storage::ordinary_preparation) fn capture(root: &DiskRoot, pack: WorkingPack) -> Result<Self> {
        let location = Location::capture(root, pack.id())?;
        let fd = descriptor()?;
        let file = root.open_read_entry(&working_pack(pack.id()))?;
        let metadata = file.metadata()?;
        if metadata.dev() != pack.entity().device() || metadata.ino() != pack.entity().inode()
            || metadata.len() != pack.physical_bytes()
        { return Err(changed()); }
        location.matches(root, &metadata)?;
        let stamp = FileStamp::from_metadata(&metadata);
        let mut bytes = vec![0; pack.index_bytes()];
        file.read_exact_at(&mut bytes, pack.data_bytes())?;
        let index = CheckedIndex::from_trailer(pack, bytes).map_err(manifest_error)?;
        let result = Self { location, pack, file, stamp, index, _fd: fd };
        result.validate(root)?;
        Ok(result)
    }
    pub(super) fn verify_full(&self, root: &DiskRoot) -> Result<()> {
        self.validate(root)?;
        let mut buffer = vec![0; self.pack.data_bytes().min(VERIFY_CHUNK) as usize];
        let mut at = 0;
        while at < self.pack.data_bytes() {
            let count = (self.pack.data_bytes() - at).min(VERIFY_CHUNK) as usize;
            self.file.read_exact_at(&mut buffer[..count], at)?;
            verify_chunk(&buffer[..count], self.index.expected_chunk(at / VERIFY_CHUNK))?;
            at += count as u64;
        }
        self.validate(root)
    }
    pub(in crate::container_storage::ordinary_preparation) fn evidence(&self) -> WorkingPack { self.pack }
    pub(in crate::container_storage::ordinary_preparation) fn validate(&self, root: &DiskRoot) -> Result<()> {
        let metadata = self.file.metadata()?;
        if self.stamp != FileStamp::from_metadata(&metadata) { return Err(changed()); }
        self.location.matches(root, &metadata)
    }
    pub(in crate::container_storage::ordinary_preparation) fn read_at(&self, root: &DiskRoot, output: &mut [u8], offset: u64) -> Result<()> {
        self.validate(root)?;
        read_checked(self.pack.data_bytes(), offset, output, |n| self.index.expected_chunk(n),
            |buffer, position| self.file.read_exact_at(buffer, position))?;
        self.validate(root)
    }
}

fn verify_chunk(observed: &[u8], expected: Option<[u8; 32]>) -> Result<()> {
    if expected != Some(*blake3::hash(observed).as_bytes()) { return Err(changed()); }
    Ok(())
}
fn read_checked(
    bytes: u64, offset: u64, output: &mut [u8], expected: impl Fn(u64) -> Option<[u8; 32]>,
    mut read: impl FnMut(&mut [u8], u64) -> std::io::Result<()>,
) -> Result<()> {
    let end = offset.checked_add(output.len() as u64).ok_or_else(changed)?;
    if end > bytes { return Err(changed()); }
    if output.is_empty() { return Ok(()); }
    let mut buffer = vec![0; bytes.min(VERIFY_CHUNK) as usize];
    let mut position = offset;
    while position < end {
        let ordinal = position / VERIFY_CHUNK;
        let start = ordinal * VERIFY_CHUNK;
        let chunk = (bytes - start).min(VERIFY_CHUNK) as usize;
        read(&mut buffer[..chunk], start)?;
        verify_chunk(&buffer[..chunk], expected(ordinal))?;
        let within = (position - start) as usize;
        let take = (chunk - within).min((end - position) as usize);
        let target = (position - offset) as usize;
        output[target..target + take].copy_from_slice(&buffer[within..within + take]);
        position += take as u64;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/container_storage/working_pack.rs"]
mod tests;
