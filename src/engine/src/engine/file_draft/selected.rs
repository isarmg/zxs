//! Full-set working selection, followed by the original held-source admission.
//! Nothing here prepares a Version, clears a draft or acknowledges a save.
use crate::business::file_draft::{DraftError as Failure, selected};
use crate::container_storage::{HeldUnbornReader, ObservedContainerReplica};
use crate::engine::file_draft::require_initial;
use crate::engine::fresh_layout::containers::published_source;
use crate::{Engine, Error, ReadFile, Result, State, ValidatedConfig};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use storage_contract::file_container::FileContainerRecord;
use storage_contract::file_draft::{
    DraftLocation, DraftPayload, WorkingContentExpectation, WorkingExpectation, WorkingSelection,
};

enum HeldWorkingBody {
    Born {
        scope: Arc<ObservedContainerReplica>,
        file: ReadFile,
        payload: DraftPayload,
    },
    Unborn(Arc<HeldUnbornReader>),
}

pub(crate) struct SelectedWorkingSource {
    request: WorkingSelection,
    record: FileContainerRecord,
    body: HeldWorkingBody,
}

impl SelectedWorkingSource {
    pub(crate) fn selection(&self) -> &WorkingSelection { &self.request }
    pub(crate) fn target(&self) -> &DraftLocation { &self.request.target }
    pub(crate) fn expected(&self) -> WorkingExpectation { self.request.expected }
    pub(crate) fn record(&self) -> &FileContainerRecord { &self.record }

    pub(crate) fn draft_digest(
        &self, state: &State, chunk_bytes: u32,
    ) -> Result<Option<Arc<crate::container_storage::ReadDraftDigest>>> {
        if self.record.working.incremental().is_none() { return Ok(None); }
        let HeldWorkingBody::Born { scope, .. } = &self.body else {
            return Err(Error::FileDraft(Failure::WorkingBodyChanged));
        };
        let (base, _) = self.record.working.draft()
            .ok_or(Error::FileDraft(Failure::WorkingBodyChanged))?;
        let version = state.versions.get(&base.version_id().to_string())
            .ok_or(Error::FileDraft(Failure::WorkingBodyChanged))?;
        let reader = crate::container_storage::HeldManualReader::capture(
            Arc::clone(scope), *base, version,
        )?;
        crate::container_storage::ReadDraftDigest::capture(Arc::new(reader), chunk_bytes).map(Some)
    }

    pub(crate) fn born_scope(&self) -> Result<&Arc<ObservedContainerReplica>> {
        match &self.body {
            HeldWorkingBody::Born { scope, .. } => Ok(scope),
            HeldWorkingBody::Unborn(_) => Err(Error::FileDraft(Failure::WorkingBodyChanged)),
        }
    }
    pub(crate) fn unborn_reader(&self) -> Result<&Arc<HeldUnbornReader>> {
        match &self.body {
            HeldWorkingBody::Unborn(reader) => Ok(reader),
            HeldWorkingBody::Born { .. } => Err(Error::FileDraft(Failure::UnbornRequired)),
        }
    }

    pub(crate) fn logical_size(&self) -> u64 {
        match &self.body {
            HeldWorkingBody::Born { payload, .. } => payload.logical_size(),
            HeldWorkingBody::Unborn(source) => source.logical_size(),
        }
    }

    pub(crate) fn require_source(&self) -> Result<()> {
        match &self.body {
            HeldWorkingBody::Born { scope, file, .. } => {
                file.check_running()?;
                scope.verify()
            }
            HeldWorkingBody::Unborn(source) => {
                source.require_record(self.request.target.file, &self.record)
            }
        }
    }

    /// Only the selected immutable body is readable. No FD or path escapes to
    /// a caller which could mistake sparse holes for a complete base version.
    pub(crate) fn read_at(&self, output: &mut [u8], offset: u64) -> Result<usize> {
        self.require_source()?;
        let read = match &self.body {
            HeldWorkingBody::Born { file, payload, .. } => {
                if offset >= payload.logical_size() || output.is_empty() {
                    return Ok(0);
                }
                let count = (payload.logical_size() - offset).min(output.len() as u64) as usize;
                file.read_exact_at(&mut output[..count], offset)?;
                count
            }
            HeldWorkingBody::Unborn(source) => source.read_at(output, offset)?,
        };
        self.require_source()?;
        Ok(read)
    }
}

/// A local, owned read set from one actual Engine occurrence. It is not a
/// serializable receipt and cannot be reconstructed from identities or hashes.
pub(crate) struct SelectedWorkingSources {
    state: Arc<State>,
    config: ValidatedConfig,
    resources: Arc<crate::disk::DiskResources>,
    snapshot: Option<String>,
    slot: u64,
    sources: Vec<SelectedWorkingSource>,
}

