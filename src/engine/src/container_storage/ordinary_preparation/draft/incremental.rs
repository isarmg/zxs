//! A held immutable version plus independently authenticated packed ranges.
use crate::business::file_draft::DraftError as Failure;
use crate::container_storage::ordinary_preparation::ObservedContainerReplica;
use crate::container_storage::ordinary_preparation::draft::HeldDraft;
use crate::container_storage::ordinary_preparation::working::{HeldPack, held_packs};
use crate::engine::PayloadProofReservation;
use crate::file_stamp::FileStamp;
use crate::{Error, FileVersion, ReadFile, Result, State};
use std::collections::BTreeMap;
use std::os::unix::fs::FileExt;
use std::sync::Arc;
use storage_contract::file_draft::manifest::{packed::PackId, ranges::ReadSpan};
use storage_contract::file_draft::{DraftBase, FileWorkingState, IncrementalManifest};
use storage_contract::FileId;

fn changed() -> Error {
    Error::FileDraft(Failure::WorkingMaterialChanged)
}

pub(in crate::container_storage::ordinary_preparation) struct HeldManualMaterial {
    working: FileWorkingState,
    base: DraftBase,
    version: FileVersion,
    file: ReadFile,
    checksums: storage_contract::ChecksumManifest,
    stamp: FileStamp,
    packs: BTreeMap<PackId, HeldPack>,
    _reservation: PayloadProofReservation,
}

impl HeldManualMaterial {
    pub(super) fn capture(scope: &ObservedContainerReplica, state: &State) -> Result<Self> {
        let (base, _) = scope.record.working.draft().ok_or_else(changed)?;
        let manifest = scope.record.working.incremental().ok_or_else(changed)?;
        let version = state
            .versions
            .get(&base.version_id().to_string())
            .ok_or_else(changed)?;
        Self::from_version(scope, *base, version, Some(manifest))
    }

    fn from_version(
        scope: &ObservedContainerReplica,
        base: DraftBase,
        version: &FileVersion,
        manifest: Option<&IncrementalManifest>,
    ) -> Result<Self> {
        scope.verify_entity()?;
        scope.parent.current_zxs()?;
        if version.version_id != base.version_id().to_string()
            || version.file_id != scope.file.to_string()
            || version.history_tree_id != scope.record.history_tree_id.to_string()
        {
            return Err(changed());
        }
        let packs = manifest.map_or(&[][..], |m| m.material().packs());
        let reservation = crate::engine::payload_proof_budget().reserve(1)
            .ok_or(Error::FileDraft(Failure::Size))?;
        let path = scope.body_path_for_material(version)?;
        // The authoritative version metadata commits to this existing sidecar.
        // Opening a working draft does not authenticate the entire base body.
        let root = scope.parent.current_zxs()?;
        let directory = root.path().join(crate::persist::layout::source_version(
            scope.record.history_tree_id, base.version_id(),
        ));
        let bytes = crate::disk::read_authenticated_checksums_at(root, &directory, version)?;
        let checksums = crate::disk::decode_authenticated_checksums(&bytes, version)?;
        let file = ReadFile::from(scope.held.open_leased_regular(&path)?);
        if file.logical_size()? != version.logical_size { return Err(changed()); }
        let stamp = FileStamp::from_metadata(&file.metadata()?);
        let packs = held_packs(scope, packs, false)?;
        let result = Self {
            working: scope.record.working.clone(),
            base,
            version: version.clone(),
            file,
            checksums,
            stamp,
            packs,
            _reservation: reservation,
        };
        result.validate(scope)?;
        Ok(result)
    }

    pub(super) fn validate(&self, scope: &ObservedContainerReplica) -> Result<()> {
        if self.working != scope.record.working {
            return Err(changed());
        }
        self.file.check_running()?;
        let path = scope.body_path_for_material(&self.version)?;
        let named = scope.held.open_leased_regular(&path)?;
        if FileStamp::from_metadata(&self.file.metadata()?) != self.stamp
            || FileStamp::from_metadata(&named.metadata()?) != self.stamp
        {
            return Err(changed());
        }
        for held in self.packs.values() {
            held.validate(&scope.held)?;
        }
        Ok(())
    }

