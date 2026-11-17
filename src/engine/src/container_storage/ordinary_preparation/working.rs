//! Same-container immutable extent observation and bounded merged reads.
//! This module neither installs a candidate nor turns a manifest hash into FULL.
use super::ObservedContainerReplica;
use crate::business::file_draft::DraftError as Failure;
use crate::file_stamp::FileStamp;
use crate::persist::layout;
use crate::{Error, ReadFile, Result};
use std::collections::BTreeMap;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::sync::Arc;
use storage_contract::file_draft::UnbornBody;
use storage_contract::file_draft::manifest::{
    ExtentHash, ManifestCommitment, SaveGeneration, WORKING_BLOCK_BYTES, WorkingBlock,
};
use storage_contract::{ContentHash, FileId};

mod measurement;
pub(crate) use measurement::MeasuredUnbornFull;
mod full;
pub(crate) use full::PreparedUnbornFull;
mod write;
pub(crate) use write::{PreparedWorkingSave, WorkingWriteSession};

fn changed() -> Error {
    Error::FileDraft(Failure::WorkingMaterialChanged)
}

pub(in crate::container_storage::ordinary_preparation) struct HeldExtent {
    pub(in crate::container_storage::ordinary_preparation) block: WorkingBlock,
    file: ReadFile,
    stamp: FileStamp,
}

impl HeldExtent {
    pub(in crate::container_storage::ordinary_preparation) fn capture(
        scope: &ObservedContainerReplica, block: WorkingBlock,
    ) -> Result<Self> {
        let result = Self::observe(scope, block)?;
        let mut buffer = [0; WORKING_BLOCK_BYTES as usize];
        result.read_block(scope, &mut buffer)?;
        Ok(result)
    }

    pub(in crate::container_storage::ordinary_preparation) fn observe(
        scope: &ObservedContainerReplica, block: WorkingBlock,
    ) -> Result<Self> {
        let extent = block.extent();
        let name = layout::container::working_extent(extent.id());
        let file = scope.held.open_read_entry(&name)?;
        let metadata = file.metadata()?;
        let identity = extent.entity();
        if metadata.dev() != identity.device() || metadata.ino() != identity.inode()
            || metadata.nlink() != 1 || metadata.len() != extent.bytes()
        {
            return Err(changed());
        }
        let result = Self { block, file, stamp: FileStamp::from_metadata(&metadata) };
        result.require_named(scope)?;
        Ok(result)
    }

    pub(in crate::container_storage::ordinary_preparation) fn require_named(
        &self, scope: &ObservedContainerReplica,
    ) -> Result<()> {
        self.file.check_running()?;
        let name = layout::container::working_extent(self.block.extent().id());
        let named = scope.held.open_read_entry(&name)?;
        if FileStamp::from_metadata(&self.file.metadata()?) != self.stamp
            || FileStamp::from_metadata(&named.metadata()?) != self.stamp
        {
            return Err(changed());
        }
        Ok(())
    }

    pub(in crate::container_storage::ordinary_preparation) fn read_block(
        &self,
        scope: &ObservedContainerReplica,
        buffer: &mut [u8; WORKING_BLOCK_BYTES as usize],
    ) -> Result<()> {
        self.require_named(scope)?;
        buffer.fill(0);
        let bytes = self.block.extent().bytes() as usize;
        // The fixed block bound was admitted before allocating or opening any
        // extents. FileExt retains the real disk lease for every physical read.
        self.file.read_exact_at(&mut buffer[..bytes], 0)?;
        if ExtentHash::of_bytes(&buffer[..bytes]) != self.block.extent().hash() {
            return Err(changed());
        }
        buffer[self.block.retained_bytes() as usize..].fill(0);
        self.require_named(scope)
    }
}

/// The exact immutable body retained by both publication and subsequent reads.
/// It does not own a container Arc, so attaching it to that occurrence has no cycle.
pub(super) struct HeldUnbornMaterial {
    body: UnbornBody,
    extents: BTreeMap<u64, HeldExtent>,
    _reservation: crate::engine::PayloadProofReservation,
}

