//! Immutable, bounded packed ranges for an unpublished body or an overlay on one exact base.
//!
//! This value is the working field of the one authoritative container record.
//! It is not a Version, a full-file content hash, or permission to open a path.
pub mod packed;
pub mod ranges;
use packed::WorkingPack;
use ranges::{LogicalRun, MAX_LIVE_PACKS, MAX_LIVE_RUNS, RunSource, RunTable, ReadSpan};

use crate::{FileId, Generation, GenerationExhausted, OperationId};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::{fmt, str::FromStr};

/// Bound on a transport edit/read fragment; not a disk allocation or logical-file limit.
pub const WORKING_IO_BYTES: u64 = 65_536;

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

/// Commitment to one private pack index; not a hash of the merged file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackIndexCommitment([u8; 32]);

/// Commits to the complete manifest, including scope, predecessor, truncation
/// and physical extent identities. It cannot be converted into ContentHash.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManifestCommitment([u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, error_definition::Error)]
pub enum ManifestError {
    #[error("working material digest is not canonical lowercase hexadecimal")]
    Digest,
    #[error("working material exceeds its bounded pack, interval or length domain")]
    Budget,
    #[error("working material lacks an observed immutable extent identity")]
    Entity,
    #[error("working material predecessor or truncation generation is inconsistent")]
    Predecessor,
    #[error("working material logical range or authenticated pack index is inconsistent")]
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
digest_wire!(PackIndexCommitment);
digest_wire!(ManifestCommitment);

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
pub struct WorkingManifest {
    file: FileId,
    operation: OperationId,
    generation: SaveGeneration,
    previous: ManifestPrevious,
    #[serialization(with = "crate::u64_string")]
    logical_size: u64,
    /// Mandatory null means no shrink has occurred. Missing is not null.
    truncation: Option<Truncation>,
    pack_list: Vec<WorkingPack>,
    runs: RunTable,
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
    #[serialization(deserialize_with = "bounded_packs")]
    pack_list: Vec<WorkingPack>,
    #[serialization(deserialize_with = "bounded_runs")]
    runs: Vec<LogicalRun>,
}
fn bounded_packs<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<WorkingPack>, D::Error> {
    ranges::bounded::<D, WorkingPack, MAX_LIVE_PACKS>(d)
}
fn bounded_runs<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<LogicalRun>, D::Error> {
    ranges::bounded::<D, LogicalRun, MAX_LIVE_RUNS>(d)
}

fn required_truncation<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Truncation>, D::Error> {
    Option::<Truncation>::deserialize(deserializer)
}

impl<'de> Deserialize<'de> for WorkingManifest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireManifest::deserialize(deserializer)?;
        let value = Self {
            file: wire.file,
            operation: wire.operation,
            generation: wire.generation,
            previous: wire.previous,
            logical_size: wire.logical_size,
            truncation: wire.truncation,
            pack_list: wire.pack_list,
            runs: RunTable::from_wire(wire.runs),
        };
        value.validate(u64::MAX).map_err(data_serialization::de::Error::custom)?;
        Ok(value)
    }
}

impl WorkingManifest {
    pub fn validate(&self, maximum: u64) -> Result<(), ManifestError> {
        if self.logical_size > maximum {
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
        if self.pack_list.iter().any(|p| p.origin().file != self.file
            || (matches!(self.previous, ManifestPrevious::Birth {}) && p.origin().operation != self.operation)) {
            return Err(ManifestError::Predecessor);
        }
        self.runs.validate(&self.pack_list, self.logical_size)
    }

    pub const fn file(&self) -> FileId { self.file }
    pub const fn operation(&self) -> OperationId { self.operation }
    pub const fn generation(&self) -> SaveGeneration { self.generation }
    pub const fn previous(&self) -> ManifestPrevious { self.previous }
    pub const fn logical_size(&self) -> u64 { self.logical_size }
    pub fn packs(&self) -> &[WorkingPack] { &self.pack_list }
    pub fn runs(&self) -> &[LogicalRun] { self.runs.runs() }
    pub fn allocated_bytes(&self) -> u64 { self.pack_list.iter().map(|p| p.physical_bytes()).sum() }
    pub fn span(&self, start: u64, end: u64) -> Result<(ReadSpan, u64), ManifestError> {
        if end > self.logical_size { return Err(ManifestError::Coverage); }
        self.runs.at(start, end)
    }

    pub fn retained_prefix(&self) -> Option<u64> {
        self.truncation.map(|cut| cut.retained_prefix)
    }

    /// A retained physical slice cannot move, grow past a cut, change identity
    /// or disappear back into a gap. New packs belong to this exact Save.
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
        let old: BTreeMap<_, _> = previous.packs().iter().map(|p| (p.id(), p)).collect();
        let entities: BTreeSet<_> = previous.packs().iter().map(|p| p.entity()).collect();
        for pack in self.packs() {
            match old.get(&pack.id()) {
                Some(&original) if original == pack => (),
                Some(_) => return Err(ManifestError::Predecessor),
                None if entities.contains(&pack.entity()) || pack.origin().operation != self.operation => {
                    return Err(ManifestError::Predecessor);
                }
                None => (),
            }
        }
        let cut = self.truncation.filter(|cut| cut.generation == self.generation);
        if self.logical_size < previous.logical_size && cut.is_none() { return Err(ManifestError::Predecessor); }
        self.runs.retains(&previous.runs, &old.keys().copied().collect(), cut.map(|c| c.retained_prefix))
    }

