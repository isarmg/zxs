//! Ordinary FILE identity planning. Budget rows cannot escape as admitted State.
use super::FreshLayoutError as Failure;
use crate::{Config, DiskConfig, Result, State};
use crate::engine::{PlanningContext, Prepared};
use crate::disk::DiskResources;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{DiskId, FileId, HistoryTreeId, OperationResult};
use storage_contract::file_container::{ContainerEntity, ContainerReplica, FileContainerRecord};
use storage_contract::file_draft::FileWorkingState;

pub(super) mod prefixes;
mod rename;
mod unborn;
mod working_batch;
pub(crate) use working_batch::verify_directory_transition;
pub(crate) use unborn::{VerifiedUnbornBirth, VerifiedUnbornFirst};
pub(crate) use rename::ContainerNameChange;

fn inventory(state: &State) -> Result<BTreeMap<FileId, HistoryTreeId>> {
    let mut files = BTreeMap::new();
    for version in state.versions.values() {
        let file = version.file_id.parse::<FileId>()?;
        let history = version.history_tree_id.parse::<HistoryTreeId>()?;
        if files.insert(file, history).is_some_and(|old| old != history) {
            return Err(Failure::ContainerShape.into());
        }
    }
    for (file, record) in state.file_containers.iter() {
        if record.working.unborn().is_some()
            && files.insert(*file, record.history_tree_id).is_some()
        {
            return Err(Failure::ContainerShape.into());
        }
    }
    Ok(files)
}