impl SelectedWorkingSources {
    pub(crate) fn open_sole_born(&self) -> Result<ReadFile> {
        if self.sources.len() != 1 { return Err(crate::business::file_draft::invalid()); }
        self.open_born(0, &self.born_path(0)?)
    }
    pub(crate) fn sources(&self) -> &[SelectedWorkingSource] { &self.sources }

    /// The path is diagnostic input to ReadSource, not another source issuer.
    /// Its open method below accepts only this same selection and held payload.
    pub(in crate::engine) fn born_path(&self, index: usize) -> Result<PathBuf> {
        let source = self.sources.get(index)
            .ok_or(Error::FileDraft(Failure::WorkingSelection))?;
        match &source.body {
            HeldWorkingBody::Born { scope, payload, .. } => {
                let layout = crate::persist::layout::container::draft_payload(payload.payload_id());
                Ok(scope.container()?.path()
                    .join(crate::persist::layout::container::DRAFTS.as_path())
                    .join(layout.as_path()))
            }
            HeldWorkingBody::Unborn(_) => Err(Error::FileDraft(Failure::UnsupportedScope)),
        }
    }

    pub(in crate::engine) fn open_born(&self, index: usize, path: &Path) -> Result<ReadFile> {
        if path != self.born_path(index)? {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        for source in &self.sources { source.require_source()?; }
        let source = self.sources.get(index)
            .ok_or(Error::FileDraft(Failure::WorkingSelection))?;
        let file = match &source.body {
            // HeldDraft::open clones the actual admitted descriptor, retains
            // its container and resets its cursor. It does not adopt by name.
            HeldWorkingBody::Born { scope, .. } => scope.open_working_draft()?,
            HeldWorkingBody::Unborn(_) => {
                return Err(Error::FileDraft(Failure::UnsupportedScope));
            }
        };
        source.require_source()?;
        Ok(file)
    }

    pub(crate) fn require_engine(&self, engine: &Engine) -> Result<()> {
        self.require_context(&engine.planning_context())
    }

    pub(crate) fn require_context(
        &self, context: &crate::engine::PlanningContext<'_>,
    ) -> Result<()> {
        if !Arc::ptr_eq(&self.state, context.state)
            || !self.config.same_admission(context.config)
            || !Arc::ptr_eq(&self.resources, context.disk_resources)
            || self.snapshot != *context.snapshot || self.slot != context.next_slot
        {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        for source in &self.sources {
            source.require_source()?;
        }
        Ok(())
    }
}

impl Engine {
    /// The caller separately admits the directory's Published host/node and its
    /// selected path set. That source cannot grant FILE authority to this method.
    pub(crate) fn select_working_sources(
        &self,
        requests: &[WorkingSelection],
    ) -> Result<SelectedWorkingSources> {
        SelectedWorkingSources::capture(&self.planning_context(), requests)
    }
}

impl SelectedWorkingSources {
    pub(crate) fn capture(context: &crate::engine::PlanningContext<'_>,
        requests: &[WorkingSelection]) -> Result<Self>
    {
        if context.current_namespaces.is_some() || context.config.generation().is_some() {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        // Complete every selected CAS before opening even the first payload.
        // In particular a later stale item cannot follow an earlier clear/save.
        let checked = selected::require_all(
            context.state, requests, context.config.max_entries, context.config.max_file_bytes,
        )?;
        let mut sources = Vec::with_capacity(checked.len());
        for item in checked {
            let request = item.request();
            let record = item.record();
            require_initial(&context, &request.target)?;
            let owner = record.owner_disk.to_string();
            let disk = context.config.disks.iter().find(|disk| disk.disk_id == owner)
                .ok_or(Error::FileDraft(Failure::UnsupportedScope))?;
            let source = published_source(
                context.config, context.disk_resources, context.state, request.target.file, disk,
            )?;
            let scope = Arc::new(ObservedContainerReplica::observe(source)?);
            let body = match request.expected.content {
                WorkingContentExpectation::Born { payload, .. } => HeldWorkingBody::Born {
                    file: scope.open_working_draft()?, scope, payload,
                },
                WorkingContentExpectation::UnbornEmpty
                | WorkingContentExpectation::UnbornSaved { .. } => {
                    HeldWorkingBody::Unborn(Arc::new(
                        scope.open_unborn_working(context.config.max_file_bytes)?,
                    ))
                }
            };
            sources.push(SelectedWorkingSource {
                request: request.clone(), record: record.clone(), body,
            });
        }
        let result = SelectedWorkingSources {
            state: Arc::clone(context.state), config: context.config.clone(),
            resources: Arc::clone(context.disk_resources), snapshot: context.snapshot.clone(),
            slot: context.next_slot, sources,
        };
        result.require_context(context)?;
        Ok(result)
    }
}