    fn base_range(&self, offset: u64, retained: u64, output: &mut [u8]) -> Result<()> {
        // A gap spans the bound old base only as far as its retained prefix;
        // the rest is sparse zero even after a later re-extension.
        output.fill(0);
        let count = retained.min(self.version.logical_size).saturating_sub(offset)
            .min(output.len() as u64) as usize;
        let chunk_bytes = u64::from(self.checksums.chunk_bytes);
        if chunk_bytes == 0 { return Err(changed()); }
        let mut copied = 0;
        while copied < count {
            let at = offset + copied as u64;
            let ordinal = at / chunk_bytes;
            let start = ordinal * chunk_bytes;
            let len = (self.version.logical_size - start).min(chunk_bytes) as usize;
            let expected = self.checksums.chunks.get(usize::try_from(ordinal).map_err(|_| changed())?)
                .ok_or_else(changed)?;
            let verified = platform_io::payload::read_verified_chunk(len, blake3::Hash::from_bytes(*expected),
                |bytes| self.file.read_exact_at(bytes, start))?;
            let within = (at - start) as usize;
            let take = (count - copied).min(len - within);
            output[copied..copied + take].copy_from_slice(&verified[within..within + take]);
            copied += take;
        }
        Ok(())
    }

    pub(super) fn read_at(&self, scope: &ObservedContainerReplica, output: &mut [u8], offset: u64) -> Result<usize> {
        self.validate(scope)?;
        let manifest = self.working.incremental();
        let length = manifest.map_or(self.version.logical_size, |m| m.material().logical_size());
        let retained = manifest.map_or(self.version.logical_size, |m| m.retained_base_bytes());
        let count = length.saturating_sub(offset).min(output.len() as u64) as usize;
        let mut copied = 0;
        while copied < count {
            let at = offset + copied as u64;
            let (span, bytes) = match manifest {
                None => (ReadSpan::Gap, (count - copied) as u64),
                Some(m) => m.material().span(at, offset + count as u64)
                    .map_err(|e| Error::FileDraft(Failure::WorkingManifest(e)))?,
            };
            let end = copied + bytes as usize;
            match span {
                ReadSpan::Zero => output[copied..end].fill(0),
                ReadSpan::Gap => self.base_range(at, retained, &mut output[copied..end])?,
                ReadSpan::Data { pack, offset } => self.packs.get(&pack).ok_or_else(changed)?
                    .read_at(&scope.held, &mut output[copied..end], offset)?,
            }
            copied = end;
        }
        self.validate(scope)?;
        Ok(count)
    }

}

pub(crate) struct HeldManualReader {
    scope: Arc<ObservedContainerReplica>,
    material: Arc<HeldManualMaterial>,
}

impl HeldManualReader {
    pub(crate) fn capture(
        scope: Arc<ObservedContainerReplica>,
        base: DraftBase,
        version: &FileVersion,
    ) -> Result<Self> {
        scope.verify()?;
        let material = match &scope.draft {
            Some(HeldDraft::Incremental(material)) => Arc::clone(material),
            None if matches!(scope.record.working, FileWorkingState::ManualClean { .. }) => {
                Arc::new(HeldManualMaterial::from_version(
                    &scope, base, version, None,
                )?)
            }
            Some(HeldDraft::Full(_)) | None => {
                return Err(Error::FileDraft(Failure::UnsupportedScope));
            }
        };
        if material.base != base || &material.version != version {
            return Err(changed());
        }
        Ok(Self { scope, material })
    }