/// Structural native-snapshot check, not permission to open a named directory.
pub(crate) fn validate_state(state: &State) -> Result<()> {
    super::directories::validate_state(state)?;
    let current = crate::business::file_draft::unborn::validate_current(state, usize::MAX)?;
    let files = inventory(state)?;
    if files.len() != state.file_containers.len() {
        return Err(Failure::ContainerShape.into());
    }
    let mut entities = BTreeSet::new();
    let mut preparations = BTreeSet::new();
    for record in state.namespace_directories.values() {
        for (disk, replica) in &record.replicas {
            if !entities.insert((*disk, replica.entity))
                || !preparations.insert((*disk, replica.preparation))
            {
                return Err(Failure::ContainerShape.into());
            }
        }
    }
    for ((disk, _), prefix) in prefixes::collect(state)?.into_records() {
        if std::iter::once(prefix.entity).chain(prefix.areas.entities())
            .any(|entity| !entities.insert((disk, entity)))
            || !preparations.insert((disk, prefix.preparation))
        {
            return Err(Failure::ContainerShape.into());
        }
    }
    for (file, history) in files {
        let record = state.file_containers.get(&file).ok_or(Failure::ContainerShape)?;
        // A parsed lifecycle label is not the missing original move/restore.
        // Leave new physical states fenced until their actual cold transition
        // and independent FILE/DISK/SYSTEM installer can issue them.
        if !matches!(record.site,storage_contract::file_container::site::FileSite::Created {})
            || record.history_tree_id != history
            || !record.replicas.contains_key(&record.owner_disk)
            || record.working.revision().generation().get() == 0
            || (!current.contains(&file)
                && super::namespace::file_target(state, file, history)? != record.namespace)
            || !state.operations.get(&record.created_by)
                .is_some_and(|result| result.sequence <= state.sequence)
        {
            return Err(Failure::ContainerShape.into());
        }
        crate::engine::file_draft::validate_working_record(state, file, record)?;
        if !matches!(record.working, FileWorkingState::ManualClean { .. })
            && record.replicas.len() != 1
        {
            return Err(Failure::ContainerScope.into());
        }
        let parents = match &record.namespace.parent {
            storage_contract::file_container::ContainerParent::Worktree { .. }
            | storage_contract::file_container::ContainerParent::Root => Vec::new(),
            storage_contract::file_container::ContainerParent::Directory(entry) => {
                super::directories::chain(state, *entry)?
            }
        };
        for (disk, replica) in &record.replicas {
            if replica.entity.inode() == 0 || !entities.insert((*disk, replica.entity))
                || !preparations.insert((*disk, replica.preparation))
            {
                return Err(Failure::ContainerShape.into());
            }
            for parent in &parents {
                let occurrence = parent.replicas.get(disk).ok_or(Failure::NamespaceUnmapped)?;
                if parent.repository != record.namespace.repository
                    || parent.branch != record.namespace.branch
                    || parent.owner_disk != record.owner_disk
                    || occurrence.root_id != replica.root_id
                    || occurrence.source_prefixes != replica.prefixes
                    || replica.prefixes.iter().map(|prefix| prefix.source_disk.to_string())
                        .ne(occurrence.source_chain.iter().map(|source| source.disk_id.clone()))
                {
                    return Err(Failure::NamespaceSource.into());
                }
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
    directory_additions: BTreeSet<(storage_contract::EntryId, DiskId)>,
    directory_targets: BTreeSet<(storage_contract::EntryId, DiskId)>,
    renames: BTreeMap<FileId, ContainerNameChange>,
    birth: Option<std::sync::Arc<VerifiedUnbornBirth>>,
    first: BTreeMap<FileId, std::sync::Arc<VerifiedUnbornFirst>>,
    draft_reads: Vec<std::sync::Arc<crate::container_storage::ReadDraftDigest>>,
}
impl ContainerPreparation {
    pub(crate) fn new(
        context: &PlanningContext<'_>,
        mut next: State,
        prepared: &[Prepared],
        result: &OperationResult,
        draft_read: Option<&std::sync::Arc<crate::container_storage::ReadDraftDigest>>,
    ) -> Result<Self> {
        validate_state(context.state)?;
        crate::engine::workspace_provenance::prepared(context, &next, result, prepared)?;
        crate::persist::file_adoption::transition(context.state, &next, result, !prepared.is_empty())?;
        // Every old writer must still see Unborn names as occupied. This pure
        // union check precedes capacity admission and all physical preparation.
        let mut current_repositories = BTreeSet::new();
        for record in next.file_containers.values().filter(|record| {
            record.working.unborn().is_some()
        }) {
            if current_repositories.insert(record.namespace.repository) {
                crate::business::namespace::working::WorkingNamespace::from_main_state(
                    &next, record.namespace.repository, context.config.max_entries,
                )?;
            }
        }
        let automatic_backup = crate::protection::verify_automatic_backup_prepared(
            context, &next, result, prepared,
        )?;
        let directory_backup = automatic_backup.as_ref().and_then(|proof| proof.directories());
        let birth = unborn::plan(context, &next, prepared, result)?.map(std::sync::Arc::new);
        let saving = unborn::save_plan(context, &next, prepared, result)?;
        let first = unborn::first_plan(context, &next, prepared, result)?;
        let working = match (&birth, &saving, &first) {
            (Some(_), None, None) => None,
            (None, Some(proof), None) => Some(proof.file()),
            (None, None, Some(proof)) => Some(proof.file()),
            (None, None, None) => crate::engine::file_draft::verify_working_transition(
                context.state, &next, result, draft_read.map(AsRef::as_ref),
            )?,
            _ => return Err(Failure::ContainerScope.into()),
        };
        let mut expected = context.state.file_containers.clone();
        if let Some(birth) = &birth {
            expected.insert(birth.file(), next.file_containers[&birth.file()].clone());
        }
        if let Some(file) = working {
            let after = next.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
            expected.get_mut(&file).ok_or(Failure::ContainerScope)?.working = after.working.clone();
        }
        if next.file_containers != expected {
            return Err(Failure::ContainerScope.into());
        }
        if next.namespace_directories != context.state.namespace_directories {
            return Err(Failure::ContainerScope.into());
        }
        let renames = rename::project(context.state, &next, result)?;
        if !renames.is_empty()
            && (context.current_namespaces.is_some() || context.config.generation().is_some()
                || !prepared.is_empty())
        {
            return Err(Failure::WholeContainerLifecycleUnsupported.into());
        }
        for (file, change) in &renames {
            next.file_containers.get_mut(file).ok_or(Failure::ContainerScope)?
                .namespace = change.target().clone();
        }
        let (directories, directory_additions) = super::directories::project(
            context.config, context.state, &next, result, None,
        )?;
        if !directory_additions.is_empty()
            && (context.current_namespaces.is_some() || context.config.generation().is_some())
        {
            return Err(Failure::NamespaceUnmapped.into());
        }
        next.namespace_directories = directories;
        let mut directory_additions = directory_additions.into_iter().map(|entry| {
            let record = &next.namespace_directories[&entry.to_string()];
            (entry, record.owner_disk)
        }).collect::<BTreeSet<_>>();
        let before = inventory(context.state)?;
        let after = inventory(&next)?;
        if before.iter().any(|(file, history)| after.get(file) != Some(history)) {
            return Err(Failure::ContainerScope.into());
        }
        let mut known_prefixes = prefixes::collect(context.state)?;
        let mut additions = BTreeMap::new();
        let mut targets = BTreeSet::new();
        if let Some(birth) = &birth {
            unborn::add_owner(context, &mut next, birth)?;
            let name = next.file_containers[&birth.file()].namespace.name.as_str().to_owned();
            additions.insert((birth.file(), birth.owner()), name);
            targets.insert((birth.file(), birth.owner()));
        }
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
                    site: storage_contract::file_container::site::FileSite::Created {},
                    history_tree_id: history,
                    owner_disk: disk.disk_id.parse()?,
                    created_by: result.operation_id,
                    namespace,
                    working: FileWorkingState::initial_manual(),
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
                || !(((!before.contains_key(&file) && record.working == FileWorkingState::initial_manual())
                    || (result.kind == "workspace.commit" && record.working.draft().is_none()
                        && record.working.unborn().is_none()))
                    || (matches!(record.working, FileWorkingState::Automatic { body: storage_contract::file_draft::AutomaticBody::Clean {}, .. })
                        && working == Some(file))
                    || (working == Some(file)
                        && matches!(record.working, FileWorkingState::ManualClean { .. })
                        && (context.state.file_containers.get(&file)
                            .is_some_and(|before| before.working.draft().is_some())
                            || first.as_ref().is_some_and(|proof| proof.file() == file))))
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
        let mut directories = next.namespace_directories.clone();
        directory_additions.extend(super::directories::project_replicas(
            context.config, &next, result, &mut directories, None,
        )?);
        if let Some(selected) = directory_backup {
            directory_additions.extend(super::directories::project_directory_backups(
                context.config, &next, result, &mut directories, selected, None,
                |record, disk| prefixes::plan_source(context, record.repository, record.branch,
                    record.owner_disk, disk, &mut known_prefixes),
            )?);
        }
        if !directory_additions.is_empty()
            && (context.current_namespaces.is_some()
                || (context.config.generation().is_some() && automatic_backup.is_none()))
        {
            return Err(Failure::NamespaceUnmapped.into());
        }
        next.namespace_directories = directories;
        let mut directory_targets = directory_additions.clone();
        if let Some(selected) = directory_backup {
            for disk in selected.route() {
                directory_targets.extend(selected.entries().map(|entry| (entry, *disk)));
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
        Ok(Self {
            next, additions, targets, directory_additions, directory_targets, renames, birth,
            first: first.into_iter().map(|proof| (proof.file(), proof)).collect(),
            draft_reads: draft_read.into_iter().cloned().collect(),
        })
    }

    pub(crate) fn birth(&self) -> Option<&std::sync::Arc<VerifiedUnbornBirth>> {
        self.birth.as_ref()
    }

    pub(crate) fn draft_reads(&self)
        -> &[std::sync::Arc<crate::container_storage::ReadDraftDigest>]
    {
        &self.draft_reads
    }

    pub(crate) fn first_sources(
        &self,
    ) -> impl Iterator<Item = &std::sync::Arc<VerifiedUnbornFirst>> {
        self.first.values()
    }

    pub(crate) fn rename(&self, file: FileId) -> Option<&ContainerNameChange> {
        self.renames.get(&file)
    }

    pub(crate) fn needs_entity_feedback(&self) -> bool {
        !self.additions.is_empty() || !self.directory_additions.is_empty()
    }

    pub(crate) fn directory_targets(&self)
        -> &BTreeSet<(storage_contract::EntryId, DiskId)>
    {
        &self.directory_targets
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
        for (entry, disk) in &self.directory_additions {
            let record = self.next.namespace_directories.get_mut(&entry.to_string())
                .ok_or(Failure::ContainerScope)?;
            let observed = entities.directory_record(record.entry, *disk)?;
            let expected = record.replicas.get_mut(disk)
                .ok_or(Failure::ContainerScope)?;
            if expected.entity.inode() != 0 || expected.preparation != observed.preparation {
                return Err(Failure::ContainerScope.into());
            }
            *expected = observed;
        }
        let mut updated = BTreeMap::new();
        for (file, disk) in self.additions.keys() {
            let entity = entities.entity(*file, *disk)?;
            let record = match updated.entry(*file) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => entry.insert(
                    self.next.file_containers.get(file).ok_or(Failure::ContainerScope)?.clone(),
                ),
            };
            let replica = record.replicas.get_mut(disk).ok_or(Failure::ContainerScope)?;
            if replica.entity != ContainerEntity::new(0, 0) {
                return Err(Failure::ContainerScope.into());
            }
            replica.entity = entity;
        }
        // Only newly added replicas can contain unbound prefixes: the admitted
        // previous rows were complete and planning preserved all existing replicas.
        // Feed back every new occurrence without scanning or dirtying other FILE rows.
        for record in updated.values_mut() {
            for (disk, replica) in &mut record.replicas {
                for prefix in &mut replica.prefixes {
                    if prefix.entity == ContainerEntity::new(0, 0) {
                        *prefix = entities.prefix_record(*disk, prefix.source_disk)?;
                    }
                }
            }
        }
        for (file, record) in updated {
            self.next.file_containers.insert(file, record);
        }
        validate_state(&self.next)?;
        Ok(self.next)
    }
}

/// The complete published record and actual registered namespace issue this
/// short-lived source together. It has no constructor outside this module.
pub(crate) struct PublishedReplicaSource<'a> {
    file: FileId,
    record: &'a FileContainerRecord,
    location: ReplicaReadLocation<'a>,
    handle: &'a std::sync::Arc<crate::disk::DiskHandle>,
    namespace: ReplicaNamespace<'a>,
    state: &'a State,
    config: &'a Config,
    resources: &'a DiskResources,
}
#[derive(Clone, Copy)]
pub(crate) enum ReplicaReadLocation<'a> {
    Published(&'a name_policy::LookupName),
    Finalized {
        source: &'a name_policy::LookupName,
        target: &'a name_policy::LookupName,
        sequence: storage_contract::Sequence,
    },
}
pub(crate) enum ReplicaNamespace<'a> {
    Owner(super::namespace::RegisteredNamespaceRoot<'a>),
    Worktree(super::namespace::RegisteredNamespaceRoot<'a>),
    Backup(super::namespace::PublishedBackupSourcePath<'a>),
}
impl<'a> PublishedReplicaSource<'a> {
    pub(crate) fn location(&self) -> ReplicaReadLocation<'a> {
        self.location
    }

    pub(crate) fn config(&self) -> &'a Config {
        self.config
    }
    pub(crate) fn resources(&self) -> &'a DiskResources {
        self.resources
    }

    pub(crate) fn into_parts(self) -> (
        FileId, &'a FileContainerRecord, &'a std::sync::Arc<crate::disk::DiskHandle>,
        ReplicaNamespace<'a>, &'a State,
    ) {
        (self.file, self.record, self.handle, self.namespace, self.state)
    }
}

/// Current/retained is the whole-container lifecycle, not version headness.
/// The retained transaction has not been implemented for this physical format.
pub(crate) fn require_current_container(state: &State, file: FileId) -> Result<()> {
    if state.file_deletions.contains_key(&file.to_string()) || state.file_containers.get(&file)
        .is_some_and(|r| !matches!(r.site,storage_contract::file_container::site::FileSite::Created {})) {
        return Err(Failure::WholeContainerLifecycleUnsupported.into());
    }
    Ok(())
}

/// Published read observation retains the exact prefix, area and container
/// descriptors. It cannot observe an unpublished preparation or install it.
pub(crate) fn observe_replica(
    config: &Config,
    resources: &DiskResources,
    state: &State,
    file: FileId,
    disk: &DiskConfig,
) -> Result<crate::container_storage::ObservedContainerReplica> {
    crate::container_storage::ObservedContainerReplica::observe(published_source(
        config, resources, state, file, disk,
    )?)
}

/// Aggregate readers share this same admission and their one descriptor budget;
/// they retain the resulting physical occurrences rather than observing twice.
pub(crate) fn published_source<'a>(
    config: &'a Config,
    resources: &'a DiskResources,
    state: &'a State,
    file: FileId,
    disk: &DiskConfig,
) -> Result<PublishedReplicaSource<'a>> {
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
    require_current_container(state, file)?;
    let namespace = if id == record.owner_disk {
        let source = super::namespace::registered_root(
            state, config, resources, record.namespace.repository, record.namespace.branch,
        )?;
        if source.disk().disk_id.parse::<DiskId>()? != record.owner_disk {
            return Err(Failure::NamespaceSource.into());
        }
        prefixes::require_geometry(config, record, id)?;
        if record.namespace.parent.is_worktree() {
            source.deleted()?;
            ReplicaNamespace::Worktree(source)
        } else {
            ReplicaNamespace::Owner(source)
        }
    } else {
        if record.namespace.parent.is_worktree() {
            return Err(Failure::ContainerScope.into());
        }
        let source = published_backup_source(config, resources, state, record, id)?;
        ReplicaNamespace::Backup(source)
    };
    Ok(PublishedReplicaSource {
        file, record, location: ReplicaReadLocation::Published(&record.namespace.name),
        handle: resources.get(disk)?, namespace, state, config, resources,
    })
}

