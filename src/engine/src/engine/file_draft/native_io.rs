//! Opaque application handle over the existing native working publication.
//! No mutable data FD or caller-supplied manifest can bypass extent preparation.
use crate::business::file_draft::DraftError as Failure;
use crate::container_storage::{
    HeldManualReader, HeldUnbornReader, PreparedWorkingSave, WorkingWriteSession,
};
use crate::{Engine, Error, OperationResult, Result};
use std::sync::Arc;
use storage_contract::file_draft::{DraftLocation, UnbornSaveRequest};
use storage_contract::{OperationId, Principal};

/// Descriptive current namespace, never a synthetic historical Entry.
#[derive(Clone, Debug)]
pub enum NativeCurrentEntry {
    Published(crate::Entry),
    Unborn {
        target: DraftLocation,
        working: storage_contract::file_draft::UnbornWorking,
    },
}

/// Exact native issuer, not a guessed ordinary namespace for an instance path.
pub enum NativeWorkingTarget {
    Current(DraftLocation),
    Worktree {
        target: storage_contract::snapshot_worktree::additions::WorktreeFileTarget,
        revision: storage_contract::snapshot_worktree::WorktreeRevision,
        maximum: u64,
    },
}
impl NativeWorkingTarget {
    fn file(&self) -> storage_contract::FileId {
        match self {
            Self::Current(target) => target.file,
            Self::Worktree { target, .. } => target.file,
        }
    }
}

pub struct NativeWorkingFile {
    target: NativeWorkingTarget,
    principal: Principal,
    reader: NativeReader,
    writable: bool,
    stage: Stage,
    operation: Option<OperationId>,
    saved: Option<OperationResult>,
}

enum Stage {
    Idle,
    Editing(WorkingWriteSession),
    Pending { request: SaveRequest, material: Arc<PreparedWorkingSave> },
    Failed,
}

enum NativeReader {
    Unborn(HeldUnbornReader),
    Manual(HeldManualReader),
}

enum SaveRequest {
    Unborn(UnbornSaveRequest),
    Manual(crate::persist::file_draft::RangeSaveRequest),
    Worktree(crate::persist::snapshot_worktree::files::WorktreeFileSaveRequest),
}

impl NativeReader {
    fn require_source(&self) -> Result<()> {
        match self {
            Self::Unborn(reader) => reader.require_source(),
            Self::Manual(reader) => reader.require_source(),
        }
    }
    fn logical_size(&self) -> u64 {
        match self {
            Self::Unborn(reader) => reader.logical_size(),
            Self::Manual(reader) => reader.logical_size(),
        }
    }
    fn read_at(&self, output: &mut [u8], offset: u64) -> Result<usize> {
        match self {
            Self::Unborn(reader) => reader.read_at(output, offset),
            Self::Manual(reader) => reader.read_at(output, offset),
        }
    }
    fn require_record(
        &self, file: storage_contract::FileId,
        record: &storage_contract::file_container::FileContainerRecord,
    ) -> Result<()> {
        match self {
            Self::Unborn(reader) => reader.require_record(file, record),
            Self::Manual(reader) => reader.require_record(file, record),
        }
    }
}

impl NativeWorkingFile {
    pub fn target(&self) -> &NativeWorkingTarget { &self.target }

    pub(crate) fn worktree(
        target: storage_contract::snapshot_worktree::additions::WorktreeFileTarget,
        principal: Principal, reader: HeldUnbornReader, writable: bool,
        revision: storage_contract::snapshot_worktree::WorktreeRevision, maximum: u64,
    ) -> Self {
        Self {
            target: NativeWorkingTarget::Worktree { target, revision, maximum },
            principal, reader: NativeReader::Unborn(reader), writable, stage: Stage::Idle,
            operation: None, saved: None,
        }
    }
    pub fn operation(&self) -> Option<OperationId> { self.operation }

