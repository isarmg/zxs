//! Immutable working material for a container which has no historical node.
//!
//! This value is the working field of the one authoritative container record.
//! It is not a Version, a full-file content hash, or permission to open a path.
use crate::file_container::ContainerEntity;
use crate::{FileId, Generation, GenerationExhausted, Id, OperationId};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::{fmt, str::FromStr};

/// Fixed block geometry is part of this private candidate's format, not an
/// HTTP-frame limit. Transport fragments cannot each become a save generation.
pub const WORKING_BLOCK_BYTES: u64 = 65_536;
pub const MAX_WORKING_BLOCKS: usize = 4096;

pub mod kind {
    pub enum WorkingExtent {}
    impl crate::IdKind for WorkingExtent {
        const PREFIX: &'static str = "wextent";
    }
}
pub type WorkingExtentId = Id<kind::WorkingExtent>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(transparent)]
pub struct SaveGeneration(Generation);

impl SaveGeneration {
    pub const fn first() -> Self {
        Self(Generation::new(1))
    }

    pub const fn generation(self) -> Generation {
        self.0
    }

    pub fn next(self) -> Result<Self, GenerationExhausted> {
        self.0.next().map(Self)
    }
}

/// Hashes exactly one immutable extent, never the visible merged file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExtentHash([u8; 32]);

/// Commits to the complete manifest, including scope, predecessor, truncation
/// and physical extent identities. It cannot be converted into ContentHash.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManifestCommitment([u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, error_definition::Error)]
pub enum ManifestError {
    #[error("working material digest is not canonical lowercase hexadecimal")]
    Digest,
    #[error("working material exceeds its bounded block or length domain")]
    Budget,
    #[error("working material does not have canonical, unique block identities")]
    Blocks,
    #[error("working material lacks an observed immutable extent identity")]
    Entity,
    #[error("working material predecessor or truncation generation is inconsistent")]
    Predecessor,
    #[error("working material replacement does not cover the exact write blocks")]
    Coverage,
    #[error("working material save generation is exhausted")]
    Generation(#[source] GenerationExhausted),
}

fn parse_digest(value: &str) -> Result<[u8; 32], ManifestError> {
    if value.len() != 64
        || !value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ManifestError::Digest);
    }
    let mut bytes = [0; 32];
    for (byte, pair) in bytes.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
        let digit = |value: u8| {
            if value <= b'9' { value - b'0' } else { value - b'a' + 10 }
        };
        *byte = digit(pair[0]) * 16 + digit(pair[1]);
    }
    Ok(bytes)
}

impl ExtentHash {
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub fn digest(self) -> blake3::Hash {
        blake3::Hash::from_bytes(self.0)
    }
}

macro_rules! digest_wire {
    ($kind:ty) => {
        impl FromStr for $kind {
            type Err = ManifestError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                parse_digest(value).map(Self)
            }
        }
        impl fmt::Display for $kind {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(blake3::Hash::from_bytes(self.0).to_hex().as_str())
            }
        }
        impl Serialize for $kind {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }
        impl<'de> Deserialize<'de> for $kind {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                String::deserialize(deserializer)?.parse()
                    .map_err(data_serialization::de::Error::custom)
            }
        }
    };
}
digest_wire!(ExtentHash);
digest_wire!(ManifestCommitment);

/// Serialized feedback is descriptive. The engine must observe this very inode
/// through the admitted container, verify all bytes, then retain the descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct WorkingExtent {
    id: WorkingExtentId,
    entity: ContainerEntity,
    #[serialization(with = "crate::u64_string")]
    bytes: u64,
    hash: ExtentHash,
}

impl WorkingExtent {
    pub fn from_observation(
        id: WorkingExtentId,
        entity: ContainerEntity,
        bytes: u64,
        hash: ExtentHash,
    ) -> Result<Self, ManifestError> {
        let value = Self { id, entity, bytes, hash };
        value.validate()?;
        Ok(value)
    }

    fn validate(self) -> Result<(), ManifestError> {
        if self.entity.inode() == 0 {
            return Err(ManifestError::Entity);
        }
        if self.bytes == 0 || self.bytes > WORKING_BLOCK_BYTES {
            return Err(ManifestError::Blocks);
        }
        Ok(())
    }

    pub const fn id(self) -> WorkingExtentId { self.id }
    pub const fn entity(self) -> ContainerEntity { self.entity }
    pub const fn bytes(self) -> u64 { self.bytes }
    pub const fn hash(self) -> ExtentHash { self.hash }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct WorkingBlock {
    #[serialization(with = "crate::u64_string")]
    index: u64,
    extent: WorkingExtent,
    /// Truncation can retain a prefix of an older extent. Re-extension never
    /// increases this field; the remainder is zero until a new write replaces it.
    #[serialization(with = "crate::u64_string")]
    retained_bytes: u64,
}

impl WorkingBlock {
    pub fn new(index: u64, extent: WorkingExtent) -> Self {
        Self { index, extent, retained_bytes: extent.bytes() }
    }