/// Only the verified recovery cursor may displace an original historical name.
/// The record, namespace owner, parent and source registrations remain original.
pub(in crate::engine) fn observe_historical_replica(
    config: &Config,
    resources: &DiskResources,
    state: &State,
    file: FileId,
    disk: &DiskConfig,
    locations: &crate::engine::recovery::container_locations::ReplayContainerLocations,
) -> Result<crate::container_storage::ObservedContainerReplica> {
    let mut source = published_source(config, resources, state, file, disk)?;
    use crate::engine::recovery::container_locations::ReplayName;
    if let Some(location) = locations.historical_location(state, file, disk.disk_id.parse()?)? {
        source.location = match location {
            ReplayName::Published(name) => ReplicaReadLocation::Published(name),
            ReplayName::Finalized { source, target, sequence } => {
                ReplicaReadLocation::Finalized { source, target, sequence }
            }
        };
    }
    crate::container_storage::ObservedContainerReplica::observe(source)
}

/// A loan of this invocation's admitted replay stream, never a caller supplied
/// flag. It selects two names only for a pair with actual all-member FINALIZE.
pub(crate) struct RecoveryRenameLocations<'a> {
    previous: &'a State,
    next: &'a State,
    candidate: &'a crate::journal::Candidate,
    locations: &'a crate::engine::recovery::container_locations::ReplayContainerLocations,
}

