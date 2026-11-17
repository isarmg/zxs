//! A published draft reader retains the same State and actual container proof.
use crate::container_storage::ObservedContainerReplica;
use crate::engine::fresh_layout::{FreshLayoutError as Failure, containers};
use crate::engine::{PlanningContext, ReadSource};
use crate::{ReadFile, Result, State, ValidatedConfig};
use crate::disk::DiskResources;
use std::{path::{Path, PathBuf}, sync::Arc};
use storage_contract::FileId;

pub(crate) struct PublishedDraftSource {
    state: Arc<State>,
    config: ValidatedConfig,
    resources: Arc<DiskResources>,
    container: Arc<ObservedContainerReplica>,
    file: FileId,
    path: PathBuf,
}
impl PublishedDraftSource {
    pub(crate) fn open(&self, path: &Path) -> Result<ReadFile> {
        if path != self.path {
            return Err(Failure::ContainerScope.into());
        }
        crate::engine::fresh_layout::verify_available(&self.config, &self.resources)?;
        let record = self.state.file_containers.get(&self.file)
            .ok_or(Failure::ContainerScope)?;
        if record.working.draft().is_none() {
            return Err(Failure::ContainerScope.into());
        }
        self.container.open_working_draft()
    }
}
impl PlanningContext<'_> {
    pub(crate) fn draft_payload_source(&self, file: FileId) -> Result<ReadSource> {
        let record = self.state.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
        if self.current_namespaces.is_some() || self.config.generation().is_some()
            || record.replicas.len() != 1
        {
            return Err(Failure::ContainerScope.into());
        }
        let (_, payload) = record.working.draft().ok_or(Failure::ContainerScope)?;
        // The State-wide logical closure remains mandatory. Physical source
        // admission is scoped to this exact owner, as for native range reads;
        // unrelated draft bodies do not consume descriptors or get read here.
        containers::validate_state(self.state)?;
        if payload.logical_size() > self.config.max_file_bytes {
            return Err(Failure::DraftTooLarge.into());
        }
        let disk = self.config.disks.iter()
            .find(|disk| disk.disk_id == record.owner_disk.to_string())
            .ok_or(Failure::Registration)?;
        let container = Arc::new(ObservedContainerReplica::observe(containers::published_source(
            self.config, self.disk_resources, self.state, file, disk,
        )?)?);
        let root = container.container()?;
        let path = root.path().join(crate::persist::layout::container::DRAFTS.as_path())
            .join(crate::persist::layout::container::draft_payload(payload.payload_id()).as_path());
        let source = Arc::new(PublishedDraftSource {
            state: Arc::clone(self.state), config: self.config.clone(),
            resources: Arc::clone(self.disk_resources), container, file, path: path.clone(),
        });
        // Full uploads retain their full-byte admission. Range drafts retain the
        // authenticated base sidecar and extent identities; their reader
        // verifies only returned chunks. Close the named source interval here.
        source.open(&path)?;
        Ok(ReadSource::VerifiedDraft { source, path })
    }
}