    pub const fn index(self) -> u64 { self.index }
    pub const fn extent(self) -> WorkingExtent { self.extent }
    pub const fn retained_bytes(self) -> u64 { self.retained_bytes }

    fn start(self) -> Result<u64, ManifestError> {
        self.index.checked_mul(WORKING_BLOCK_BYTES).ok_or(ManifestError::Budget)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManifestPrevious {
    Birth {},
    Saved {
        generation: SaveGeneration,
        commitment: ManifestCommitment,
        #[serialization(with = "crate::u64_string")]
        visible_length: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Truncation {
    generation: SaveGeneration,
    #[serialization(with = "crate::u64_string")]
    old_visible_length: u64,
    #[serialization(with = "crate::u64_string")]
    retained_prefix: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serialization(deny_unknown_fields)]
pub struct UnbornManifest {
    file: FileId,
    operation: OperationId,
    generation: SaveGeneration,
    previous: ManifestPrevious,
    #[serialization(with = "crate::u64_string")]
    logical_size: u64,
    /// Mandatory null means no shrink has occurred. Missing is not null.
    truncation: Option<Truncation>,
    blocks: Vec<WorkingBlock>,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct WireManifest {
    file: FileId,
    operation: OperationId,
    generation: SaveGeneration,
    previous: ManifestPrevious,
    #[serialization(with = "crate::u64_string")]
    logical_size: u64,
    #[serialization(deserialize_with = "required_truncation")]
    truncation: Option<Truncation>,
    #[serialization(deserialize_with = "bounded_blocks")]
    blocks: Vec<WorkingBlock>,
}

fn bounded_blocks<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<WorkingBlock>, D::Error> {
    struct Blocks;
    impl<'de> data_serialization::de::Visitor<'de> for Blocks {
        type Value = Vec<WorkingBlock>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded immutable working-block sequence")
        }

        fn visit_seq<A: data_serialization::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            let mut blocks = Vec::new();
            while let Some(block) = sequence.next_element()? {
                if blocks.len() == MAX_WORKING_BLOCKS {
                    return Err(data_serialization::de::Error::custom(ManifestError::Budget));
                }
                blocks.push(block);
            }
            Ok(blocks)
        }
    }
    deserializer.deserialize_seq(Blocks)
}

fn required_truncation<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Truncation>, D::Error> {
    Option::<Truncation>::deserialize(deserializer)
}

impl<'de> Deserialize<'de> for UnbornManifest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireManifest::deserialize(deserializer)?;
        let value = Self {
            file: wire.file,
            operation: wire.operation,
            generation: wire.generation,
            previous: wire.previous,
            logical_size: wire.logical_size,
            truncation: wire.truncation,
            blocks: wire.blocks,
        };
        value.validate(u64::MAX).map_err(data_serialization::de::Error::custom)?;
        Ok(value)
    }
}

impl UnbornManifest {
    pub fn first(
        file: FileId,
        operation: OperationId,
        logical_size: u64,
        blocks: Vec<WorkingBlock>,
        maximum: u64,
    ) -> Result<Self, ManifestError> {
        let value = Self {
            file,
            operation,
            generation: SaveGeneration::first(),
            previous: ManifestPrevious::Birth {},
            logical_size,
            truncation: None,
            blocks,
        };
        value.validate(maximum)?;
        Ok(value)
    }

    pub fn validate(&self, maximum: u64) -> Result<(), ManifestError> {
        if self.logical_size > maximum || self.blocks.len() > MAX_WORKING_BLOCKS {
            return Err(ManifestError::Budget);
        }
        match self.previous {
            ManifestPrevious::Birth {} if self.generation == SaveGeneration::first() => {}
            ManifestPrevious::Saved { generation, .. }
                if generation.generation().get() != 0
                    && generation.next().map_err(ManifestError::Generation)? == self.generation =>
            {}
            ManifestPrevious::Birth {} | ManifestPrevious::Saved { .. } => {
                return Err(ManifestError::Predecessor);
            }
        }
        if self.truncation.is_some_and(|cut| {
            cut.generation > self.generation || cut.generation.0.get() == 0
                || cut.retained_prefix >= cut.old_visible_length
                || cut.retained_prefix > self.logical_size
        }) {
            return Err(ManifestError::Predecessor);
        }
        let mut identities = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let mut last = None;
        for block in &self.blocks {
            block.extent.validate()?;
            let start = block.start()?;
            if last.is_some_and(|index| index >= block.index)
                || !ids.insert(block.extent.id())
                || !identities.insert(block.extent.entity())
                || start >= self.logical_size
                || block.retained_bytes == 0
                || block.retained_bytes > block.extent.bytes()
                || block.retained_bytes > self.logical_size - start
            {
                return Err(ManifestError::Blocks);
            }
            last = Some(block.index);
        }
        Ok(())
    }