pub(in crate::engine) fn recovery_rename_locations<'a>(
    previous: &'a State,
    next: &'a State,
    candidate: &'a crate::journal::Candidate,
    locations: &'a crate::engine::recovery::container_locations::ReplayContainerLocations,
) -> RecoveryRenameLocations<'a> {
    RecoveryRenameLocations { previous, next, candidate, locations }
}

impl RecoveryRenameLocations<'_> {
    fn require_pair(
        &self, previous: &State, next: &State, candidate: &crate::journal::Candidate,
    ) -> Result<()> {
        if !std::ptr::eq(previous, self.previous) || !std::ptr::eq(next, self.next)
            || !std::ptr::eq(candidate, self.candidate)
        {
            return Err(Failure::ContainerScope.into());
        }
        Ok(())
    }

    /// The earlier Unborn record selects its original immutable extents inside
    /// the same inode. Only this admitted replay loan can resolve a later name.
    fn observe_previous(
        &self,
        config: &Config,
        resources: &DiskResources,
        previous: &State,
        next: &State,
        candidate: &crate::journal::Candidate,
        file: FileId,
        disk: &DiskConfig,
    ) -> Result<crate::container_storage::ObservedContainerReplica> {
        self.require_pair(previous, next, candidate)?;
        observe_historical_replica(config, resources, previous, file, disk, self.locations)
    }

    pub(crate) fn permits_target(
        &self, previous: &State, next: &State, candidate: &crate::journal::Candidate,
        file: FileId, disk: DiskId,
    ) -> Result<bool> {
        self.require_pair(previous, next, candidate)?;
        let old = previous.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
        let new = next.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
        use crate::engine::recovery::container_locations::ReplayName;
        Ok(matches!(self.locations.historical_location(next, file, disk)?,
            Some(ReplayName::Finalized { source, target, sequence })
                if source == &old.namespace.name && target == &new.namespace.name
                    && sequence.get() == candidate.sequence))
    }
}

