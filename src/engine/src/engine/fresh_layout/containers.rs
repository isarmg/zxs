//! Ordinary FILE identity planning. Budget rows cannot escape as admitted State.
use super::FreshLayoutError as Failure;
use crate::{Config, DiskConfig, Result, State};
use crate::engine::{PlanningContext, Prepared};
use crate::disk::DiskResources;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{DiskId, FileId, Generation, HistoryTreeId, OperationResult};
use storage_contract::file_container::{ContainerEntity, ContainerReplica, FileContainerRecord};
use storage_contract::file_draft::{FileWorkingState, WorkingRevision};

mod prefixes;

fn inventory(state: &State) -> Result<BTreeMap<FileId, HistoryTreeId>> {
    let mut files = BTreeMap::new();
    for version in state.versions.values() {
        let file = version.file_id.parse::<FileId>()?;
        let history = version.history_tree_id.parse::<HistoryTreeId>()?;
        if files.insert(file, history).is_some_and(|old| old != history) {
            return Err(Failure::ContainerShape.into());
        }
    }
    Ok(files)
}

/// Structural native-snapshot check, not permission to open a named directory.
pub(crate) fn validate_state(state: &State) -> Result<()> {
    let files = inventory(state)?;
    if files.len() != state.file_containers.len() {
        return Err(Failure::ContainerShape.into());
    }
    let mut entities = BTreeSet::new();
    let mut preparations = BTreeSet::new();
    for ((disk, _), prefix) in prefixes::collect(state)? {
        if !entities.insert((disk, prefix.entity))
            || !preparations.insert((disk, prefix.preparation))
        {
            return Err(Failure::ContainerShape.into());
        }
    }
    for (file, history) in files {
        let record = state.file_containers.get(&file).ok_or(Failure::ContainerShape)?;
        if record.history_tree_id != history
            || !record.replicas.contains_key(&record.owner_disk)
            || record.working.revision().generation().get() == 0
            || super::namespace::file_target(state, file, history)? != record.namespace
            || !state.operations.get(&record.created_by)
                .is_some_and(|result| result.sequence <= state.sequence)
        {
            return Err(Failure::ContainerShape.into());
        }
        crate::engine::file_draft::validate_working_record(state, file, record)?;
        if !matches!(record.working, FileWorkingState::Automatic { .. })
            && record.replicas.len() != 1
        {
            return Err(Failure::ContainerScope.into());
        }
        for (disk, replica) in &record.replicas {
            if replica.entity.inode() == 0 || !entities.insert((*disk, replica.entity))
                || !preparations.insert((*disk, replica.preparation))
            {
                return Err(Failure::ContainerShape.into());
            }
        }
    }
    Ok(())
}