    pub const fn file(&self) -> FileId { self.file }
    pub const fn operation(&self) -> OperationId { self.operation }
    pub const fn generation(&self) -> SaveGeneration { self.generation }
    pub const fn previous(&self) -> ManifestPrevious { self.previous }
    pub const fn logical_size(&self) -> u64 { self.logical_size }
    pub fn blocks(&self) -> &[WorkingBlock] { &self.blocks }

    pub fn block(&self, index: u64) -> Option<WorkingBlock> {
        self.blocks.binary_search_by_key(&index, |block| block.index)
            .ok().map(|position| self.blocks[position])
    }

    /// Retained extents are immutable occurrences, not reusable byte buffers.
    /// A successor may drop or shorten one, but cannot move it to another block,
    /// change its hash or resurrect the prefix removed by an earlier truncate.
    /// New extents still require the engine's actual EXCL/readback product.
    pub fn require_successor_of(
        &self,
        previous: &Self,
        maximum: u64,
    ) -> Result<(), ManifestError> {
        previous.validate(maximum)?;
        self.validate(maximum)?;
        let expected = ManifestPrevious::Saved {
            generation: previous.generation,
            commitment: previous.commitment(),
            visible_length: previous.logical_size,
        };
        if self.file != previous.file || self.operation == previous.operation
            || self.previous != expected
        {
            return Err(ManifestError::Predecessor);
        }
        if self.truncation != previous.truncation
            && !self.truncation.is_some_and(|cut| cut.generation == self.generation)
        {
            return Err(ManifestError::Predecessor);
        }
        let by_id: BTreeMap<_, _> = previous.blocks.iter()
            .map(|block| (block.extent.id(), block)).collect();
        let by_entity: BTreeSet<_> = previous.blocks.iter()
            .map(|block| block.extent.entity()).collect();
        let cut = self.truncation.filter(|cut| cut.generation == self.generation);
        for block in &self.blocks {
            let retained_limit = cut.map_or(block.retained_bytes, |cut| {
                cut.retained_prefix.saturating_sub(block.index * WORKING_BLOCK_BYTES)
            });
            match by_id.get(&block.extent.id()) {
                Some(old) if old.index == block.index && old.extent == block.extent
                    && block.retained_bytes <= old.retained_bytes
                    && block.retained_bytes <= retained_limit => {}
                Some(_) => return Err(ManifestError::Predecessor),
                None if by_entity.contains(&block.extent.entity()) => {
                    return Err(ManifestError::Entity);
                }
                None => {}
            }
        }
        Ok(())
    }

    pub fn commitment(&self) -> ManifestCommitment {
        let mut hash = blake3::Hasher::new();
        hash.update(b"zxs:unborn-working-manifest:v1\0");
        hash.update(self.file.to_string().as_bytes());
        hash.update(self.operation.to_string().as_bytes());
        hash.update(&self.generation.0.get().to_le_bytes());
        match self.previous {
            ManifestPrevious::Birth {} => { hash.update(&[0]); }
            ManifestPrevious::Saved { generation, commitment, visible_length } => {
                hash.update(&[1]);
                hash.update(&generation.0.get().to_le_bytes());
                hash.update(&commitment.0);
                hash.update(&visible_length.to_le_bytes());
            }
        }
        hash.update(&self.logical_size.to_le_bytes());
        match self.truncation {
            None => { hash.update(&[0]); }
            Some(cut) => {
                hash.update(&[1]);
                hash.update(&cut.generation.0.get().to_le_bytes());
                hash.update(&cut.old_visible_length.to_le_bytes());
                hash.update(&cut.retained_prefix.to_le_bytes());
            }
        }
        hash.update(&(self.blocks.len() as u64).to_le_bytes());
        for block in &self.blocks {
            hash.update(&block.index.to_le_bytes());
            hash.update(block.extent.id().to_string().as_bytes());
            hash.update(&block.extent.entity().device().to_le_bytes());
            hash.update(&block.extent.entity().inode().to_le_bytes());
            hash.update(&block.extent.bytes().to_le_bytes());
            hash.update(&block.extent.hash().0);
            hash.update(&block.retained_bytes.to_le_bytes());
        }
        ManifestCommitment(*hash.finalize().as_bytes())
    }