pub(crate) fn verify_replica(
    config: &Config,
    resources: &DiskResources,
    state: &State,
    file: FileId,
    disk: &DiskConfig,
) -> Result<()> {
    observe_replica(config, resources, state, file, disk).map(|_| ())
}

/// The same permanent namespace registration, borrowed from the actual replay
/// prefix. This does not accept a next snapshot as its own owner authority.
pub(crate) fn read_container_owner<'a>(
    config: &'a Config,
    resources: &'a DiskResources,
    previous: &'a State,
    record: &FileContainerRecord,
) -> Result<super::namespace::RegisteredNamespaceRoot<'a>> {
    let source = super::namespace::registered_root(
        previous, config, resources, record.namespace.repository, record.namespace.branch,
    )?;
    if source.disk().disk_id.parse::<DiskId>()? != record.owner_disk {
        return Err(Failure::NamespaceSource.into());
    }
    Ok(source)
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

/// Published-only physical read. The admitted State still fixes the owner,
/// route and every source registration; no ancestor disk is reopened as proof
/// of an already complete destination. Candidate/prepare use read_backup_source.
pub(crate) fn published_backup_source<'a>(
    config: &'a Config, resources: &'a DiskResources, state: &'a State,
    record: &FileContainerRecord, target: DiskId,
) -> Result<super::namespace::PublishedBackupSourcePath<'a>> {
    prefixes::require_geometry(config, record, target)?;
    let source = super::namespace::published_backup_source_path(
        state, config, resources, record.namespace.repository, record.namespace.branch, target,
    )?;
    if source.owner() != record.owner_disk {
        return Err(Failure::NamespaceSource.into());
    }
    source.require_source(state, config, resources)?;
    Ok(source)
}