pub(crate) struct ContainerPreparation {
    next: State,
    // Scope is frozen before capacity admission and exclusive physical preparation.
    additions: BTreeMap<(FileId, DiskId), String>,
    targets: BTreeSet<(FileId, DiskId)>,
}
impl ContainerPreparation {
    pub(crate) fn new(
        context: &PlanningContext<'_>,
        mut next: State,
        prepared: &[Prepared],
        result: &OperationResult,
    ) -> Result<Self> {
        validate_state(context.state)?;
        crate::protection::verify_automatic_backup_prepared(context, &next, result, prepared)?;
        let working = crate::engine::file_draft::verify_working_transition(
            context.state, &next, result,
        )?;
        let mut expected = context.state.file_containers.clone();
        if let Some(file) = working {
            let after = next.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
            expected.get_mut(&file).ok_or(Failure::ContainerScope)?.working = after.working.clone();
        }
        if next.file_containers != expected {
            return Err(Failure::ContainerScope.into());
        }
        let before = inventory(context.state)?;
        let after = inventory(&next)?;
        if before.iter().any(|(file, history)| after.get(file) != Some(history)) {
            return Err(Failure::ContainerScope.into());
        }
        let mut known_prefixes = prefixes::collect(context.state)?;
        let mut additions = BTreeMap::new();
        let mut targets = BTreeSet::new();
        for payload in prepared {
            let version = &payload.version;
            let placement = &payload.placement;
            if next.versions.get(&version.version_id) != Some(version)
                || next.placements.get(&version.version_id) != Some(placement)
            {
                return Err(Failure::ContainerScope.into());
            }
            let file = version.file_id.parse::<FileId>()?;
            let history = version.history_tree_id.parse::<HistoryTreeId>()?;
            if !next.file_containers.contains_key(&file) {
                let first = placement.route_slot_ids.first().ok_or(Failure::ContainerScope)?;
                let disk = context.disk(first)?;
                if before.contains_key(&file) || disk.level != 1
                    || disk.upstream_slot_id.is_some() || version.version_sequence != next.sequence
                {
                    return Err(Failure::ContainerScope.into());
                }
                let namespace = super::namespace::file_target(&next, file, history)?;
                next.file_containers.insert(file, FileContainerRecord {
                    history_tree_id: history,
                    owner_disk: disk.disk_id.parse()?,
                    created_by: result.operation_id,
                    namespace,
                    working: FileWorkingState::Automatic {
                        revision: WorkingRevision::new(Generation::new(1)),
                    },
                    replicas: BTreeMap::new(),
                });
            }
            let namespace = &next.file_containers.get(&file)
                .ok_or(Failure::ContainerScope)?.namespace;
            let source = context.namespace_host_root(namespace.repository, namespace.branch)?;
            if source.disk().disk_id.parse::<DiskId>()?
                != next.file_containers[&file].owner_disk
            {
                return Err(Failure::NamespaceSource.into());
            }
            let record = next.file_containers.get_mut(&file).ok_or(Failure::ContainerScope)?;
            if record.history_tree_id != history
                || !(matches!(record.working, FileWorkingState::Automatic { .. })
                    || (working == Some(file)
                        && matches!(record.working, FileWorkingState::ManualClean { .. })
                        && context.state.file_containers.get(&file)
                            .is_some_and(|before| before.working.draft().is_some())))
            {
                return Err(Failure::ContainerScope.into());
            }
            let first = placement.route_slot_ids.first().ok_or(Failure::ContainerScope)?;
            if context.disk(first)?.disk_id.parse::<DiskId>()? != record.owner_disk {
                return Err(Failure::ContainerScope.into());
            }
            for slot in &placement.route_slot_ids {
                let disk = context.disk(slot)?;
                let id = disk.disk_id.parse::<DiskId>()?;
                let planned_prefixes = prefixes::plan(
                    context, &record.namespace, record.owner_disk, id, &mut known_prefixes,
                )?;
                targets.insert((file, id));
                let root = context.config.storage_roots.get(&id).ok_or(Failure::Registration)?;
                match record.replicas.get(&id) {
                    Some(replica) if replica.root_id != root.root_id
                        || replica.prefixes != planned_prefixes => {
                        return Err(Failure::ContainerIdentity.into());
                    }
                    Some(_) => (),
                    None => {
                        record.replicas.insert(id, ContainerReplica {
                            root_id: root.root_id,
                            prefixes: planned_prefixes,
                            preparation: storage_contract::working_tree::DirectoryId::generate()?,
                            entity: ContainerEntity::new(0, 0),
                        });
                        additions.insert((file, id), version.creation_name.clone());
                    }
                }
            }
        }
        for file in crate::container_storage::affected_file_material(context.state, &next) {
            let file = file.parse::<FileId>()?;
            if let Some(record) = next.file_containers.get(&file) {
                targets.extend(record.replicas.keys().map(|disk| (file, *disk)));
            }
        }
        let touched = targets.iter().map(|(file, _)| *file).collect::<BTreeSet<_>>();
        for file in &touched {
            if let Some(record) = context.state.file_containers.get(file) {
                targets.insert((*file, record.owner_disk));
            }
        }
        if after.len() != next.file_containers.len() {
            return Err(Failure::ContainerScope.into());
        }
        Ok(Self { next, additions, targets })
    }

    pub(crate) fn budget_state(&self) -> &State {
        &self.next
    }

    pub(crate) fn targets(&self) -> &BTreeSet<(FileId, DiskId)> {
        &self.targets
    }

    pub(crate) fn additions(&self) -> &BTreeMap<(FileId, DiskId), String> {
        &self.additions
    }

    pub(crate) fn finish(
        mut self,
        entities: &crate::container_storage::PreparedContainerEntities,
    ) -> Result<State> {
        entities.verify()?;
        for (file, disk) in self.additions.keys() {
            let entity = entities.entity(*file, *disk)?;
            let replica = self.next.file_containers.get_mut(file)
                .and_then(|record| record.replicas.get_mut(disk))
                .ok_or(Failure::ContainerScope)?;
            if replica.entity != ContainerEntity::new(0, 0) {
                return Err(Failure::ContainerScope.into());
            }
            replica.entity = entity;
        }
        // One shared prefix occurrence is fed back into every referencing row.
        // Its fixed-width shape was budgeted before any exclusive mkdir.
        for record in self.next.file_containers.values_mut() {
            for (disk, replica) in &mut record.replicas {
                for prefix in &mut replica.prefixes {
                    if prefix.entity == ContainerEntity::new(0, 0) {
                        prefix.entity = entities.prefix_entity(*disk, prefix.source_disk)?;
                    }
                }
            }
        }
        validate_state(&self.next)?;
        Ok(self.next)
    }
}

