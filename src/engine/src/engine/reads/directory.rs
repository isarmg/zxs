//! An immutable FILE read on one exact issuing disk, retained across an outer
//! writer's preparation. Neither an equal backup nor a replacement name is it.
use super::*;
use crate::container_storage::ObservedContainerReplica;
use crate::engine::{PayloadProofReservation, fresh_layout::FreshLayoutError as Failure};
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use std::io::{Seek, SeekFrom};
use storage_contract::{DiskConfig, FileId, VersionId};

pub(crate) struct HeldDirectoryFileBody {
    file: ReadFile,
    scope: Arc<ObservedContainerReplica>,
    version: FileVersion,
    parent: DiskRoot,
    name: EntryName,
    parent_id: FileIdentity,
    entity: FileIdentity,
    _permit: PayloadProofReservation,
}
impl HeldDirectoryFileBody {
    pub(crate) fn identity(&self) -> (FileIdentity,&EntryName,FileIdentity) {
        (self.parent_id, &self.name, self.entity)
    }
    pub(crate) fn reader(&mut self) -> &mut ReadFile { &mut self.file }

    pub(crate) fn require_named(&self) -> Result<()> {
        self.scope.verify()?;
        let material = VersionMaterial::current(&self.scope, &self.version)?;
        let container = material.container()?;
        let versions = container.path().join(crate::persist::layout::VERSIONS.as_path());
        if self.parent.path() != versions || container.directory_identity(&versions)? != self.parent_id
            || material.body_path()? != versions.join(self.name.as_path())
        { return Err(Failure::ContainerScope.into()); }
        let actual = self.parent.open_read_entry(&self.name)?;
        if FileIdentity::from_metadata(&actual.metadata()?) != self.entity
            || FileIdentity::from_metadata(&self.file.metadata()?) != self.entity
        { return Err(Failure::ContainerIdentity.into()); }
        Ok(())
    }

    pub(crate) fn open_original(&self) -> Result<ReadFile> {
        self.require_named()?;
        let mut file = self.file.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }
}

impl Engine {
    pub(crate) fn open_directory_file_body(&self, file: FileId, version: VersionId,
        disk: &DiskConfig) -> Result<HeldDirectoryFileBody> {
        let permit = crate::engine::payload_proof_budget().reserve(9).ok_or(Failure::ContainerBudget)?;
        // Once a root has been bound, the existing normal read kernel may
        // authenticate only that registration. No other replica is a fallback.
        let (held,scope,v,parent,name,parent_id,entity) =
            self.with_matching_replica(&version.to_string(),Some(disk), |scope,material,v,lease| {
                if v.file_id != file.to_string() { return Err(Failure::ContainerScope.into()); }
                let container = material.container()?;
                let versions = container.path().join(crate::persist::layout::VERSIONS.as_path());
                let address = material.body_path()?;
                if address.parent() != Some(versions.as_path()) { return Err(Failure::ContainerScope.into()); }
                let name = EntryName::new(address.file_name().ok_or(Failure::ContainerScope)?)
                    .map_err(|_| Failure::ContainerScope)?;
                let parent_id = container.directory_identity(&versions)?;
                let parent = container.private_child_matching(&versions,parent_id)?;
                let mut body = verify_read_payload(material,v,lease)?;
                let entity = FileIdentity::from_metadata(&body.metadata()?);
                body.seek(SeekFrom::Start(0))?;
                Ok((ReadFile::in_container(body,Arc::clone(scope)),Arc::clone(scope),v.clone(),parent,name,parent_id,entity))
            })?;
        let source = HeldDirectoryFileBody { file:held,scope,version:v,parent,name,parent_id,entity,_permit:permit };
        source.require_named()?;
        Ok(source)
    }
}