pub(crate) fn verify_available(
    config: &Config,
    resources: &DiskResources,
    state: &State,
) -> Result<()> {
    verify_available_at(config, resources, state, None)
}

pub(in crate::engine) fn verify_historical_available(
    config: &Config,
    resources: &DiskResources,
    state: &State,
    locations: &crate::engine::recovery::container_locations::ReplayContainerLocations,
) -> Result<()> {
    verify_available_at(config, resources, state, Some(locations))
}

fn verify_available_at(
    config: &Config,
    resources: &DiskResources,
    state: &State,
    locations: Option<&crate::engine::recovery::container_locations::ReplayContainerLocations>,
) -> Result<()> {
    validate_state(state)?;
    for record in state.namespace_directories.values() {
        for id in record.replicas.keys() {
            let disk = config.disks.iter().find(|disk| disk.disk_id == id.to_string())
                .ok_or(Failure::Registration)?;
            if resources.available(disk).is_some() {
                crate::container_storage::verify_ordinary_directory(
                    config, resources, state, record.entry, *id,
                )?;
            }
        }
    }
    for (file, record) in state.file_containers.iter() {
        for id in record.replicas.keys() {
            let disk = config.disks.iter().find(|disk| disk.disk_id == id.to_string())
                .ok_or(Failure::Registration)?;
            if resources.available(disk).is_some() {
                match locations {
                    Some(locations) => {
                        let scope = observe_historical_replica(
                            config, resources, state, *file, disk, locations,
                        )?;
                        if locations.requires_complete_history(state, *file) {
                            for version in state.versions.values().filter(|version| {
                                version.file_id == file.to_string()
                                    && state.content_states.get(&version.version_id)
                                        == Some(&crate::ContentState::Available)
                                    && state.placements.get(&version.version_id)
                                        .is_some_and(|place| {
                                            place.route_slot_ids.iter().any(|slot| {
                                                slot.eq_ignore_ascii_case(&disk.slot_id)
                                            })
                                        })
                            }) {
                                scope.verify_version_body(version)?;
                            }
                        }
                    }
                    None => verify_replica(config, resources, state, *file, disk)?,
                }
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
    renames: BTreeMap<FileId, ContainerNameChange>,
    birth: Option<std::sync::Arc<VerifiedUnbornBirth>>,
    first: BTreeMap<FileId, std::sync::Arc<VerifiedUnbornFirst>>,
    draft_reads: Vec<std::sync::Arc<crate::container_storage::ReadDraftDigest>>,
}
impl VerifiedContainerTransition<'_> {
    pub(crate) fn birth(&self) -> Option<&std::sync::Arc<VerifiedUnbornBirth>> {
        self.birth.as_ref()
    }

    pub(crate) fn draft_reads(&self)
        -> &[std::sync::Arc<crate::container_storage::ReadDraftDigest>]
    {
        &self.draft_reads
    }

    pub(crate) fn first_sources(
        &self,
    ) -> impl Iterator<Item = &std::sync::Arc<VerifiedUnbornFirst>> {
        self.first.values()
    }

    pub(in crate::engine) fn renames(&self) -> &BTreeMap<FileId, ContainerNameChange> {
        &self.renames
    }

    pub(crate) fn require_pair(
        &self, previous: &State, next: &State, candidate: &crate::journal::Candidate,
    ) -> Result<()> {
        if !std::ptr::eq(self.previous, previous) || !std::ptr::eq(self.next, next)
            || !std::ptr::eq(self.candidate, candidate)
        {
            return Err(Failure::ContainerScope.into());
        }
        for read in &self.draft_reads { read.require_source()?; }
        Ok(())
    }

    pub(crate) fn attach_feedback(&self, reconstructed: &mut State) -> Result<()> {
        if reconstructed.file_containers != self.previous.file_containers
            || reconstructed.namespace_directories != self.previous.namespace_directories
        {
            return Err(Failure::ContainerScope.into());
        }
        reconstructed.file_containers = self.next.file_containers.clone();
        reconstructed.namespace_directories = self.next.namespace_directories.clone();
        Ok(())
    }
}

/// First commit alone needs the original immutable working extents. Neither a
/// serialized FULL hash nor the existence of a later Version supplies them.
/// The ordinary pure verifier remains unchanged for every other operation.
pub(crate) fn verify_transition_with_working_source<'a>(
    config: &crate::ValidatedConfig,
    resources: &std::sync::Arc<DiskResources>,
    previous: &'a State,
    next: &'a State,
    candidate: &'a crate::journal::Candidate,
    locations: Option<&RecoveryRenameLocations<'_>>,
) -> Result<VerifiedContainerTransition<'a>> {
    let result = next.operations.get(&candidate.operation_id).ok_or(Failure::ContainerScope)?;
    if result.kind == "file.draft.commit" {
        let decoded = crate::persist::file_draft::decode(previous, result)?;
        let target = decoded.outcome().request.target();
        let record = previous.file_containers.get(&target.file).ok_or(Failure::ContainerScope)?;
        if record.working.incremental().is_some() {
            let disk = config.disks.iter().find(|disk| disk.disk_id == record.owner_disk.to_string())
                .ok_or(Failure::ContainerScope)?;
            let observed = match locations {
                Some(locations) => locations.observe_previous(
                    config, resources, previous, next, candidate, target.file, disk,
                )?,
                None => crate::container_storage::ObservedContainerReplica::observe(
                    published_source(config, resources, previous, target.file, disk)?,
                )?,
            };
            let (base, _) = record.working.draft().ok_or(Failure::ContainerScope)?;
            let version = previous.versions.get(&base.version_id().to_string())
                .ok_or(Failure::ContainerScope)?;
            let reader = crate::container_storage::HeldManualReader::capture(
                std::sync::Arc::new(observed), *base, version,
            )?;
            let read = crate::container_storage::ReadDraftDigest::capture(
                std::sync::Arc::new(reader), config.checksum_chunk_bytes,
            )?;
            return verify_transition_read(config, previous, next, candidate, Some(read));
        }
    }
    if result.kind != crate::persist::file_draft::unborn_first::KIND
        && result.kind != crate::persist::snapshot_worktree::ADDED_FIRST
    {
        return verify_transition(config, previous, next, candidate);
    }
    validate_state(previous)?;
    validate_state(next)?;
    let first = unborn::first_candidate(config, resources, previous, next, candidate, locations)?
        .ok_or(Failure::ContainerScope)?;
    Ok(VerifiedContainerTransition {
        previous, next, candidate, renames: BTreeMap::new(), birth: None,
        first: BTreeMap::from([(first.file(), first)]), draft_reads: Vec::new(),
    })
}