    fn require_writer(&self, principal: &Principal) -> Result<()> {
        if !self.writable || &self.principal != principal {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        Ok(())
    }

    fn editing(&mut self, engine: &Engine, principal: &Principal)
        -> Result<&mut WorkingWriteSession>
    {
        self.require_writer(principal)?;
        if matches!(self.stage, Stage::Idle) {
            // A later batch gets a new operation only from the then-current
            // admitted source. We do not rebind an old Editing source Arc.
            let record = engine.state.file_containers.get(&self.target.file())
                .ok_or(Error::FileDraft(Failure::SourceChanged))?;
            self.reader.require_record(self.target.file(), record)?;
            let operation = OperationId::generate()?;
            let editing = match (&self.target, &self.reader) {
                (NativeWorkingTarget::Current(target), NativeReader::Unborn(_)) => {
                    engine.begin_unborn_write(target, operation)?
                }
                (NativeWorkingTarget::Current(target), NativeReader::Manual(_)) => {
                    WorkingWriteSession::begin_manual(&engine.planning_context(),
                        engine.open_manual_working(target)?, operation)?
                }
                (NativeWorkingTarget::Worktree { target, revision, maximum },
                    NativeReader::Unborn(_)) => {
                    let (reader, current, limit) = crate::snapshot_worktree::additions::reader(
                        engine, target, true,
                    ).map_err(crate::business::file_draft::worktree::failure)?;
                    if current != *revision || limit != *maximum {
                        return Err(Error::FileDraft(Failure::SourceChanged));
                    }
                    WorkingWriteSession::begin_bounded(&engine.planning_context(), reader,
                        operation, limit)?
                }
                (NativeWorkingTarget::Worktree { .. }, NativeReader::Manual(_)) => {
                    return Err(Error::FileDraft(Failure::UnbornRequired));
                }
            };
            self.stage = Stage::Editing(editing);
            self.operation = Some(operation);
            self.saved = None;
        }
        match &mut self.stage {
            Stage::Editing(editing) => Ok(editing),
            Stage::Idle | Stage::Pending { .. } | Stage::Failed => {
                Err(Error::FileDraft(Failure::WorkingEditFailed))
            }
        }
    }
}

impl Engine {
    /// A complete, admitted native state with no Unborn row for this root
    /// can describe the registry's published listing exactly. A possible
    /// row is a reason to refuse, never a grant to enumerate its name or size.
    pub(crate) fn require_projected_unborn_absence(&self, repository: storage_contract::RepositoryId)
        -> Result<()> {
        if self.state.file_containers.values().any(|row| {
            row.namespace.repository == repository && row.working.unborn().is_some()
        }) {
            return Err(Error::new(crate::ErrorCode::UnsupportedOperation,
                "the current disk registry has no proved listing for this root's uncommitted FILEs"));
        }
        Ok(())
    }

    pub fn native_current_entries(
        &self, repository: storage_contract::RepositoryId,
    ) -> Result<Vec<NativeCurrentEntry>> {
        use crate::business::namespace::working::{WorkingEntry, WorkingNamespace};
        let repo = self.state.repositories.get(&repository.to_string())
            .ok_or(Error::FileDraft(Failure::WorkingNamespace))?;
        let selected = self.ordinary_metadata_root_for_slot(&repo.primary_slot_id)?
            .filter(|held| held.repository_id == repo.repository_id
                && held.main_branch_id == repo.main_branch_id)
            .ok_or(Error::FileDraft(Failure::WorkingNamespace))?;
        let snapshot = self.namespace_snapshot(&selected.main_branch_id)?;
        if self.admitted_current_namespaces()?.is_some() || self.config.generation().is_some() {
            self.require_projected_unborn_absence(repository)?;
            // The detached/disabled registry owns these scoped rows, not the
            // raw historic HEAD. No payload or stopped root is opened here.
            return Ok(snapshot.tree().visible_entries().cloned()
                .map(NativeCurrentEntry::Published).collect());
        }
        let namespace = WorkingNamespace::from_main_state(
            &self.state, repository, self.config.max_entries,
        )?;
        namespace.entries().iter().map(|entry| match entry {
            WorkingEntry::Published(entry) => Ok(NativeCurrentEntry::Published(entry.clone())),
            WorkingEntry::Unborn { file, record, path } => {
                let working = record.working.unborn().cloned()
                    .ok_or(Error::FileDraft(Failure::UnbornRequired))?;
                Ok(NativeCurrentEntry::Unborn {
                    target: DraftLocation {
                        repository, path: path.clone(), file: *file, entry: record.namespace.entry,
                    },
                    working,
                })
            }
        }).collect()
    }

    pub fn open_native_working(
        &self, target: &DraftLocation, principal: &Principal, writable: bool,
    ) -> Result<NativeWorkingFile> {
        if writable { self.planning_context().require_write()?; }
        Ok(NativeWorkingFile {
            target: NativeWorkingTarget::Current(target.clone()), principal: principal.clone(),
            reader: self.open_native_reader(target)?, writable, stage: Stage::Idle,
            operation: None, saved: None,
        })
    }

    fn open_native_reader(&self, target: &DraftLocation) -> Result<NativeReader> {
        let record = self.state.file_containers.get(&target.file)
            .ok_or(Error::FileDraft(Failure::SourceChanged))?;
        if record.working.unborn().is_some() {
            self.open_unborn_working(target).map(NativeReader::Unborn)
        } else {
            self.open_manual_working(target).map(NativeReader::Manual)
        }
    }