    pub fn commitment(&self) -> ManifestCommitment {
        let mut hash = blake3::Hasher::new();
        hash.update(b"zxs:packed-working-manifest\0");
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
        hash.update(&(self.pack_list.len() as u64).to_le_bytes());
        for &pack in &self.pack_list {
            hash.update(pack.id().to_string().as_bytes());
            hash.update(pack.origin().file.to_string().as_bytes());
            hash.update(pack.origin().operation.to_string().as_bytes());
            hash.update(&pack.entity().device().to_le_bytes());
            hash.update(&pack.entity().inode().to_le_bytes());
            hash.update(&pack.data_bytes().to_le_bytes());
            hash.update(&pack.index_digest());
        }
        hash.update(&(self.runs().len() as u64).to_le_bytes());
        for &run in self.runs() {
            hash.update(&run.start().to_le_bytes());
            hash.update(&run.bytes().to_le_bytes());
            match run.source() {
                RunSource::Zero {} => { hash.update(&[0]); }
                RunSource::Data { pack, offset } => {
                    hash.update(&[1]); hash.update(pack.to_string().as_bytes()); hash.update(&offset.to_le_bytes());
                }
            }
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
    ) -> Result<WorkingSave, ManifestError> {
        self.validate(maximum)?;
        Ok(WorkingSave { next: self.successor(operation)?, maximum })
    }
}

/// One private, ordered edit batch. Mutation frames never allocate save
/// generations. Only finish returns the one candidate for a later durable
/// publication; it does not itself issue Saved or release the published base.
pub struct WorkingSave {
    next: WorkingManifest,
    maximum: u64,
}

impl WorkingSave {
    pub fn first(file: FileId, operation: OperationId, maximum: u64) -> Self {
        Self {
            next: WorkingManifest {
                file,
                operation,
                generation: SaveGeneration::first(),
                previous: ManifestPrevious::Birth {},
                logical_size: 0,
                truncation: None,
                pack_list: Vec::new(),
                runs: RunTable::default(),
            },
            maximum,
        }
    }

    pub fn first_with_length(
        file: FileId, operation: OperationId, length: u64, maximum: u64,
    ) -> Result<Self, ManifestError> {
        let mut value = Self::first(file, operation, maximum);
        value.resize(length)?;
        Ok(value)
    }

    pub fn continues_data(&self, at: u64, pack: packed::PackId, physical: u64) -> bool {
        self.next.runs.continues_data(at, pack, physical)
    }
    pub fn maximum(&self) -> u64 { self.maximum }
    pub fn logical_size(&self) -> u64 { self.next.logical_size }
    pub fn span(&self, start: u64, end: u64) -> Result<(ReadSpan, u64), ManifestError> {
        self.next.span(start, end)
    }
    pub fn write(&mut self, start: u64, bytes: u64, source: RunSource) -> Result<(), ManifestError> {
        let end = start.checked_add(bytes).ok_or(ManifestError::Budget)?;
        if end > self.maximum { return Err(ManifestError::Budget); }
        self.next.runs.replace(start, bytes, source)?;
        self.next.logical_size = self.next.logical_size.max(end);
        Ok(())
    }
    pub fn references(&self) -> BTreeSet<packed::PackId> { self.next.runs.references() }
    pub fn seal_pack(&mut self, pack: WorkingPack) -> Result<(), ManifestError> {
        if pack.origin().file != self.next.file || pack.origin().operation != self.next.operation {
            return Err(ManifestError::Predecessor);
        }
        match self.next.pack_list.binary_search_by_key(&pack.id(), |p| p.id()) {
            Ok(_) => Err(ManifestError::Entity),
            Err(index) => {
                // At most the old retained set plus the selected new set can be pending final pruning.
                if self.next.pack_list.len() == 2 * MAX_LIVE_PACKS { return Err(ManifestError::Budget); }
                self.next.pack_list.insert(index, pack); Ok(())
            }
        }
    }

    pub fn resize(&mut self, length: u64) -> Result<(), ManifestError> {
        if length > self.maximum { return Err(ManifestError::Budget); }
        if length == self.next.logical_size { return Ok(()); }
        if length < self.next.logical_size {
            if !self.next.truncation.is_some_and(|cut| {
                cut.generation == self.next.generation && cut.retained_prefix <= length
            }) {
                self.next.truncation = Some(Truncation { generation: self.next.generation,
                    old_visible_length: self.next.logical_size, retained_prefix: length });
            }
            self.next.runs.truncate(length);
        }
        self.next.logical_size = length;
        Ok(())
    }
    pub fn finish(mut self) -> Result<WorkingManifest, ManifestError> {
        let referenced = self.references();
        self.next.pack_list.retain(|pack| referenced.contains(&pack.id()));
        self.next.validate(self.maximum)?;
        Ok(self.next)
    }
}