/// Cold ordinary transition check. Candidate evidence supplies the exact first
/// prepared order; serialized owner/root fields cannot choose a different target.
pub(crate) fn verify_transition<'a>(
    config: &crate::ValidatedConfig,
    previous: &'a State,
    next: &'a State,
    candidate: &'a crate::journal::Candidate,
) -> Result<VerifiedContainerTransition<'a>> {
    verify_transition_read(config, previous, next, candidate, None)
}

fn verify_transition_read<'a>(
    config: &crate::ValidatedConfig, previous: &'a State, next: &'a State,
    candidate: &'a crate::journal::Candidate,
    draft_read: Option<std::sync::Arc<crate::container_storage::ReadDraftDigest>>,
) -> Result<VerifiedContainerTransition<'a>> {
    validate_state(previous)?;
    validate_state(next)?;
    // This pure container projection is also used for an already-sealed
    // namespace rename and Backup geometry, neither of which issues Workspace
    // source authority. The actual planner, cold scan, installed material
    // observers and archive all require the adjacent, original-prefix E proof.
    let result=next.operations.get(&candidate.operation_id).ok_or(Failure::ContainerScope)?;
    crate::engine::workspace_provenance::retained(next,result)?;
    if let Some(proof) = config.verify_backup_first_layout(previous, next, candidate)? {
        proof.require_pair(previous, next, candidate)?;
        return Ok(VerifiedContainerTransition {
            previous, next, candidate, renames: BTreeMap::new(), birth: None,
            first: BTreeMap::new(), draft_reads: Vec::new(),
        });
    }
    if let Some(birth) = unborn::candidate(config, previous, next, candidate)? {
        return Ok(VerifiedContainerTransition {
            previous, next, candidate, renames: BTreeMap::new(),
            birth: Some(std::sync::Arc::new(birth)), first: BTreeMap::new(), draft_reads: Vec::new(),
        });
    }
    if unborn::save_candidate(config, previous, next, candidate)?.is_some() {
        return Ok(VerifiedContainerTransition {
            previous, next, candidate, renames: BTreeMap::new(), birth: None,
            first: BTreeMap::new(), draft_reads: Vec::new(),
        });
    }
    crate::persist::file_adoption::transition(previous, next, result, !candidate.versions.is_empty())?;
    let (mut directories, additions) = super::directories::project(
        config, previous, next, result, Some(&next.namespace_directories),
    )?;
    if !additions.is_empty() && candidate.generation().is_some() {
        return Err(Failure::NamespaceSource.into());
    }
    let working = crate::engine::file_draft::verify_working_transition(previous, next, result, draft_read.as_deref())?;
    let mut expected = previous.file_containers.clone();
    if let Some(file) = working {
        let after = next.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
        expected.get_mut(&file).ok_or(Failure::ContainerScope)?.working = after.working.clone();
    }
    let renames = rename::project(previous, next, result)?;
    if !renames.is_empty() && (candidate.generation().is_some() || !candidate.versions.is_empty()) {
        return Err(Failure::WholeContainerLifecycleUnsupported.into());
    }
    for (file, change) in &renames {
        expected.get_mut(file).ok_or(Failure::ContainerScope)?
            .namespace = change.target().clone();
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
                site: storage_contract::file_container::site::FileSite::Created {},
                history_tree_id: version.history_tree_id.parse()?,
                owner_disk: disk.disk_id.parse()?,
                created_by: candidate.operation_id,
                namespace: super::namespace::file_target(
                    next, file, version.history_tree_id.parse()?,
                )?,
                working: FileWorkingState::initial_manual(),
                replicas: BTreeMap::new(),
            });
        }
        let mut record = expected.get(&file).ok_or(Failure::ContainerShape)?.clone();
        if !(((!previous.file_containers.contains_key(&file) && record.working == FileWorkingState::initial_manual())
            || (result.kind == "workspace.commit" && record.working.draft().is_none()
                && record.working.unborn().is_none()))
            || (matches!(record.working, FileWorkingState::Automatic { body: storage_contract::file_draft::AutomaticBody::Clean {}, .. })
                && working == Some(file))
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
        let mut installed_sources = BTreeSet::new();
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
            match replica.prefixes.first().map(|prefix| prefix.source_disk) {
                None if installed_sources.is_empty() && id == actual.owner_disk => (),
                Some(source) if installed_sources.contains(&source) => {
                    let parent = actual.replicas.get(&source).ok_or(Failure::ContainerScope)?;
                    if replica.prefixes.iter().skip(1).map(|prefix| prefix.source_disk)
                        .ne(parent.prefixes.iter().map(|prefix| prefix.source_disk))
                    { return Err(Failure::ContainerScope.into()); }
                }
                _ => return Err(Failure::ContainerScope.into()),
            }
            if !installed_sources.insert(id) { return Err(Failure::ContainerScope.into()); }
            known_prefixes.extend_chain(id, &replica.prefixes)?;
            // Existing complete records never retarget, even for identical bytes.
            record.replicas.entry(id).or_insert_with(|| replica.clone());
        }
        expected.insert(file, record);
    }
    if expected != next.file_containers {
        return Err(Failure::ContainerScope.into());
    }
    let mut additions = super::directories::project_replicas(
        config, next, result, &mut directories, Some(&next.namespace_directories),
    )?;
    if let Some(selected) = crate::protection::candidate_directory_selection(
        config, previous, next, result,
    )? {
        additions.extend(super::directories::project_directory_backups(
            config, next, result, &mut directories, &selected, Some(&next.namespace_directories),
            |record, disk| {
                let replica = record.replicas.get(&disk).ok_or(Failure::NamespaceSource)?;
                known_prefixes.extend_chain(disk, &replica.source_prefixes)?;
                Ok(replica.source_prefixes.clone())
            },
        )?);
    }
    if directories != next.namespace_directories {
        return Err(Failure::NamespaceSource.into());
    }
    if !additions.is_empty() && candidate.generation().is_some() {
        crate::protection::verify_automatic_addition_candidate_source(
            config, previous, result, candidate,
        )?;
    }
    Ok(VerifiedContainerTransition {
        previous, next, candidate, renames, birth: None, first: BTreeMap::new(),
        draft_reads: draft_read.into_iter().collect(),
    })
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/ordinary_container_rename_tests.rs"]
mod rename_tests;

/// Native Backup's already authenticated original Config/State uses this same
/// geometry kernel before opening its exact original held occurrence.
pub(crate) fn require_backup_original_geometry(
    config: &Config, state: &State, file: FileId, target: DiskId,
) -> Result<()> {
    let record = state.file_containers.get(&file).ok_or(Failure::ContainerShape)?;
    require_current_container(state, file)?;
    if super::namespace::file_target(state, file, record.history_tree_id)? != record.namespace {
        return Err(Failure::NamespaceSource.into());
    }
    let origin = super::namespace::root_origin(
        state, record.namespace.repository, record.namespace.branch,
    )?;
    if config.storage_roots.get(&record.owner_disk) != Some(&origin.genesis) {
        return Err(Failure::RootIdentity.into());
    }
    prefixes::require_geometry(config, record, target)
}