/// Same published native row, held registered disk and root identity. Locators
/// select a path only; the published entity admits the descriptor used for I/O.
pub(crate) fn verify_replica(
    config: &Config,
    resources: &DiskResources,
    state: &State,
    file: FileId,
    disk: &DiskConfig,
) -> Result<crate::disk::DiskRoot> {
    let id = disk.disk_id.parse::<DiskId>()?;
    let record = state.file_containers.get(&file).ok_or(Failure::ContainerShape)?;
    let replica = record.replicas.get(&id).ok_or(Failure::ContainerShape)?;
    let binding = config.storage_roots.get(&id).ok_or(Failure::Registration)?;
    if replica.root_id != binding.root_id || replica.entity.inode() == 0 {
        return Err(Failure::ContainerIdentity.into());
    }
    if super::namespace::file_target(state, file, record.history_tree_id)? != record.namespace {
        return Err(Failure::NamespaceSource.into());
    }
    let source = super::namespace::registered_root(
        state, config, resources, record.namespace.repository, record.namespace.branch,
    )?;
    if source.disk().disk_id.parse::<DiskId>()? != record.owner_disk {
        return Err(Failure::NamespaceSource.into());
    }
    prefixes::require_geometry(config, record, id)?;
    if id == record.owner_disk {
        return crate::container_storage::open_published_container(
            source.common()?, &record.namespace.name, replica.entity,
        );
    }
    let backup = super::namespace::read_backup_source_path(
        state, config, resources, record.namespace.repository, record.namespace.branch, id,
    )?;
    let parent = crate::container_storage::open_published_backup_parent(
        &backup, &replica.prefixes,
    )?;
    crate::container_storage::open_published_container(
        &parent, &record.namespace.name, replica.entity,
    )
}

/// A read-only replica namespace source. The actual registered owner receipt,
/// full configured direct-source chain and held roots are all required.
pub(crate) fn read_backup_source<'a>(
    config: &'a Config,
    resources: &'a DiskResources,
    state: &'a State,
    record: &FileContainerRecord,
    target: DiskId,
) -> Result<super::namespace::PreparedBackupSourcePath<'a>> {
    prefixes::require_geometry(config, record, target)?;
    let source = super::namespace::read_backup_source_path(
        state, config, resources, record.namespace.repository, record.namespace.branch, target,
    )?;
    if source.destination().source_ids().last() != Some(record.owner_disk) {
        return Err(Failure::NamespaceSource.into());
    }
    Ok(source)
}

pub(crate) fn verify_available(
    config: &Config,
    resources: &DiskResources,
    state: &State,
) -> Result<()> {
    validate_state(state)?;
    for (file, record) in state.file_containers.iter() {
        for id in record.replicas.keys() {
            let disk = config.disks.iter().find(|disk| disk.disk_id == id.to_string())
                .ok_or(Failure::Registration)?;
            if resources.available(disk).is_some() {
                verify_replica(config, resources, state, *file, disk)?;
            }
        }
    }
    Ok(())
}

/// Only the complete ordinary transition below creates this exact pair proof.
/// It can transfer physical feedback into that pair's logical reconstruction,
/// not supply an arbitrary container table or a root/install capability.
pub(crate) struct VerifiedContainerTransition<'a> {
    previous: &'a State,
    next: &'a State,
    candidate: &'a crate::journal::Candidate,
}
impl VerifiedContainerTransition<'_> {
    pub(crate) fn require_pair(
        &self, previous: &State, next: &State, candidate: &crate::journal::Candidate,
    ) -> Result<()> {
        if !std::ptr::eq(self.previous, previous) || !std::ptr::eq(self.next, next)
            || !std::ptr::eq(self.candidate, candidate)
        {
            return Err(Failure::ContainerScope.into());
        }
        Ok(())
    }

    pub(crate) fn attach_feedback(&self, reconstructed: &mut State) -> Result<()> {
        if reconstructed.file_containers != self.previous.file_containers {
            return Err(Failure::ContainerScope.into());
        }
        reconstructed.file_containers = self.next.file_containers.clone();
        Ok(())
    }
}