    fn successor(&self, operation: OperationId) -> Result<Self, ManifestError> {
        if operation == self.operation {
            return Err(ManifestError::Predecessor);
        }
        let mut next = self.clone();
        next.operation = operation;
        next.generation = self.generation.next().map_err(ManifestError::Generation)?;
        next.previous = ManifestPrevious::Saved {
            generation: self.generation,
            commitment: self.commitment(),
            visible_length: self.logical_size,
        };
        Ok(next)
    }

    pub fn begin_save(
        &self,
        operation: OperationId,
        maximum: u64,
    ) -> Result<UnbornSave, ManifestError> {
        self.validate(maximum)?;
        Ok(UnbornSave { next: self.successor(operation)?, maximum })
    }
}

/// One private, ordered edit batch. Mutation frames never allocate save
/// generations. Only finish returns the one candidate for a later durable
/// publication; it does not itself issue Saved or release the published base.
pub struct UnbornSave {
    next: UnbornManifest,
    maximum: u64,
}

impl UnbornSave {
    pub fn first(file: FileId, operation: OperationId, maximum: u64) -> Self {
        Self {
            next: UnbornManifest {
                file,
                operation,
                generation: SaveGeneration::first(),
                previous: ManifestPrevious::Birth {},
                logical_size: 0,
                truncation: None,
                blocks: Vec::new(),
            },
            maximum,
        }
    }

    pub fn view(&self) -> &UnbornManifest { &self.next }

    /// Read/modify/write of every affected block is verified by the actual
    /// held-source producer. None means an observed all-zero block, never an
    /// unreadable extent. An error leaves the entire prior edit batch intact.
    pub fn write_blocks(
        &mut self,
        offset: u64,
        accepted: u64,
        replacements: &[(u64, Option<WorkingExtent>)],
    ) -> Result<(), ManifestError> {
        let end = offset.checked_add(accepted).ok_or(ManifestError::Budget)?;
        if accepted == 0 || end > self.maximum || replacements.len() > MAX_WORKING_BLOCKS {
            return Err(ManifestError::Coverage);
        }
        let first = offset / WORKING_BLOCK_BYTES;
        let last = (end - 1) / WORKING_BLOCK_BYTES;
        if last - first >= MAX_WORKING_BLOCKS as u64
            || replacements.len() as u64 != last - first + 1
            || replacements.iter().enumerate().any(|(ordinal, (index, _))| {
                *index != first + ordinal as u64
            })
        {
            return Err(ManifestError::Coverage);
        }
        let mut candidate = self.next.clone();
        candidate.logical_size = candidate.logical_size.max(end);
        candidate.blocks.retain(|block| block.index < first || block.index > last);
        for &(index, extent) in replacements {
            if let Some(extent) = extent {
                let start = index.checked_mul(WORKING_BLOCK_BYTES).ok_or(ManifestError::Budget)?;
                if extent.bytes() != (candidate.logical_size - start).min(WORKING_BLOCK_BYTES) {
                    return Err(ManifestError::Coverage);
                }
                candidate.blocks.push(WorkingBlock::new(index, extent));
            }
        }
        candidate.blocks.sort_by_key(|block| block.index);
        candidate.validate(self.maximum)?;
        self.next = candidate;
        Ok(())
    }

    pub fn resize(&mut self, length: u64) -> Result<(), ManifestError> {
        if length > self.maximum { return Err(ManifestError::Budget); }
        if length == self.next.logical_size { return Ok(()); }
        let mut candidate = self.next.clone();
        if length < candidate.logical_size {
            // Keep the tightest cut in this save. Shrink -> extend -> shrink
            // must not replace an earlier zero-tail obligation with a wider one.
            if !candidate.truncation.is_some_and(|cut| {
                cut.generation == candidate.generation && cut.retained_prefix <= length
            }) {
                candidate.truncation = Some(Truncation {
                    generation: candidate.generation,
                    old_visible_length: candidate.logical_size,
                    retained_prefix: length,
                });
            }
            candidate.blocks.retain_mut(|block| {
                // Every current block already passed validate before this edit.
                let start = block.index * WORKING_BLOCK_BYTES;
                if start >= length { return false; }
                block.retained_bytes = block.retained_bytes.min(length - start);
                true
            });
        }
        candidate.logical_size = length;
        candidate.validate(self.maximum)?;
        self.next = candidate;
        Ok(())
    }

    pub fn finish(self) -> Result<UnbornManifest, ManifestError> {
        self.next.validate(self.maximum)?;
        Ok(self.next)
    }
}
