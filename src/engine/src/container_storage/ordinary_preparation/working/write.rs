//! Private ordered edits over one published held source. Exclusive immutable
//! packs may be prepared here; only the native publication can acknowledge a save.
use crate::business::file_draft::DraftError as Failure;
use crate::container_storage::ordinary_preparation::working::{
    HeldPack, HeldUnbornReader, changed, pack::PendingPack,
};
use crate::disk::DiskResources;
use crate::engine::PlanningContext;
use crate::{Error, Result, State, ValidatedConfig};
use std::collections::BTreeMap;
use std::sync::Arc;
use storage_contract::file_draft::manifest::{
    WorkingManifest, WorkingSave, WORKING_IO_BYTES,
    packed::{PACK_BYTES, PackId, PackOrigin, physical_bytes_for},
    ranges::{MAX_LIVE_PACKS, ReadSpan, RunSource},
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
    fn pack(&self, id: PackId) -> Result<&HeldPack> {
        match self {
            Self::Unborn(r) => r.material.packs.get(&id).ok_or_else(changed),
            Self::Manual(r) => r.pack(id),
        }
    }
    fn base_range(&self, offset: u64, retained: u64, buffer: &mut [u8]) -> Result<()> {
        match self {
            Self::Unborn(_) => { buffer.fill(0); Ok(()) },
            Self::Manual(r) => r.base_range(offset, retained, buffer),
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

enum Editing {
    Active(WorkingSave),
    Failed,
}

/// Connection-local edit state, not a durable accepted-write log. A failed frame
/// poisons this batch and cannot later be flushed as if its partial copies had
/// succeeded. Unrecorded packs are preserved, never adopted or auto-cleaned.
pub(crate) struct WorkingWriteSession {
    source: Source,
    operation: OperationId,
    editing: Editing,
    pending: BTreeMap<PackId, PendingPack>,
    current: Option<PackId>,
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
            source, operation, editing: Editing::Active(batch), pending: BTreeMap::new(), current: None,
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
        Ok(self.active()?.logical_size())
    }

    /// Connection-local read, not a Save. Authenticate only touched ranges.
    pub(crate) fn read_at(&self, context: &PlanningContext<'_>, output: &mut [u8], offset: u64) -> Result<usize> {
        self.source.require_context(context)?;
        if output.len() > WORKING_IO_BYTES as usize { return Err(Error::FileDraft(Failure::Size)); }
        let batch = self.active()?;
        let count = batch.logical_size().saturating_sub(offset).min(output.len() as u64) as usize;
        let root = &self.source.reader.scope().held;
        let mut copied = 0;
        while copied < count {
            let at = offset + copied as u64;
            let (span, bytes) = batch.span(at, offset + count as u64).map_err(manifest_error)?;
            let end = copied + bytes as usize;
            let slice = &mut output[copied..end];
            match span {
                ReadSpan::Gap => self.source.reader.base_range(at, self.retained_base_bytes, slice)?,
                ReadSpan::Zero => slice.fill(0),
                ReadSpan::Data { pack, offset } => match self.pending.get(&pack) {
                    Some(pending) => pending.read_at(root, slice, offset)?,
                    None => self.source.reader.pack(pack)?.read_at(root, slice, offset)?,
                },
            }
            copied = end;
        }
        self.source.require_context(context)?;
        Ok(count)
    }

    // All private uploads (also bytes overwritten later in this FLUSH) still
    // occupy disk until actual cleanup. Never spend credit for old live packs.
    fn require_capacity(&self, context: &PlanningContext<'_>, data: u64) -> Result<()> {
        let capacity = || Error::FileDraft(Failure::WorkingCapacity);
        let scope = self.source.reader.scope();
        let disk = context.config.disks.iter().find(|disk| disk.disk_id == scope.disk.seal.disk_id)
            .ok_or(Error::FileDraft(Failure::SourceChanged))?;
        let stat = scope.disk.root.filesystem_statistics()?;
        let unit = stat.f_bsize.max(stat.f_frsize);
        if unit == 0 || stat.f_frsize == 0 { return Err(capacity()); }
        let round = |n: u64| n.div_ceil(unit).checked_mul(unit).ok_or_else(capacity);
        let mut extra = data;
        let mut outstanding = 0u64;
        let mut all = 0u64;
        for (&id, pending) in &self.pending {
            let remaining = pending.remaining()?;
            let current = PACK_BYTES - remaining;
            let added = if self.current == Some(id) { extra.min(remaining) } else { 0 };
            extra -= added;
            let physical = physical_bytes_for(current + added).map_err(manifest_error)?;
            all = all.checked_add(physical).ok_or_else(capacity)?;
            // The checksum trailer has not yet been flushed. Held data bytes
            // are already reflected in f_bavail, but never deduct more than that.
            outstanding = outstanding.checked_add(round(physical - current)?).ok_or_else(capacity)?;
        }
        if extra != 0 {
            if self.pending.len() == MAX_LIVE_PACKS || (stat.f_files != 0 && stat.f_favail == 0) {
                return Err(capacity());
            }
            let physical = physical_bytes_for(extra).map_err(manifest_error)?;
            all = all.checked_add(physical).ok_or_else(capacity)?;
            outstanding = outstanding.checked_add(round(physical)?).ok_or_else(capacity)?;
        }
        let available = stat.f_bavail.checked_mul(stat.f_frsize).ok_or_else(capacity)?;
        if outstanding.checked_add(context.config.metadata_reserve_bytes).ok_or_else(capacity)? > available
            || disk.capacity_bytes.is_some_and(|limit| context.used_bytes(&disk.slot_id)
                .checked_add(all).is_none_or(|needed| needed > limit))
        { return Err(capacity()); }
        Ok(())
    }

    /// Only spend changed bytes. A covering zero write is an explicit logical
    /// run; it never drops a block back onto a nonzero historical base.
    pub(crate) fn write_at(&mut self, context: &PlanningContext<'_>, offset: u64, bytes: &[u8]) -> Result<usize> {
        self.source.require_context(context)?;
        let max = self.active()?.maximum();
        if bytes.len() > WORKING_IO_BYTES as usize || offset.checked_add(bytes.len() as u64).is_none_or(|end| end > max) {
            return Err(Error::FileDraft(Failure::Size));
        }
        if bytes.is_empty() { return Ok(0); }
        let sequential = match self.current.and_then(|id| self.pending.get(&id)) {
            Some(active) => self.active()?.continues_data(offset, active.id(), PACK_BYTES - active.remaining()?),
            None => false,
        };
        // Standalone zero is explicit. If it extends an actual append, storing
        // the submitted zeros keeps a long mixed sequential write in one run.
        let zero = !sequential && bytes.iter().all(|&value| value == 0);
        self.require_capacity(context, if zero { 0 } else { bytes.len() as u64 })?;
        let Editing::Active(mut batch) = std::mem::replace(&mut self.editing, Editing::Failed) else {
            return Err(Error::FileDraft(Failure::WorkingEditFailed));
        };
        if zero {
            batch.write(offset, bytes.len() as u64, RunSource::Zero {}).map_err(manifest_error)?;
        } else {
            let root = &self.source.reader.scope().held;
            let origin = PackOrigin { file: self.source.reader.file(), operation: self.operation };
            let mut pos = 0;
            while pos < bytes.len() {
                let pending = self.current.and_then(|id| self.pending.get(&id));
                let (id, remaining, create) = match pending {
                    Some(p) if p.remaining()? > 0 => (p.id(), p.remaining()?, false),
                    _ => (PackId::generate()?, PACK_BYTES, true),
                };
                let take = remaining.min((bytes.len() - pos) as u64) as usize;
                let physical = PACK_BYTES - remaining;
                batch.write(offset + pos as u64, take as u64, RunSource::Data { pack: id, offset: physical })
                    .map_err(manifest_error)?;
                if create {
                    if self.pending.contains_key(&id) { return Err(changed()); }
                    self.pending.insert(id, PendingPack::create(root, id, origin)?);
                    self.current = Some(id);
                }
                if self.pending.get_mut(&id).ok_or_else(changed)?.append(root, &bytes[pos..pos + take])? != physical {
                    return Err(changed());
                }
                pos += take;
            }
        }
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
        if self.active()?.logical_size() != old_length {
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
        self.require_capacity(context, 0)?;
        let Editing::Active(mut batch) = self.editing else {
            return Err(Error::FileDraft(Failure::WorkingEditFailed));
        };
        let live = batch.references();
        let mut created = BTreeMap::new();
        for (id, pending) in self.pending {
            if live.contains(&id) {
                let held = pending.seal(&self.source.reader.scope().held)?;
                batch.seal_pack(held.evidence()).map_err(manifest_error)?;
                created.insert(id, held);
            } // Unreferenced partial retained on disk, never adopted or deleted here.
        }
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
            created,
            body,
        };
        result.require_context(context)?;
        Ok(Some(result))
    }
}

/// Actual prepared packed-material feedback. There is deliberately no raw constructor,
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
    created: BTreeMap<PackId, HeldPack>,
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
        for &pack in self.manifest.packs() {
            let held = match self.created.get(&pack.id()) {
                Some(fresh) => fresh,
                None => self.source.reader.pack(pack.id())?,
            };
            if held.evidence() != pack { return Err(changed()); }
            held.validate(&self.source.reader.scope().held)?;
        }
        Ok(())
    }
}
