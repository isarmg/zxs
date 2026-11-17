//! Owner evidence for the explicit-loss writer, not proof that bytes survived.
//! Only its parent issues this after the real plan/ACCEPT, fresh observation,
//! permanent-loss acknowledgement and installed-original checks have succeeded.
use super::RecoveryPlan;
use crate::container_storage::PreparedContainerEntities;
use crate::disk::DiskRoot;
use crate::engine::fresh_layout::FreshLayoutError as Failure;
use crate::journal::Candidate;
use crate::{Result, State};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use storage_contract::{DiskId, FileId};

pub(crate) struct AcknowledgedContainerLoss {
    previous: Arc<State>,
    next: Arc<State>,
    containers: Arc<PreparedContainerEntities>,
    new_lost: BTreeMap<FileId, BTreeSet<DiskId>>,
}

impl AcknowledgedContainerLoss {
    pub(super) fn new(
        previous: &Arc<State>,
        next: &Arc<State>,
        candidate: &Candidate,
        plan: &RecoveryPlan,
        containers: Arc<PreparedContainerEntities>,
    ) -> Result<Self> {
        let mut new_lost = BTreeMap::<FileId, BTreeSet<DiskId>>::new();
        for evidence in &candidate.versions {
            let file = evidence.version.file_id.parse::<FileId>()?;
            // A missing marker on an existing container is not BODY loss.
            if previous.file_containers.contains_key(&file) {
                continue;
            }
            let health = plan.version_protection.iter().find(|health| {
                health["version_id"] == evidence.version.version_id
            }).ok_or(Failure::ContainerScope)?;
            let slots = health["slots"].as_array().ok_or(Failure::ContainerScope)?;
            for slot in slots {
                if slot["state"] == "UNAVAILABLE" {
                    let disk = slot["disk_id"].as_str().ok_or(Failure::ContainerScope)?
                        .parse::<DiskId>()?;
                    containers.scope(file, disk)?.verify()?;
                    new_lost.entry(file).or_default().insert(disk);
                }
            }
        }
        Ok(Self {
            previous: Arc::clone(previous), next: Arc::clone(next), containers, new_lost,
        })
    }

    pub(crate) fn require_pair(&self, previous: &State, next: &State) -> Result<()> {
        if !std::ptr::eq(previous, self.previous.as_ref())
            || !std::ptr::eq(next, self.next.as_ref())
        {
            return Err(Failure::ContainerScope.into());
        }
        Ok(())
    }

    pub(crate) fn containers(&self) -> Arc<PreparedContainerEntities> {
        Arc::clone(&self.containers)
    }

    pub(crate) fn permits_unmarked(
        &self, next: &State, file: FileId, held: &DiskRoot,
    ) -> Result<bool> {
        if !std::ptr::eq(next, self.next.as_ref()) {
            return Err(Failure::ContainerScope.into());
        }
        let Some(disks) = self.new_lost.get(&file) else {
            return Ok(false);
        };
        let identity = held.directory_identity(held.path())?;
        for disk in disks {
            let scope = self.containers.scope(file, *disk)?;
            // Recheck the original held name/root after any intervening I/O;
            // matching file IDs, bytes or a path alone cannot substitute it.
            let original = scope.container()?;
            if original.directory_identity(original.path())? == identity {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
