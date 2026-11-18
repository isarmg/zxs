//! Authenticated geometry of one independent working-data pack. Its index is
//! kept in the pack, not expanded into each FILE receipt or into logical runs.
use super::{ManifestError, PackIndexCommitment};
use crate::{FileId, Id, OperationId, file_container::ContainerEntity};
use data_serialization::{Deserialize, Deserializer, Serialize};

pub const VERIFY_CHUNK: u64 = 1024 * 1024;
/// Rollover boundary, not a file or save ceiling.
pub const PACK_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const HASH_BYTES: usize = 32;

pub enum PackKind {}
impl crate::IdKind for PackKind { const PREFIX: &'static str = "wpack"; }
pub type PackId = Id<PackKind>;

/// Original FILE and native save, not a reader-supplied attribution.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct PackOrigin { pub file: FileId, pub operation: OperationId }
impl PackOrigin {
    fn marker(self, id: PackId, bytes: u64) -> [u8; 32] {
        let mut hash = blake3::Hasher::new();
        hash.update(b"zxs:working-pack-origin\0");
        hash.update(self.file.to_string().as_bytes());
        hash.update(self.operation.to_string().as_bytes());
        hash.update(id.to_string().as_bytes());
        hash.update(&bytes.to_le_bytes());
        *hash.finalize().as_bytes()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serialization(deny_unknown_fields)]
pub struct WorkingPack {
    id: PackId,
    origin: PackOrigin,
    entity: ContainerEntity,
    #[serialization(with = "crate::u64_string")]
    data_bytes: u64,
    index: PackIndexCommitment,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct WirePack {
    id: PackId,
    origin: PackOrigin,
    entity: ContainerEntity,
    #[serialization(with = "crate::u64_string")]
    data_bytes: u64,
    index: PackIndexCommitment,
}

impl<'de> Deserialize<'de> for WorkingPack {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = WirePack::deserialize(d)?;
        Self::new(wire.id, wire.origin, wire.entity, wire.data_bytes, wire.index)
            .map_err(data_serialization::de::Error::custom)
    }
}

impl WorkingPack {
    fn new(id: PackId, origin: PackOrigin, entity: ContainerEntity, data_bytes: u64, index: PackIndexCommitment)
        -> Result<Self, ManifestError>
    {
        if entity.inode() == 0 { return Err(ManifestError::Entity); }
        if !(1..=PACK_BYTES).contains(&data_bytes) { return Err(ManifestError::Budget); }
        Ok(Self { id, origin, entity, data_bytes, index })
    }
    pub const fn id(self) -> PackId { self.id }
    pub const fn origin(self) -> PackOrigin { self.origin }
    pub const fn entity(self) -> ContainerEntity { self.entity }
    pub const fn data_bytes(self) -> u64 { self.data_bytes }
    pub fn index_bytes(self) -> usize { (1 + self.data_bytes.div_ceil(VERIFY_CHUNK) as usize) * HASH_BYTES }
    pub fn physical_bytes(self) -> u64 { self.data_bytes + self.index_bytes() as u64 }
    pub fn index_digest(self) -> [u8; 32] { self.index.0 }
}

fn index_hash(id: PackId, bytes: u64, trailer: &[u8]) -> PackIndexCommitment {
    let mut hash = blake3::Hasher::new();
    hash.update(b"zxs:working-pack-checksum-index\0");
    hash.update(id.to_string().as_bytes());
    hash.update(&bytes.to_le_bytes());
    hash.update(trailer);
    PackIndexCommitment(*hash.finalize().as_bytes())
}

/// In-process append proof. Never deserialize a claimed unfinished prefix.
#[derive(Default)]
pub struct AppendIndex {
    bytes: u64,
    tail: blake3::Hasher,
    completed: Vec<u8>,
}
impl AppendIndex {
    pub fn bytes(&self) -> u64 { self.bytes }
    pub fn remaining(&self) -> u64 { PACK_BYTES - self.bytes }
    pub fn append(&mut self, mut input: &[u8]) -> Result<(), ManifestError> {
        if input.len() as u64 > self.remaining() { return Err(ManifestError::Budget); }
        while !input.is_empty() {
            let take = input.len().min((VERIFY_CHUNK - self.bytes % VERIFY_CHUNK) as usize);
            let (part, rest) = input.split_at(take);
            self.tail.update(part);
            self.bytes += take as u64;
            if self.bytes.is_multiple_of(VERIFY_CHUNK) {
                self.completed.extend_from_slice(self.tail.finalize().as_bytes());
                self.tail.reset();
            }
            input = rest;
        }
        Ok(())
    }
    pub fn expected_chunk(&self, ordinal: u64) -> Option<[u8; 32]> {
        if ordinal < self.bytes / VERIFY_CHUNK {
            chunk_hash(&self.completed, ordinal)
        } else if ordinal == self.bytes / VERIFY_CHUNK && !self.bytes.is_multiple_of(VERIFY_CHUNK) {
            Some(*self.tail.finalize().as_bytes())
        } else { None }
    }
    pub fn finish(mut self, id: PackId, origin: PackOrigin, entity: ContainerEntity)
        -> Result<(WorkingPack, Vec<u8>), ManifestError>
    {
        if !self.bytes.is_multiple_of(VERIFY_CHUNK) {
            self.completed.extend_from_slice(self.tail.finalize().as_bytes());
        }
        self.completed.extend_from_slice(&origin.marker(id, self.bytes));
        let seal = WorkingPack::new(id, origin, entity, self.bytes, index_hash(id, self.bytes, &self.completed))?;
        Ok((seal, self.completed))
    }
}

/// The only constructor verifies the bounded trailer against the selected pack.
/// Reading storage can then authenticate just the touched complete chunks.
pub struct CheckedIndex(Vec<u8>);
impl CheckedIndex {
    pub fn from_trailer(pack: WorkingPack, mut trailer: Vec<u8>) -> Result<Self, ManifestError> {
        if trailer.len() != pack.index_bytes()
            || !trailer.ends_with(&pack.origin.marker(pack.id, pack.data_bytes))
            || index_hash(pack.id, pack.data_bytes, &trailer) != pack.index
        { return Err(ManifestError::Coverage); }
        trailer.truncate(trailer.len() - HASH_BYTES);
        Ok(Self(trailer))
    }
    pub fn expected_chunk(&self, ordinal: u64) -> Option<[u8; 32]> {
        chunk_hash(&self.0, ordinal)
    }
}
fn chunk_hash(trailer: &[u8], ordinal: u64) -> Option<[u8; 32]> {
    let start = usize::try_from(ordinal).ok()?.checked_mul(HASH_BYTES)?;
    trailer.get(start..start.checked_add(HASH_BYTES)?)?.try_into().ok()
}

/// Pure admission estimate, including the last partial checksum. Zero means no
/// pack has been started. This is not evidence that a referenced pack exists.
pub fn physical_bytes_for(data: u64) -> Result<u64, ManifestError> {
    if data > PACK_BYTES { return Err(ManifestError::Budget); }
    if data == 0 { return Ok(0); }
    Ok(data + (1 + data.div_ceil(VERIFY_CHUNK)) * HASH_BYTES as u64)
}

#[cfg(test)]
#[path = "../../../../../test/contract/unit/packed_working_material.rs"]
mod tests;
