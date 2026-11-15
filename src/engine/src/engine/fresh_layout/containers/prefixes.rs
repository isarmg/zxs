//! Shared replica prefixes are actual observed directories, not path authority.
use super::*;
use crate::disk_names::configuration::ConfiguredDiskNames;
use storage_contract::file_container::{ContainerAreas, ContainerNamespace, ContainerPrefix};
use storage_contract::working_tree::DirectoryId;

pub(super) type Prefixes = BTreeMap<(DiskId, DiskId), ContainerPrefix>;

pub(super) fn collect(state: &State) -> Result<Prefixes> {
    let mut result = BTreeMap::new();
    let mut parents = BTreeMap::new();
    for record in state.file_containers.values() {
        for (disk, replica) in &record.replicas {
            if (*disk == record.owner_disk) != replica.prefixes.is_empty()
                || (!replica.prefixes.is_empty()
                    && replica.prefixes.last().map(|prefix| prefix.source_disk)
                        != Some(record.owner_disk))
            {
                return Err(Failure::ContainerShape.into());
            }
            let mut parent = None;
            let mut seen = BTreeSet::new();
            for prefix in &replica.prefixes {
                let key = (*disk, prefix.source_disk);
                if !seen.insert(prefix.source_disk) || prefix.source_disk == *disk
                    || prefix.entity.inode() == 0
                    || prefix.areas.entities().iter().any(|entity| {
                        entity.inode() == 0 || entity.device() != prefix.entity.device()
                    })
                    || parents.insert(key, parent).is_some_and(|old| old != parent)
                    || result.insert(key, prefix.clone()).is_some_and(|old| old != *prefix)
                {
                    return Err(Failure::ContainerShape.into());
                }
                parent = Some(prefix.source_disk);
            }
        }
    }
    Ok(result)
}

pub(super) fn plan(
    context: &PlanningContext<'_>,
    namespace: &ContainerNamespace,
    owner: DiskId,
    target: DiskId,
    known: &mut Prefixes,
) -> Result<Vec<ContainerPrefix>> {
    if owner == target {
        return Ok(Vec::new());
    }
    let source = context.prepare_backup_source_path(
        namespace.repository, namespace.branch, target,
    )?;
    source.require_context(context)?;
    // Each hop is copied from the preceding installed BODY descriptor. The
    // ordinary slice admits B and C through the same ordered held-parent chain.
    if source.destination().source_ids().len() > 2 {
        return Err(Failure::NamespaceUnmapped.into());
    }
    if source.destination().source_ids().last() != Some(owner) {
        return Err(Failure::NamespaceSource.into());
    }
    let mut result = Vec::new();
    for source_disk in source.destination().source_ids() {
        let key = (target, source_disk);
        let prefix = match known.get(&key) {
            Some(prefix) => prefix.clone(),
            None => {
                let prefix = ContainerPrefix {
                    source_disk,
                    preparation: DirectoryId::generate()?,
                    entity: ContainerEntity::new(0, 0),
                    areas: ContainerAreas::budget(),
                };
                known.insert(key, prefix.clone());
                prefix
            }
        };
        result.push(prefix);
    }
    Ok(result)
}

/// Cold structural equality with the exact selected registered chain. This
/// returns no root; the physical reader separately consumes the namespace source.
pub(super) fn require_geometry(
    config: &Config,
    record: &FileContainerRecord,
    target: DiskId,
) -> Result<()> {
    let replica = record.replicas.get(&target).ok_or(Failure::ContainerShape)?;
    let names = ConfiguredDiskNames::new(config).map_err(crate::Error::ConfiguredDiskName)?;
    let path = names.source_path(target).map_err(crate::Error::ConfiguredDiskName)?;
    let binding = config.storage_roots.get(&target).ok_or(Failure::Registration)?;
    if path.source_ids().len() > 2 {
        return Err(Failure::NamespaceUnmapped.into());
    }
    if replica.root_id != binding.root_id
        || binding.registration != path.target_registration()
        || binding.source_chain != path.source_registrations().collect::<Vec<_>>()
        || replica.prefixes.iter().map(|prefix| prefix.source_disk).collect::<Vec<_>>()
            != path.source_ids().collect::<Vec<_>>()
        || (target != record.owner_disk && path.source_ids().last() != Some(record.owner_disk))
    {
        return Err(Failure::Registration.into());
    }
    Ok(())
}
