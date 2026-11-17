//! A published draft reader retains the same State and actual container proof.
use super::FreshLayoutError as Failure;
use crate::container_storage::PreparedContainerEntities;
use crate::engine::{PlanningContext, ReadSource};
use crate::{ReadFile, Result, State, ValidatedConfig};
use crate::disk::DiskResources;
use std::{path::{Path, PathBuf}, sync::Arc};
use storage_contract::FileId;

pub(crate) struct PublishedDraftSource {
    state: Arc<State>,
    config: ValidatedConfig,
    resources: Arc<DiskResources>,
    containers: Arc<PreparedContainerEntities>,
    file: FileId,
    path: PathBuf,
}
impl PublishedDraftSource {
    pub(crate) fn open(&self, path: &Path) -> Result<ReadFile> {
        if path != self.path {
            return Err(Failure::ContainerScope.into());
        }
        super::verify_available(&self.config, &self.resources)?;
        let record = self.state.file_containers.get(&self.file)
            .ok_or(Failure::ContainerScope)?;
        if record.working.draft().is_none() {
            return Err(Failure::ContainerScope.into());
        }
        self.containers.open_draft(self.file)
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
        let containers = Arc::new(PreparedContainerEntities::published(
            self.config, self.disk_resources, self.state,
        )?);
        let root = containers.target(file, record.owner_disk)?;
        let path = root.path().join(crate::persist::layout::container::DRAFTS.as_path())
            .join(crate::persist::layout::container::draft_payload(payload.payload_id()).as_path());
        let source = Arc::new(PublishedDraftSource {
            state: Arc::clone(self.state), config: self.config.clone(),
            resources: Arc::clone(self.disk_resources), containers, file, path: path.clone(),
        });
        // Full uploads retain their full-byte admission. Range drafts retain the
        // authenticated base sidecar and extent identities; their reader
        // verifies only returned chunks. Close the named source interval here.
        source.open(&path)?;
        Ok(ReadSource::VerifiedDraft { source, path })
    }
}
