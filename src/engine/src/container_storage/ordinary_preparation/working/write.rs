//! Private ordered edits over one published held source. Exclusive immutable
//! extents may be prepared here; only the native publication can acknowledge a save.
use crate::business::file_draft::DraftError as Failure;
use crate::container_storage::ordinary_preparation::working::{
    HeldExtent, HeldUnbornReader, changed,
};
use crate::disk::{DiskResources, NewEntryCopyError};
use crate::engine::{PayloadProofReservation, PlanningContext};
use crate::persist::layout;
use crate::{Error, Result, State, ValidatedConfig};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;
use storage_contract::file_container::ContainerEntity;
use storage_contract::file_draft::manifest::{
    ExtentHash, MAX_WORKING_BLOCKS, WorkingManifest, WorkingSave, WORKING_BLOCK_BYTES,
    WorkingBlock, WorkingExtent, WorkingExtentId,
};
use storage_contract::file_draft::UnbornBody;
use storage_contract::{OperationId};
use crate::container_storage::HeldManualReader;
use storage_contract::file_draft::{DraftPayload, DraftPayloadId, IncrementalManifest};

enum SourceReader {
    Unborn(HeldUnbornReader),
    Manual(HeldManualReader),
}

impl SourceReader {
    fn scope(&self) -> &Arc<crate::container_storage::ObservedContainerReplica> {
        match self {
            Self::Unborn(reader) => &reader.scope,
            Self::Manual(reader) => reader.scope(),
        }
    }
    fn file(&self) -> storage_contract::FileId { self.scope().file }
    fn require_source(&self) -> Result<()> {
        match self {
            Self::Unborn(reader) => reader.require_source(),
            Self::Manual(reader) => reader.require_source(),
        }
    }
    fn manifest(&self) -> Option<&WorkingManifest> {
        match self {
            Self::Unborn(reader) => match &reader.material.body {
                UnbornBody::Empty {} => None,
                UnbornBody::Saved { manifest } => Some(manifest),
            },
            Self::Manual(reader) => reader.manifest().map(|manifest| manifest.material()),
        }
    }
    fn logical_size(&self) -> u64 {
        match self {
            Self::Unborn(reader) => reader.logical_size(),
            Self::Manual(reader) => reader.logical_size(),
        }
    }
    fn retained_base_bytes(&self) -> u64 {
        match self {
            Self::Unborn(_) => 0,
            Self::Manual(reader) => reader.retained_base_bytes(),
        }
    }
    fn extent(&self, block: WorkingBlock) -> Result<&HeldExtent> {
        match self {
            Self::Unborn(reader) => reader.material.extents.get(&block.index())
                .ok_or_else(changed),
            Self::Manual(reader) => reader.extent(block),
        }
    }
    fn base_block(
        &self, index: u64, retained: u64, buffer: &mut [u8; WORKING_BLOCK_BYTES as usize],
    ) -> Result<()> {
        match self {
            Self::Unborn(_) => { buffer.fill(0); Ok(()) }
            Self::Manual(reader) => reader.base_block(index, retained, buffer),
        }
    }
}


struct Source {
    state: Arc<State>,
    config: ValidatedConfig,
    resources: Arc<DiskResources>,
    snapshot: Option<String>,
    reader: SourceReader,
}

impl Source {
    fn capture(context: &PlanningContext<'_>, reader: SourceReader) -> Result<Self> {
        let source = Self {
            state: Arc::clone(context.state),
            config: context.config.clone(),
            resources: Arc::clone(context.disk_resources),
            snapshot: context.snapshot.clone(),
            reader,
        };
        source.require_context(context)?;
        Ok(source)
    }

    fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        context.require_write()?;
        if context.current_namespaces.is_some() || context.config.generation().is_some() {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        if !Arc::ptr_eq(&self.state, context.state)
            || !self.config.same_admission(context.config)
            || !Arc::ptr_eq(&self.resources, context.disk_resources)
            || self.snapshot != *context.snapshot
            || context.state.file_containers.get(&self.reader.file())
                != Some(&self.reader.scope().record)
        {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        let disk = context.config.disks.iter().find(|disk| {
            disk.disk_id == self.reader.scope().disk.seal.disk_id
        }).ok_or(Error::FileDraft(Failure::SourceChanged))?;
        if !Arc::ptr_eq(context.disk_resources.get(disk)?, &self.reader.scope().disk) {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        self.reader.require_source()
    }
}

struct CreatedExtent {
    held: HeldExtent,
    // Do not release accounting while the actual inode-preventing descriptor lives.
    _reservation: PayloadProofReservation,
}

enum Editing {
    Active(WorkingSave),
    Failed,
}

/// Connection-local edit state, not a durable accepted-write log. A failed frame
/// poisons this batch and cannot later be flushed as if its partial copies had
/// succeeded. Unrecorded extents are preserved, never adopted or auto-cleaned.
pub(crate) struct WorkingWriteSession {
    source: Source,
    operation: OperationId,
    editing: Editing,
    created: BTreeMap<WorkingExtentId, CreatedExtent>,
    changed: bool,
    retained_base_bytes: u64,
}

fn manifest_error(source: storage_contract::file_draft::manifest::ManifestError) -> Error {
    Error::FileDraft(Failure::WorkingManifest(source))
}

impl WorkingWriteSession {
    pub(crate) fn begin(
        context: &PlanningContext<'_>,
        reader: HeldUnbornReader,
        operation: OperationId,
    ) -> Result<Self> {
        Self::begin_source(
            context, SourceReader::Unborn(reader), operation, context.config.max_file_bytes,
        )
    }

    pub(crate) fn begin_manual(
        context: &PlanningContext<'_>, reader: HeldManualReader, operation: OperationId,
    ) -> Result<Self> {
        Self::begin_source(
            context, SourceReader::Manual(reader), operation, context.config.max_file_bytes,
        )
    }

    pub(crate) fn begin_bounded(
        context: &PlanningContext<'_>, reader: HeldUnbornReader, operation: OperationId,
        maximum: u64,
    ) -> Result<Self> {
        if maximum > context.config.max_file_bytes { return Err(Error::FileDraft(Failure::Size)); }
        Self::begin_source(context, SourceReader::Unborn(reader), operation, maximum)
    }

    fn begin_source(
        context: &PlanningContext<'_>, reader: SourceReader, operation: OperationId, maximum: u64,
    ) -> Result<Self> {
        let source = Source::capture(context, reader)?;
        if context.state.operations.contains_key(&operation) {
            return Err(Error::FileDraft(Failure::Publication));
        }
        let batch = match source.reader.manifest() {
            None => WorkingSave::first_with_length(source.reader.file(), operation,
                source.reader.logical_size(), maximum,
            ).map_err(manifest_error)?,
            Some(manifest) => manifest.begin_save(operation, maximum)
                .map_err(manifest_error)?,
        };
        let retained_base_bytes = source.reader.retained_base_bytes();
        Ok(Self {
            source, operation, editing: Editing::Active(batch), created: BTreeMap::new(),
            changed: false, retained_base_bytes,
        })
    }

    fn active(&self) -> Result<&WorkingSave> {
        match &self.editing {
            Editing::Active(batch) => Ok(batch),
            Editing::Failed => Err(Error::FileDraft(Failure::WorkingEditFailed)),
        }
    }

    pub(crate) fn logical_size(&self, context: &PlanningContext<'_>) -> Result<u64> {
        self.source.require_context(context)?;
        Ok(self.active()?.view().logical_size())
    }

    /// Read this ordered batch, including prepared but not yet saved extents.
    /// It is never advertised as an acknowledged Save or a full Version.
    pub(crate) fn read_at(
        &self,
        context: &PlanningContext<'_>,
        output: &mut [u8],
        offset: u64,
    ) -> Result<usize> {
        self.source.require_context(context)?;
        if output.len() > WORKING_BLOCK_BYTES as usize {
            return Err(Error::FileDraft(Failure::Size));
        }
        let view = self.active()?.view();
        let count = view.logical_size().saturating_sub(offset).min(output.len() as u64) as usize;
        let mut buffer = [0; WORKING_BLOCK_BYTES as usize];
        let mut copied = 0;
        while copied < count {
            let position = offset + copied as u64;
            let index = position / WORKING_BLOCK_BYTES;
            let within = (position % WORKING_BLOCK_BYTES) as usize;
            self.read_block(view, index, &mut buffer)?;
            let take = (count - copied).min(buffer.len() - within);
            output[copied..copied + take].copy_from_slice(&buffer[within..within + take]);
            copied += take;
        }
        self.source.require_context(context)?;
        Ok(count)
    }

    fn require_capacity(&self, context: &PlanningContext<'_>, bytes: u64) -> Result<()> {
        let disk = context.config.disks.iter().find(|disk| {
            disk.disk_id == self.source.reader.scope().disk.seal.disk_id
        }).ok_or(Error::FileDraft(Failure::SourceChanged))?;
        let prepared: u64 = self.created.values()
            .map(|extent| extent.held.block.extent().bytes()).sum();
        let used = context.used_bytes(&disk.slot_id).saturating_add(prepared);
        let available = self.source.reader.scope().disk.root.available()?;
        if bytes.saturating_add(context.config.metadata_reserve_bytes) > available
            || disk.capacity_bytes.is_some_and(|limit| bytes > limit.saturating_sub(used))
        {
            return Err(Error::FileDraft(Failure::WorkingCapacity));
        }
        Ok(())
    }

    fn read_block(
        &self,
        manifest: &WorkingManifest,
        index: u64,
        buffer: &mut [u8; WORKING_BLOCK_BYTES as usize],
    ) -> Result<()> {
        let Some(block) = manifest.block(index) else {
            return self.source.reader.base_block(index, self.retained_base_bytes, buffer);
        };
        let held = match self.created.get(&block.extent().id()) {
            Some(created) => &created.held,
            None => self.source.reader.extent(block)?,
        };
        if held.block.extent() != block.extent() {
            return Err(changed());
        }
        held.read_block(self.source.reader.scope(), buffer)?;
        buffer[block.retained_bytes() as usize..].fill(0);
        Ok(())
    }

    fn create_extent(&mut self, index: u64, bytes: &[u8]) -> Result<WorkingExtent> {
        if self.created.len() == MAX_WORKING_BLOCKS {
            return Err(Error::FileDraft(Failure::Size));
        }
        let reservation = crate::engine::payload_proof_budget().reserve(1)
            .ok_or(Error::FileDraft(Failure::Size))?;
        let id = WorkingExtentId::generate()?;
        let name = layout::container::working_extent(id);
        let hash = ExtentHash::of_bytes(bytes);
        let scope = self.source.reader.scope();
        scope.verify()?;
        crate::container_storage::version_names::check_target_name(&scope.held, &name)?;
        let receipt = scope.held.copy_new_entry_verified_synced(
            &name,
            &mut Cursor::new(bytes),
            platform_io::payload::Expectation {
                bytes: bytes.len() as u64,
                blake3: Some(hash.digest()),
            },
        ).map_err(|error| match error {
            NewEntryCopyError::Admission(source) | NewEntryCopyError::Published { source, .. } => {
                source
            }
            NewEntryCopyError::Payload(source) => source.into(),
        })?;
        let extent = WorkingExtent::from_observation(
            id,
            ContainerEntity::new(receipt.identity.device, receipt.identity.inode),
            bytes.len() as u64,
            hash,
        ).map_err(manifest_error)?;
        let held = HeldExtent::capture(scope, WorkingBlock::new(index, extent))?;
        scope.verify()?;
        self.created.insert(id, CreatedExtent { held, _reservation: reservation });
        Ok(extent)
    }

    fn replace_write_blocks(
        &mut self,
        batch: &mut WorkingSave,
        offset: u64,
        bytes: &[u8],
    ) -> Result<()> {
        let end = offset.checked_add(bytes.len() as u64)
            .ok_or(Error::FileDraft(Failure::Size))?;
        let length = batch.view().logical_size().max(end);
        if end > self.source.config.max_file_bytes {
            return Err(Error::FileDraft(Failure::Size));
        }
        let first = offset / WORKING_BLOCK_BYTES;
        let last = (end - 1) / WORKING_BLOCK_BYTES;
        let count = usize::try_from(last - first + 1)
            .map_err(|source| Error::FileDraft(Failure::WorkingCount(source)))?;
        if self.created.len().checked_add(count).is_none_or(|n| n > MAX_WORKING_BLOCKS) {
            return Err(Error::FileDraft(Failure::Size));
        }
        let mut replacements = Vec::with_capacity(count);
        let mut buffer = [0; WORKING_BLOCK_BYTES as usize];
        for index in first..=last {
            self.read_block(batch.view(), index, &mut buffer)?;
            let start = index * WORKING_BLOCK_BYTES;
            let from = offset.max(start);
            let to = start + (end - start).min(WORKING_BLOCK_BYTES);
            buffer[(from - start) as usize..(to - start) as usize]
                .copy_from_slice(&bytes[(from - offset) as usize..(to - offset) as usize]);
            let retained = (length - start).min(WORKING_BLOCK_BYTES) as usize;
            let extent = if matches!(self.source.reader, SourceReader::Unborn(_))
                && buffer[..retained].iter().all(|byte| *byte == 0) {
                None
            } else {
                Some(self.create_extent(index, &buffer[..retained])?)
            };
            replacements.push((index, extent));
        }
        batch.write_blocks(offset, bytes.len() as u64, &replacements).map_err(manifest_error)
    }

    /// This bounded call either prepares all supplied bytes or returns an error.
    /// The count is not Saved and the transport ordinal is not SaveGeneration.
    pub(crate) fn write_at(
        &mut self,
        context: &PlanningContext<'_>,
        offset: u64,
        bytes: &[u8],
    ) -> Result<usize> {
        self.source.require_context(context)?;
        self.active()?;
        if bytes.len() > WORKING_BLOCK_BYTES as usize {
            return Err(Error::FileDraft(Failure::Size));
        }
        if bytes.is_empty() {
            return Ok(0);
        }
        let end = offset.checked_add(bytes.len() as u64)
            .ok_or(Error::FileDraft(Failure::Size))?;
        if end > context.config.max_file_bytes {
            return Err(Error::FileDraft(Failure::Size));
        }
        let touched = (end - 1) / WORKING_BLOCK_BYTES - offset / WORKING_BLOCK_BYTES + 1;
        self.require_capacity(context, touched * WORKING_BLOCK_BYTES)?;
        let Editing::Active(mut batch) = std::mem::replace(&mut self.editing, Editing::Failed)
        else {
            return Err(Error::FileDraft(Failure::WorkingEditFailed));
        };
        self.replace_write_blocks(&mut batch, offset, bytes)?;
        self.source.require_context(context)?;
        self.editing = Editing::Active(batch);
        self.changed = true;
        Ok(bytes.len())
    }

    pub(crate) fn truncate(
        &mut self,
        context: &PlanningContext<'_>,
        old_length: u64,
        new_length: u64,
    ) -> Result<()> {
        self.source.require_context(context)?;
        if self.active()?.view().logical_size() != old_length {
            return Err(Error::FileDraft(Failure::WorkingLength));
        }
        let Editing::Active(mut batch) = std::mem::replace(&mut self.editing, Editing::Failed)
        else {
            return Err(Error::FileDraft(Failure::WorkingEditFailed));
        };
        batch.resize(new_length).map_err(manifest_error)?;
        self.source.require_context(context)?;
        self.editing = Editing::Active(batch);
        self.changed |= old_length != new_length;
        self.retained_base_bytes = self.retained_base_bytes.min(new_length);
        Ok(())
    }

    pub(crate) fn prepare(
        self,
        context: &PlanningContext<'_>,
    ) -> Result<Option<PreparedWorkingSave>> {
        self.source.require_context(context)?;
        self.active()?;
        if !self.changed { return Ok(None); }
        let Editing::Active(batch) = self.editing else {
            return Err(Error::FileDraft(Failure::WorkingEditFailed));
        };
        let manifest = batch.finish().map_err(manifest_error)?;
        let body = match &self.source.reader {
            SourceReader::Unborn(_) => SavedBody::Unborn,
            SourceReader::Manual(_) => SavedBody::Manual {
                payload: DraftPayload::ranges(DraftPayloadId::generate()?,
                    manifest.commitment(), manifest.logical_size()),
                retained_base_bytes: self.retained_base_bytes,
            },
        };
        let result = PreparedWorkingSave {
            source: self.source,
            operation: self.operation,
            manifest,
            created: self.created,
            body,
        };
        result.require_context(context)?;
        Ok(Some(result))
    }
}

/// Actual prepared extent feedback. There is deliberately no raw constructor,
/// serde implementation, saved receipt, FULL claim, or protection capability.
#[derive(Clone, Copy)]
enum SavedBody {
    Unborn,
    Manual { payload: DraftPayload, retained_base_bytes: u64 },
}

pub(crate) struct PreparedWorkingSave {
    body: SavedBody,
    source: Source,
    operation: OperationId,
    manifest: WorkingManifest,
    created: BTreeMap<WorkingExtentId, CreatedExtent>,
}

impl PreparedWorkingSave {
    pub(crate) fn manifest(&self) -> &WorkingManifest {
        &self.manifest
    }

    pub(crate) fn request(
        &self,
        target: &storage_contract::file_draft::DraftLocation,
    ) -> Result<storage_contract::file_draft::UnbornSaveRequest> {
        if !matches!(self.body, SavedBody::Unborn) {
            return Err(Error::FileDraft(Failure::UnbornRequired));
        }
        Ok(storage_contract::file_draft::UnbornSaveRequest {
            target: target.clone(),
            expected: self.source.reader.scope().record.working.revision(),
            manifest: self.manifest.clone(),
        })
    }

    pub(crate) fn worktree_request(
        &self, target: &storage_contract::snapshot_worktree::additions::WorktreeFileTarget,
        expected: storage_contract::snapshot_worktree::WorktreeRevision,
    ) -> Result<crate::persist::snapshot_worktree::files::WorktreeFileSaveRequest> {
        let record = &self.source.reader.scope().record;
        if !matches!(self.body, SavedBody::Unborn) || target.file != self.source.reader.file()
            || target.entry != record.namespace.entry
            || !matches!(&record.namespace.parent,
                storage_contract::file_container::ContainerParent::Worktree {
                    target: instance, path, ..
                }
                if *instance == target.worktree && *path == target.path)
        {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        Ok(crate::persist::snapshot_worktree::files::WorktreeFileSaveRequest {
            target: target.clone(), expected, working: record.working.revision(),
            manifest: self.manifest.clone(),
        })
    }

    pub(crate) fn manual_material(&self) -> Result<(DraftPayload, IncrementalManifest)> {
        let SavedBody::Manual { payload, retained_base_bytes } = self.body else {
            return Err(Error::FileDraft(Failure::ManualRequired));
        };
        Ok((payload, IncrementalManifest::new(self.manifest.clone(), retained_base_bytes)
            .map_err(manifest_error)?))
    }

    pub(crate) fn manual_command(
        &self, target: &storage_contract::file_draft::DraftLocation,
    ) -> Result<crate::persist::file_draft::RangeSaveRequest> {
        let SourceReader::Manual(reader) = &self.source.reader else {
            return Err(Error::FileDraft(Failure::ManualRequired));
        };
        let (payload, _) = self.manual_material()?;
        Ok(crate::persist::file_draft::RangeSaveRequest::new(
            target.clone(), self.source.reader.scope().record.working.revision(),
            reader.base(), self.manifest.commitment(), payload.logical_size(),
        ))
    }

    pub(crate) fn is_manual(&self) -> bool { matches!(self.body, SavedBody::Manual { .. }) }

    pub(crate) fn project(
        &self,
        context: &PlanningContext<'_>,
        target: &storage_contract::file_draft::DraftLocation,
    ) -> Result<crate::business::file_draft::unborn::Projection> {
        self.require_context(context)?;
        if !matches!(self.body, SavedBody::Unborn) {
            return Err(Error::FileDraft(Failure::UnbornRequired));
        }
        if target.file != self.source.reader.file() {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        crate::business::file_draft::unborn::project(
            context.state,
            context.config,
            self.operation,
            storage_contract::Sequence::new(context.next_slot),
            crate::business::file_draft::unborn::Edit::Save {
                target,
                expected: self.source.reader.scope().record.working.revision(),
                manifest: &self.manifest,
            },
            context.config.max_entries,
            context.config.max_file_bytes,
        )
    }

    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        self.source.require_context(context)?;
        if self.manifest.operation() != self.operation {
            return Err(Error::FileDraft(Failure::Publication));
        }
        for block in self.manifest.blocks() {
            let held = match self.created.get(&block.extent().id()) {
                Some(created) => &created.held,
                None => self.source.reader.extent(*block)?,
            };
            if held.block.extent() != block.extent() {
                return Err(changed());
            }
            held.require_named(self.source.reader.scope())?;
        }
        Ok(())
    }
}
