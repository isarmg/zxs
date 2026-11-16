//! Shared replica prefixes are actual observed directories, not path authority.
use super::*;
use crate::disk_names::configuration::ConfiguredDiskNames;
use storage_contract::file_container::{ContainerAreas, ContainerNamespace, ContainerPrefix};
use storage_contract::working_tree::DirectoryId;

/// Structural address only. Names are deliberately absent: changing a Config
/// string cannot change a source area's target, direct parent or owned entities.
/// A physical consumer still needs its admitted source and held-root checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PrefixAddress {
    target: DiskId,
    source: DiskId,
    parent: Option<DiskId>,
}

struct PrefixOccurrence {
    address: PrefixAddress,
    record: ContainerPrefix,
}

/// One inventory shared by shape admission, live preparation and cold feedback.
/// Preserve the parent relation proved by collect_chain instead of discarding
/// it and later comparing only the leaf's inode/areas under another parent.
#[derive(Default)]
pub(super) struct Prefixes {
    occurrences: BTreeMap<(DiskId, DiskId), PrefixOccurrence>,
}

impl Prefixes {
    fn insert(&mut self, address: PrefixAddress, record: &ContainerPrefix) -> bool {
        let key = (address.target, address.source);
        if let Some(previous) = self.occurrences.get(&key) {
            return previous.address == address && previous.record == *record;
        }
        self.occurrences.insert(key, PrefixOccurrence { address, record: record.clone() });
        true
    }

    fn at(
        &self, target: DiskId, parent: Option<DiskId>, source: DiskId,
    ) -> Result<Option<&ContainerPrefix>> {
        let Some(previous) = self.occurrences.get(&(target, source)) else {
            return Ok(None);
        };
        if previous.address != (PrefixAddress { target, source, parent }) {
            return Err(Failure::ContainerIdentity.into());
        }
        Ok(Some(&previous.record))
    }

    pub(super) fn extend_chain(&mut self, target: DiskId, chain: &[ContainerPrefix]) -> Result<()> {
        let mut parent = None;
        for record in chain {
            let source = record.source_disk;
            if !self.insert(PrefixAddress { target, source, parent }, record) {
                return Err(Failure::ContainerIdentity.into());
            }
            parent = Some(source);
        }
        Ok(())
    }

    pub(super) fn into_records(self) -> impl Iterator<Item = ((DiskId, DiskId), ContainerPrefix)> {
        self.occurrences.into_iter().map(|(key, occurrence)| (key, occurrence.record))
    }
}

pub(super) fn collect(state: &State) -> Result<Prefixes> {
    let mut result = Prefixes::default();
    for record in state.file_containers.values() {
        for (disk, replica) in &record.replicas {
            collect_chain(*disk, record.owner_disk, &replica.prefixes, &mut result)?;
        }
    }
    for record in state.namespace_directories.values() {
        for (disk, replica) in &record.replicas {
            collect_chain(*disk, record.owner_disk, &replica.source_prefixes,
                &mut result)?;
        }
    }
    Ok(result)
}

fn collect_chain(
    disk: DiskId, owner: DiskId, chain: &[ContainerPrefix], result: &mut Prefixes,
) -> Result<()> {
    if (disk == owner) != chain.is_empty()
        || (!chain.is_empty() && chain.last().map(|prefix| prefix.source_disk) != Some(owner))
    {
        return Err(Failure::ContainerShape.into());
    }
    let mut parent = None;
    let mut seen = BTreeSet::new();
    for prefix in chain {
        if !seen.insert(prefix.source_disk) || prefix.source_disk == disk
            || prefix.entity.inode() == 0
            || prefix.areas.entities().iter().any(|entity| {
                entity.inode() == 0 || entity.device() != prefix.entity.device()
            })
            || !result.insert(PrefixAddress {
                target: disk, source: prefix.source_disk, parent,
            }, prefix)
        {
            return Err(Failure::ContainerShape.into());
        }
        parent = Some(prefix.source_disk);
    }
    Ok(())
}

pub(super) fn plan(
    context: &PlanningContext<'_>,
    namespace: &ContainerNamespace,
    owner: DiskId,
    target: DiskId,
    known: &mut Prefixes,
) -> Result<Vec<ContainerPrefix>> {
    plan_source(context, namespace.repository, namespace.branch, owner, target, known)
}

pub(super) fn plan_source(
    context: &PlanningContext<'_>, repository: storage_contract::RepositoryId,
    branch: storage_contract::BranchId, owner: DiskId, target: DiskId,
    known: &mut Prefixes,
) -> Result<Vec<ContainerPrefix>> {
    if owner == target {
        return Ok(Vec::new());
    }
    let source = context.prepare_backup_source_path(
        repository, branch, target,
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
    let mut parent = None;
    for source_disk in source.destination().source_ids() {
        let prefix = match known.at(target, parent, source_disk)? {
            Some(prefix) => prefix.clone(),
            None => {
                let prefix = ContainerPrefix {
                    source_disk,
                    preparation: DirectoryId::generate()?,
                    entity: ContainerEntity::new(0, 0),
                    areas: ContainerAreas::budget(),
                };
                if !known.insert(PrefixAddress {
                    target, source: source_disk, parent,
                }, &prefix) {
                    return Err(Failure::ContainerIdentity.into());
                }
                prefix
            }
        };
        result.push(prefix);
        parent = Some(source_disk);
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
    require_source_geometry(config, record.owner_disk, target, replica.root_id, &replica.prefixes)
}

pub(in crate::engine::fresh_layout) fn require_source_geometry(
    config: &Config, owner: DiskId, target: DiskId,
    root: storage_contract::working_tree::RootId, prefixes: &[ContainerPrefix],
) -> Result<()> {
    let names = ConfiguredDiskNames::new(config).map_err(crate::Error::ConfiguredDiskName)?;
    let path = names.source_path(target).map_err(crate::Error::ConfiguredDiskName)?;
    let binding = config.storage_roots.get(&target).ok_or(Failure::Registration)?;
    if path.source_ids().len() > 2 {
        return Err(Failure::NamespaceUnmapped.into());
    }
    if root != binding.root_id
        || binding.registration != path.target_registration()
        || binding.source_chain != path.source_registrations().collect::<Vec<_>>()
        || prefixes.iter().map(|prefix| prefix.source_disk).collect::<Vec<_>>()
            != path.source_ids().collect::<Vec<_>>()
        || (target != owner && path.source_ids().last() != Some(owner))
    {
        return Err(Failure::Registration.into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/source_prefix_inventory_tests.rs"]
mod tests;