/// Cold ordinary transition check. Candidate evidence supplies the exact first
/// prepared order; serialized owner/root fields cannot choose a different target.
pub(crate) fn verify_transition<'a>(
    config: &Config,
    previous: &'a State,
    next: &'a State,
    candidate: &'a crate::journal::Candidate,
) -> Result<VerifiedContainerTransition<'a>> {
    validate_state(previous)?;
    validate_state(next)?;
    let result = next.operations.get(&candidate.operation_id).ok_or(Failure::ContainerScope)?;
    let working = crate::engine::file_draft::verify_working_transition(previous, next, result)?;
    let mut expected = previous.file_containers.clone();
    if let Some(file) = working {
        let after = next.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
        expected.get_mut(&file).ok_or(Failure::ContainerScope)?.working = after.working.clone();
    }
    let mut known_prefixes = prefixes::collect(previous)?;
    for evidence in &candidate.versions {
        let version = &evidence.version;
        let placement = &evidence.placement;
        if next.versions.get(&version.version_id) != Some(version)
            || next.placements.get(&version.version_id) != Some(placement)
        {
            return Err(Failure::ContainerScope.into());
        }
        let file = version.file_id.parse::<FileId>()?;
        let actual = next.file_containers.get(&file).ok_or(Failure::ContainerShape)?;
        if !expected.contains_key(&file) {
            let primary = placement.route_slot_ids.first().ok_or(Failure::ContainerScope)?;
            let disk = config.disks.iter().find(|disk| disk.slot_id.eq_ignore_ascii_case(primary))
                .ok_or(Failure::Registration)?;
            if disk.level != 1 || disk.upstream_slot_id.is_some()
                || version.version_sequence != next.sequence
            {
                return Err(Failure::ContainerScope.into());
            }
            expected.insert(file, FileContainerRecord {
                history_tree_id: version.history_tree_id.parse()?,
                owner_disk: disk.disk_id.parse()?,
                created_by: candidate.operation_id,
                namespace: super::namespace::file_target(
                    next, file, version.history_tree_id.parse()?,
                )?,
                working: FileWorkingState::Automatic {
                    revision: WorkingRevision::new(Generation::new(1)),
                },
                replicas: BTreeMap::new(),
            });
        }
        let mut record = expected.get(&file).ok_or(Failure::ContainerShape)?.clone();
        if !(matches!(record.working, FileWorkingState::Automatic { .. })
            || (working == Some(file)
                && matches!(record.working, FileWorkingState::ManualClean { .. })
                && previous.file_containers.get(&file)
                    .is_some_and(|before| before.working.draft().is_some())))
        {
            return Err(Failure::ContainerScope.into());
        }
        let first = placement.route_slot_ids.first().ok_or(Failure::ContainerScope)?;
        let first = config.disks.iter().find(|disk| disk.slot_id.eq_ignore_ascii_case(first))
            .ok_or(Failure::Registration)?;
        if first.disk_id.parse::<DiskId>()? != record.owner_disk {
            return Err(Failure::ContainerScope.into());
        }
        let mut predecessor = None;
        for slot in &placement.route_slot_ids {
            let disk = config.disks.iter().find(|disk| disk.slot_id.eq_ignore_ascii_case(slot))
                .ok_or(Failure::Registration)?;
            let id = disk.disk_id.parse::<DiskId>()?;
            let root = config.storage_roots.get(&id).ok_or(Failure::Registration)?;
            let replica = actual.replicas.get(&id).ok_or(Failure::ContainerShape)?;
            if replica.root_id != root.root_id || replica.entity.inode() == 0 {
                return Err(Failure::ContainerIdentity.into());
            }
            prefixes::require_geometry(config, actual, id)?;
            if replica.prefixes.first().map(|prefix| prefix.source_disk) != predecessor {
                return Err(Failure::ContainerScope.into());
            }
            predecessor = Some(id);
            for prefix in &replica.prefixes {
                if known_prefixes.insert((id, prefix.source_disk), prefix.clone())
                    .is_some_and(|old| old != *prefix)
                {
                    return Err(Failure::ContainerIdentity.into());
                }
            }
            // Existing complete records never retarget, even for identical bytes.
            record.replicas.entry(id).or_insert_with(|| replica.clone());
        }
        expected.insert(file, record);
    }
    if expected != next.file_containers {
        return Err(Failure::ContainerScope.into());
    }
    Ok(VerifiedContainerTransition { previous, next, candidate })
}