    pub fn native_working_size(&self, file: &NativeWorkingFile) -> Result<u64> {
        match &file.stage {
            Stage::Editing(editing) => editing.logical_size(&self.planning_context()),
            Stage::Idle => {
                file.reader.require_source()?;
                Ok(file.reader.logical_size())
            }
            Stage::Pending { .. } | Stage::Failed => {
                Err(Error::FileDraft(Failure::WorkingEditFailed))
            }
        }
    }

    pub fn read_native_working(
        &self, file: &NativeWorkingFile, output: &mut [u8], offset: u64,
    ) -> Result<usize> {
        match &file.stage {
            Stage::Editing(editing) => editing.read_at(&self.planning_context(), output, offset),
            Stage::Idle => file.reader.read_at(output, offset),
            Stage::Pending { .. } | Stage::Failed => {
                Err(Error::FileDraft(Failure::WorkingEditFailed))
            }
        }
    }

    pub fn write_native_working(
        &self, file: &mut NativeWorkingFile, principal: &Principal, offset: u64, bytes: &[u8],
    ) -> Result<usize> {
        file.editing(self, principal)?.write_at(&self.planning_context(), offset, bytes)
    }

    pub fn append_native_working(
        &self, file: &mut NativeWorkingFile, principal: &Principal, bytes: &[u8],
    ) -> Result<usize> {
        let context = self.planning_context();
        let editing = file.editing(self, principal)?;
        let end = editing.logical_size(&context)?;
        editing.write_at(&context, end, bytes)
    }

    pub fn resize_native_working(
        &self, file: &mut NativeWorkingFile, principal: &Principal, length: u64,
    ) -> Result<()> {
        let context = self.planning_context();
        let editing = file.editing(self, principal)?;
        let previous = editing.logical_size(&context)?;
        editing.truncate(&context, previous, length)
    }

    /// Pending retains the same original request and held prepared extents on
    /// every error. Only a genuine native Published/Existing result may reset
    /// the batch; ACCEPT and a lost response are not a second successful save.
    pub fn flush_native_working(
        &mut self, file: &mut NativeWorkingFile, principal: &Principal,
    ) -> Result<Option<OperationResult>> {
        file.require_writer(principal)?;
        if matches!(file.stage, Stage::Editing(_)) {
            let Stage::Editing(editing) = std::mem::replace(&mut file.stage, Stage::Failed) else {
                return Err(Error::FileDraft(Failure::WorkingEditFailed));
            };
            match editing.prepare(&self.planning_context())? {
                Some(material) => {
                    let request = match &file.target {
                        NativeWorkingTarget::Current(target) if material.is_manual() => {
                            SaveRequest::Manual(material.manual_command(target)?)
                        }
                        NativeWorkingTarget::Current(target) => {
                            SaveRequest::Unborn(material.request(target)?)
                        }
                        NativeWorkingTarget::Worktree { target, revision, .. } => {
                            SaveRequest::Worktree(material.worktree_request(target, *revision)?)
                        }
                    };
                    file.stage = Stage::Pending { request, material: Arc::new(material) };
                }
                None => { file.stage = Stage::Idle; }
            }
        }
        match &file.stage {
            Stage::Idle => return Ok(file.saved.clone()),
            Stage::Pending { .. } => {}
            Stage::Editing(_) | Stage::Failed => {
                return Err(Error::FileDraft(Failure::WorkingEditFailed));
            }
        }
        let Stage::Pending { request, material } = &file.stage else {
            return Err(Error::FileDraft(Failure::WorkingEditFailed));
        };
        let result = match request {
            SaveRequest::Unborn(request) => self.save_unborn_working(
                request, principal, Some(Arc::clone(material)),
            )?,
            SaveRequest::Worktree(request) => crate::snapshot_worktree::additions::save(
                self, request, principal, Arc::clone(material),
            ).map_err(crate::business::file_draft::worktree::failure)?,
            SaveRequest::Manual(request) => self.save_incremental_working(
                request, principal, material.manifest().operation(), Arc::clone(material),
            )?,
        };
        // If post-publication source admission fails, retain Pending. The same
        // request will reach Existing before its old source revision on retry.
        let reader = match &mut file.target {
            NativeWorkingTarget::Current(target) => self.open_native_reader(target)?,
            NativeWorkingTarget::Worktree { target, revision, maximum } => {
                let (reader, current, limit) =
                    crate::snapshot_worktree::additions::reader(self, target, true)
                    .map_err(crate::business::file_draft::worktree::failure)?;
                *revision = current;
                *maximum = limit;
                NativeReader::Unborn(reader)
            }
        };
        file.reader = reader;
        file.stage = Stage::Idle;
        file.saved = Some(result.clone());
        Ok(Some(result))
    }
}
