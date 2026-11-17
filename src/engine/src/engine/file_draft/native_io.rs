//! Opaque application handle over the existing native working publication.
//! No mutable data FD or caller-supplied manifest can bypass extent preparation.
use crate::business::file_draft::DraftError as Failure;
use crate::container_storage::{HeldUnbornReader, PreparedUnbornSave, UnbornWriteSession};
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

pub struct NativeWorkingFile {
    target: DraftLocation,
    principal: Principal,
    reader: HeldUnbornReader,
    writable: bool,
    stage: Stage,
    operation: Option<OperationId>,
    saved: Option<OperationResult>,
}

enum Stage {
    Idle,
    Editing(UnbornWriteSession),
    Pending { request: UnbornSaveRequest, material: Arc<PreparedUnbornSave> },
    Failed,
}

impl NativeWorkingFile {
    pub fn target(&self) -> &DraftLocation { &self.target }
    pub fn operation(&self) -> Option<OperationId> { self.operation }

    fn require_writer(&self, principal: &Principal) -> Result<()> {
        if !self.writable || &self.principal != principal {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        Ok(())
    }

    fn editing(&mut self, engine: &Engine, principal: &Principal)
        -> Result<&mut UnbornWriteSession>
    {
        self.require_writer(principal)?;
        if matches!(self.stage, Stage::Idle) {
            // A later batch gets a new operation only from the then-current
            // admitted source. We do not rebind an old Editing source Arc.
            let record = engine.state.file_containers.get(&self.target.file)
                .ok_or(Error::FileDraft(Failure::SourceChanged))?;
            self.reader.require_record(self.target.file, record)?;
            let operation = OperationId::generate()?;
            let editing = engine.begin_unborn_write(&self.target, operation)?;
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
    pub fn native_current_entries(
        &self, repository: storage_contract::RepositoryId,
    ) -> Result<Vec<NativeCurrentEntry>> {
        use crate::business::namespace::working::{WorkingEntry, WorkingNamespace};
        let repo = self.state.repositories.get(&repository.to_string())
            .ok_or(Error::FileDraft(Failure::WorkingNamespace))?;
        self.namespace_snapshot(&repo.main_branch_id)?;
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
            target: target.clone(), principal: principal.clone(),
            reader: self.open_unborn_working(target)?, writable, stage: Stage::Idle,
            operation: None, saved: None,
        })
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
                    let request = material.request(&file.target);
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
        let result = self.save_unborn_working(request, principal, Some(Arc::clone(material)))?;
        // If post-publication source admission fails, retain Pending. The same
        // request will reach Existing before its old source revision on retry.
        let reader = self.open_unborn_working(&file.target)?;
        file.reader = reader;
        file.stage = Stage::Idle;
        file.saved = Some(result.clone());
        Ok(Some(result))
    }
}