    pub(crate) fn scope(&self) -> &Arc<ObservedContainerReplica> {
        &self.scope
    }
    pub(crate) fn base(&self) -> DraftBase {
        self.material.base
    }
    pub(crate) fn logical_size(&self) -> u64 {
        self.material
            .working
            .incremental()
            .map_or(self.material.version.logical_size, |manifest| {
                manifest.material().logical_size()
            })
    }
    pub(crate) fn retained_base_bytes(&self) -> u64 {
        self.material
            .working
            .incremental()
            .map_or(self.material.version.logical_size, |manifest| {
                manifest.retained_base_bytes()
            })
    }
    pub(crate) fn manifest(&self) -> Option<&IncrementalManifest> {
        self.material.working.incremental()
    }
    pub(crate) fn require_source(&self) -> Result<()> {
        self.scope.verify()?;
        self.material.validate(&self.scope)
    }
    pub(crate) fn require_record(
        &self,
        file: FileId,
        record: &storage_contract::file_container::FileContainerRecord,
    ) -> Result<()> {
        if file != self.scope.file || record != &self.scope.record {
            return Err(changed());
        }
        self.require_source()
    }
    pub(crate) fn read_at(&self, output: &mut [u8], offset: u64) -> Result<usize> {
        self.require_source()?;
        self.material.read_at(&self.scope, output, offset)
    }
    pub(in crate::container_storage::ordinary_preparation) fn base_range(
        &self, offset: u64, retained: u64, output: &mut [u8],
    ) -> Result<()> {
        self.require_source()?;
        self.material.base_range(offset, retained, output)?;
        self.require_source()
    }
    pub(in crate::container_storage::ordinary_preparation) fn pack(&self, id: PackId) -> Result<&HeldPack> {
        self.material.packs.get(&id).ok_or_else(changed)
    }

}

/// Only an explicit commit reads the complete merged body. This non-serializable
/// proof retains that very old draft and its source occurrence for live/cold replay.
pub(crate) struct ReadDraftDigest {
    source: Arc<HeldManualReader>,
    payload: storage_contract::file_draft::DraftPayload,
    hash: storage_contract::ContentHash,
    checksums: storage_contract::ChecksumManifest,
}

impl ReadDraftDigest {
    pub(crate) fn capture(source: Arc<HeldManualReader>, chunk_bytes: u32) -> Result<Arc<Self>> {
        source.require_source()?;
        let payload = source.scope.incremental_identity()?;
        let mut full = blake3::Hasher::new();
        let mut chunks = Vec::new();
        let mut buffer = vec![0; chunk_bytes as usize];
        let mut offset = 0;
        while offset < payload.logical_size() {
            let count = source.read_at(&mut buffer, offset)?;
            if count == 0 { return Err(changed()); }
            full.update(&buffer[..count]);
            chunks.push(*blake3::hash(&buffer[..count]).as_bytes());
            offset += count as u64;
        }
        source.require_source()?;
        Ok(Arc::new(Self {
            source, payload, hash: storage_contract::ContentHash::from_digest(full.finalize()),
            checksums: storage_contract::ChecksumManifest {
                chunk_bytes, logical_size: payload.logical_size(), chunks,
            },
        }))
    }

    pub(crate) fn content_hash(&self) -> storage_contract::ContentHash { self.hash }
    pub(crate) fn checksums(&self) -> &storage_contract::ChecksumManifest { &self.checksums }
    pub(crate) fn file(&self) -> FileId { self.source.scope.file }
    pub(crate) fn owner(&self) -> storage_contract::DiskId { self.source.scope.record.owner_disk }
    pub(crate) fn payload(&self) -> storage_contract::file_draft::DraftPayload { self.payload }
    pub(crate) fn require_source(&self) -> Result<()> { self.source.require_source() }
    pub(crate) fn require_record(
        &self, file: FileId, record: &storage_contract::file_container::FileContainerRecord,
    ) -> Result<()> {
        if record.working.draft().map(|(_, payload)| *payload) != Some(self.payload) {
            return Err(changed());
        }
        self.source.require_record(file, record)
    }
}
