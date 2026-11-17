//! The actual empty EXCL occurrence may initialize its first FILE database.
//! No decoded Unborn tag, arbitrary empty directory or published path mints this.
use crate::business::file_draft::invalid;
use crate::container_storage::ordinary_preparation::ObservedContainerReplica;
use crate::disk::DiskRoot;
use crate::engine::fresh_layout::containers::VerifiedUnbornBirth;
use crate::state_projection::StateProjection;
use crate::Result;
use storage_contract::{DiskId, FileId};

pub(in crate::container_storage) struct UnbornBirthFile<'a> {
    proof: &'a VerifiedUnbornBirth,
    scope: &'a ObservedContainerReplica,
}

impl<'a> UnbornBirthFile<'a> {
    pub(super) fn observe(
        proof: &'a VerifiedUnbornBirth,
        scope: &'a ObservedContainerReplica,
    ) -> Result<Self> {
        scope.verify()?;
        proof.require_record(&scope.record)?;
        if proof.file() != scope.file || proof.owner() != scope.record.owner_disk
            || scope.disk.seal.disk_id.parse::<DiskId>()? != proof.owner()
        {
            return Err(invalid());
        }
        Ok(Self { proof, scope })
    }

    pub(in crate::container_storage) fn require(
        &self,
        view: StateProjection<'_>,
        file: FileId,
        held: &DiskRoot,
    ) -> Result<()> {
        self.scope.verify()?;
        self.proof.require_material(view.state(), file)?;
        let record = view.state().file_containers.get(&file).ok_or_else(invalid)?;
        let replica = record.replicas.get(&self.proof.owner()).ok_or_else(invalid)?;
        if replica.entity != self.scope.entity
            || held.directory_identity(held.path())? != self.scope.identity()
            || self.scope.held.directory_identity(self.scope.held.path())? != self.scope.identity()
        {
            return Err(invalid());
        }
        Ok(())
    }
}