impl HeldUnbornMaterial {
    pub(super) fn capture(scope: &ObservedContainerReplica, maximum: u64) -> Result<Self> {
        scope.verify_entity()?;
        scope.parent.current_zxs()?;
        let working = scope.record.working.unborn()
            .ok_or(Error::FileDraft(Failure::UnbornRequired))?;
        if scope.record.replicas.len() != 1
            || scope.disk.seal.disk_id.parse::<storage_contract::DiskId>()?
                != scope.record.owner_disk
        {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        let body = working.body().clone();
        let blocks = match &body {
            UnbornBody::Empty {} => &[][..],
            UnbornBody::Saved { manifest } => {
                manifest.validate(maximum).map_err(|source| {
                    Error::FileDraft(Failure::WorkingManifest(source))
                })?;
                if manifest.file() != scope.file { return Err(changed()); }
                manifest.blocks()
            }
        };
        let count = blocks.len().checked_add(1)
            .ok_or(Error::FileDraft(Failure::Size))?;
        let reservation = crate::engine::payload_proof_budget().reserve(count)
            .ok_or(Error::FileDraft(Failure::Size))?;
        let mut extents = BTreeMap::new();
        for &block in blocks {
            extents.insert(block.index(), HeldExtent::capture(scope, block)?);
        }
        let result = Self { body, extents, _reservation: reservation };
        result.validate(scope)?;
        scope.verify_entity()?;
        Ok(result)
    }

    pub(super) fn validate(&self, scope: &ObservedContainerReplica) -> Result<()> {
        if scope.record.working.unborn().map(|working| working.body()) != Some(&self.body) {
            return Err(changed());
        }
        for extent in self.extents.values() {
            extent.require_named(scope)?;
        }
        Ok(())
    }
}

pub(super) fn populate(
    held: &mut BTreeMap<(FileId, storage_contract::DiskId), ObservedContainerReplica>,
    maximum: u64,
) -> Result<()> {
    for container in held.values_mut() {
        if container.record.working.unborn().is_some() {
            container.unborn = Some(Arc::new(HeldUnbornMaterial::capture(container, maximum)?));
        }
    }
    Ok(())
}

/// Captures one immutable save, not today's mutable path or a new authorization.
/// A later read shares the original observed extents and their descriptor budget.
pub(crate) struct HeldUnbornReader {
    scope: Arc<ObservedContainerReplica>,
    material: Arc<HeldUnbornMaterial>,
}

impl HeldUnbornReader {
    pub(super) fn capture(scope: Arc<ObservedContainerReplica>, maximum: u64) -> Result<Self> {
        scope.verify()?;
        let material = Arc::clone(scope.unborn.as_ref()
            .ok_or(Error::FileDraft(Failure::UnbornRequired))?);
        if let UnbornBody::Saved { manifest } = &material.body {
            manifest.validate(maximum).map_err(|source| {
                Error::FileDraft(Failure::WorkingManifest(source))
            })?;
        }
        let result = Self { scope, material };
        result.require_source()?;
        Ok(result)
    }

    pub(crate) fn file(&self) -> FileId { self.scope.file }

    pub(crate) fn logical_size(&self) -> u64 {
        match &self.material.body {
            UnbornBody::Empty {} => 0,
            UnbornBody::Saved { manifest } => manifest.logical_size(),
        }
    }

    pub(crate) fn generation(&self) -> Option<SaveGeneration> {
        match &self.material.body {
            UnbornBody::Empty {} => None,
            UnbornBody::Saved { manifest } => Some(manifest.generation()),
        }
    }

    pub(crate) fn commitment(&self) -> Option<ManifestCommitment> {
        match &self.material.body {
            UnbornBody::Empty {} => None,
            UnbornBody::Saved { manifest } => Some(manifest.commitment()),
        }
    }

    pub(crate) fn require_source(&self) -> Result<()> {
        self.scope.verify()?;
        if self.scope.record.working.unborn().map(|working| working.body())
            != Some(&self.material.body)
        {
            return Err(changed());
        }
        Ok(())
    }

    pub(crate) fn require_record(
        &self,
        file: FileId,
        record: &storage_contract::file_container::FileContainerRecord,
    ) -> Result<()> {
        if self.scope.file != file || &self.scope.record != record {
            return Err(changed());
        }
        self.require_source()
    }

    /// A bounded range read returns only bytes verified through the exact held
    /// block. It never exposes a sparse OS FD whose holes pretend to be a base.
    pub(crate) fn read_at(&self, output: &mut [u8], offset: u64) -> Result<usize> {
        self.require_source()?;
        let length = self.logical_size();
        if output.is_empty() || offset >= length { return Ok(0); }
        let count = (length - offset).min(output.len() as u64) as usize;
        let mut block = [0; WORKING_BLOCK_BYTES as usize];
        let mut written = 0;
        while written < count {
            let position = offset + written as u64;
            let index = position / WORKING_BLOCK_BYTES;
            let within = (position % WORKING_BLOCK_BYTES) as usize;
            let take = (count - written).min(block.len() - within);
            match self.material.extents.get(&index) {
                Some(extent) => extent.read_block(&self.scope, &mut block)?,
                None => block.fill(0),
            }
            output[written..written + take].copy_from_slice(&block[within..within + take]);
            written += take;
        }
        self.require_source()?;
        Ok(count)
    }

    /// Only a full actual merged read can produce this content digest. It still
    /// proves no independent FULL installation and no historical publication.
    pub(crate) fn full_digest(&self) -> Result<ReadWorkingDigest<'_>> {
        let mut hash = blake3::Hasher::new();
        let mut block = [0; WORKING_BLOCK_BYTES as usize];
        let mut offset = 0;
        while offset < self.logical_size() {
            let read = self.read_at(&mut block, offset)?;
            hash.update(&block[..read]);
            offset += read as u64;
        }
        self.require_source()?;
        Ok(ReadWorkingDigest { source: self, hash: ContentHash::from_digest(hash.finalize()) })
    }
}

pub(crate) struct ReadWorkingDigest<'a> {
    source: &'a HeldUnbornReader,
    hash: ContentHash,
}

impl ReadWorkingDigest<'_> {
    pub(crate) fn source(&self) -> &HeldUnbornReader { self.source }
    pub(crate) fn content_hash(&self) -> ContentHash { self.hash }
    pub(crate) fn require_source(&self) -> Result<()> { self.source.require_source() }
}
