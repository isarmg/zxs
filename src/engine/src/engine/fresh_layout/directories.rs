//! Ordinary logical parents and their decided physical occurrences. This kernel
//! supplies no handles: only the ordinary publication's EXCL preparation can
//! replace fixed-width budget entities with actual observations.
use super::FreshLayoutError as Failure;
use crate::{Config, Entry, NamespaceEntryKind, OperationResult, Result, State};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{BranchId, DiskId, EntryId, RepositoryId};
use storage_contract::file_container::{
    ContainerEntity, NamespaceDirectoryParent as Parent, NamespaceDirectoryRecord as Record,
    NamespaceDirectoryReplica as Replica,
};
use storage_contract::working_tree::{BindingLifecycle, DirectoryBinding, DirectoryId};

type Records = storage_contract::TrackedMap<Record>;

fn entries(state: &State, repository: RepositoryId, branch: BranchId) -> Result<&[Entry]> {
    let repo = state.repositories.get(&repository.to_string())
        .filter(|repo| repo.file_manager_root && repo.main_branch_id == branch.to_string())
        .ok_or(Failure::NamespaceUnmapped)?;
    let branch = state.branches.get(&repo.main_branch_id)
        .filter(|branch| branch.file_id.is_none() && branch.state == crate::BranchState::Active)
        .ok_or(Failure::NamespaceSource)?;
    Ok(&state.commits.get(&branch.head_commit_id).ok_or(Failure::NamespaceSource)?.entries)
}

pub(super) fn parent(entries: &[Entry], entry: &Entry) -> Result<Parent> {
    parent_with(entry, |key| entries.iter().find(|item| {
        item.kind == NamespaceEntryKind::Directory && item.path.key() == key
    }))
}

fn parent_with<'a>(
    entry: &Entry, lookup: impl FnOnce(&str) -> Option<&'a Entry>,
) -> Result<Parent> {
    match entry.path.parent() {
        None => Ok(Parent::Root),
        Some(path) => {
            let parent = lookup(path.key()).ok_or(Failure::NamespaceUnmapped)?;
            Ok(Parent::Directory(parent.entry_id.parse()?))
        }
    }
}

/// Borrow only this invocation's exact current branch entries. First-match
/// semantics deliberately match the old scans; duplicate/invalid rows are not
/// accepted here or rejected earlier than the existing complete validators.
struct DirectoryEntries<'a> {
    all: &'a [Entry],
    by_id: BTreeMap<&'a str, &'a Entry>,
    by_path: BTreeMap<&'a str, &'a Entry>,
}
impl<'a> DirectoryEntries<'a> {
    fn new(all: &'a [Entry]) -> Self {
        let mut by_id = BTreeMap::new();
        let mut by_path = BTreeMap::new();
        for entry in all.iter().filter(|entry| entry.kind == NamespaceEntryKind::Directory) {
            by_id.entry(entry.entry_id.as_str()).or_insert(entry);
            by_path.entry(entry.path.key()).or_insert(entry);
        }
        Self { all, by_id, by_path }
    }

    fn directory(&self, id: &str) -> Result<&'a Entry> {
        self.by_id.get(id).copied().ok_or_else(|| Failure::NamespaceSource.into())
    }

    fn parent(&self, entry: &Entry) -> Result<Parent> {
        parent_with(entry, |key| self.by_path.get(key).copied())
    }
}

/// At most two borrowed index entries per actual directory in the branches
/// touched by this one State validation/projection. No cross-call/state cache,
/// authority, retained root or new physical observation escapes this object.
struct BranchEntries<'a> {
    state: &'a State,
    branches: BTreeMap<(RepositoryId, BranchId), DirectoryEntries<'a>>,
}
impl<'a> BranchEntries<'a> {
    fn new(state: &'a State) -> Self {
        Self { state, branches: BTreeMap::new() }
    }

    fn get(&mut self, repository: RepositoryId, branch: BranchId) -> Result<&DirectoryEntries<'a>> {
        use std::collections::btree_map::Entry;
        match self.branches.entry((repository, branch)) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => {
                let all = entries(self.state, repository, branch)?;
                Ok(entry.insert(DirectoryEntries::new(all)))
            }
        }
    }
}

/// Complete logical chain, root first. No physical path is accepted as evidence.
pub(crate) fn chain(state: &State, leaf: EntryId) -> Result<Vec<&Record>> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut cursor = Parent::Directory(leaf);
    while let Parent::Directory(entry) = cursor {
        if !seen.insert(entry)
            || chain.len() == crate::working_tree::admission::MAX_DIRECTORY_DEPTH
        {
            return Err(Failure::ContainerShape.into());
        }
        let record = state.namespace_directories.get(&entry.to_string())
            .ok_or(Failure::NamespaceUnmapped)?;
        chain.push(record);
        cursor = record.parent;
    }
    chain.reverse();
    Ok(chain)
}

pub(crate) fn validate_state(state: &State) -> Result<()> {
    let mut workspace_creations = BTreeMap::new();
    let mut indexed = BranchEntries::new(state);
    let mut ids = BTreeSet::new();
    let mut entities = BTreeSet::new();
    for (key, record) in state.namespace_directories.iter() {
        if key != &record.entry.to_string() || record.replicas.is_empty()
            || !record.replicas.contains_key(&record.owner_disk)
        {
            return Err(Failure::ContainerShape.into());
        }
        let all = indexed.get(record.repository, record.branch)?;
        let entry = all.directory(key)?;
        if all.parent(entry)? != record.parent
            || name_policy::LookupName::parse(entry.name())? != record.name
        {
            return Err(Failure::NamespaceSource.into());
        }
        let origin = super::namespace::root_origin(state, record.repository, record.branch)?;
        if origin.genesis.registration.disk_id.parse::<DiskId>()? != record.owner_disk {
            return Err(Failure::NamespaceSource.into());
        }
        if !crate::persist::layout::namespace::ordinary_path(&entry.path) {
            return Err(Failure::NamespaceSource.into());
        }
        require_creation(state, record, &mut workspace_creations)?;
        for (disk, replica) in &record.replicas {
            if replica.entity.inode() == 0 || replica.parent_entity.inode() == 0
                || replica.entity.device() != replica.parent_entity.device()
                || replica.lifecycle != BindingLifecycle::Current
                || !ids.insert((*disk, replica.preparation))
                || !entities.insert((*disk, replica.entity))
                || !state.operations.get(&replica.created_by)
                    .is_some_and(|result| result.sequence <= state.sequence)
                || (disk == &record.owner_disk && (!replica.source_chain.is_empty()
                    || !replica.source_prefixes.is_empty()
                    || replica.root_id != origin.genesis.root_id))
                || (disk != &record.owner_disk && replica.source_chain.last()
                    .is_none_or(|source| source.disk_id != record.owner_disk.to_string()))
                || replica.source_prefixes.iter().map(|prefix| prefix.source_disk.to_string())
                    .ne(replica.source_chain.iter().map(|source| source.disk_id.clone()))
            {
                return Err(Failure::ContainerShape.into());
            }
            let expected = match record.parent {
                Parent::Root if *disk == record.owner_disk => {
                    let entity = origin.genesis.roots.common_data.entity;
                    ContainerEntity::new(entity.device(), entity.inode())
                }
                Parent::Root => {
                    replica.source_prefixes.last()
                        .ok_or(Failure::NamespaceSource)?.areas.common_data
                }
                Parent::Directory(parent) => {
                    let parent = state.namespace_directories.get(&parent.to_string())
                        .ok_or(Failure::ContainerShape)?;
                    let occurrence = parent.replicas.get(disk).ok_or(Failure::ContainerShape)?;
                    if parent.repository != record.repository || parent.branch != record.branch
                        || parent.owner_disk != record.owner_disk
                        || occurrence.root_id != replica.root_id
                        || occurrence.source_chain != replica.source_chain
                        || occurrence.source_prefixes != replica.source_prefixes
                    {
                        return Err(Failure::ContainerShape.into());
                    }
                    occurrence.entity
                }
            };
            if replica.parent_entity != expected {
                return Err(Failure::ContainerIdentity.into());
            }
        }
        chain(state, record.entry)?;
    }
    Ok(())
}

fn require_creation(
    state: &State,
    record: &Record,
    workspace_creations: &mut BTreeMap<
        storage_contract::OperationId, (RepositoryId, BTreeSet<EntryId>),
    >,
) -> Result<()> {
    let result = state.operations.get(&record.created_by).ok_or(Failure::NamespaceSource)?;
    if result.kind == "workspace.commit" {
        // One original receipt may introduce many directories. Cache only this
        // invocation's verified logical EntryIds, never workspace/runtime I/O.
        if !workspace_creations.contains_key(&record.created_by) {
            let after = state.commits.get(result.commit_id.as_deref()
                .ok_or(Failure::NamespaceSource)?).ok_or(Failure::NamespaceSource)?;
            let [parent] = after.parent_commit_ids.as_slice() else {
                return Err(Failure::NamespaceSource.into());
            };
            let before = state.commits.get(parent).ok_or(Failure::NamespaceSource)?;
            let (repository, introduced) = crate::business::workspace_directory_entries(
                state, result, before, after, after.entries.len(),
            )?;
            let ids = introduced.iter().map(|entry| entry.entry_id.parse::<EntryId>())
                .collect::<std::result::Result<BTreeSet<_>, _>>()?;
            workspace_creations.insert(record.created_by, (repository, ids));
        }
        let (repository, introduced) = workspace_creations.get(&record.created_by)
            .ok_or(Failure::NamespaceSource)?;
        if *repository != record.repository || !introduced.contains(&record.entry)
            || result.branch_id.as_deref() != Some(record.branch.to_string().as_str())
        {
            return Err(Failure::NamespaceSource.into());
        }
        return Ok(());
    }
    let (repository, change) = crate::persist::namespace_directory::mkdir_request(state, result)?;
    let commit = state.commits.get(result.commit_id.as_deref()
        .ok_or(Failure::NamespaceSource)?).ok_or(Failure::NamespaceSource)?;
    let entry = commit.entries.iter().find(|entry| entry.entry_id == record.entry.to_string()
        && entry.kind == NamespaceEntryKind::Directory).ok_or(Failure::NamespaceSource)?;
    if repository != record.repository || result.branch_id.as_deref()
        != Some(record.branch.to_string().as_str())
        || commit.operation_id != record.created_by || commit.sequence != result.sequence
        || !(change.path() == entry.path.as_str()
            || change.path().starts_with(&format!("{}/", entry.path.as_str())))
    {
        return Err(Failure::NamespaceSource.into());
    }
    Ok(())
}

/// Reuse the existing namespace constructor with the actual committed EntryIds.
/// The caller cannot introduce any other logical entry or mutate prior entries.
fn new_entries(
    previous: &State,
    next: &State,
    result: &OperationResult,
    maximum: usize,
) -> Result<(RepositoryId, Vec<Entry>)> {
    if result.kind == "workspace.commit" {
        let history = next.histories.get(result.history_tree_id.as_deref()
            .ok_or(Failure::NamespaceSource)?).ok_or(Failure::NamespaceSource)?;
        let repo = previous.repositories.get(&history.repository_id)
            .filter(|repo| repo.file_manager_root).ok_or(Failure::NamespaceUnmapped)?;
        let branch = previous.branches.get(&repo.main_branch_id)
            .ok_or(Failure::NamespaceSource)?;
        let before = previous.commits.get(&branch.head_commit_id)
            .ok_or(Failure::NamespaceSource)?;
        let next_branch = next.branches.get(&repo.main_branch_id)
            .ok_or(Failure::NamespaceSource)?;
        let after = next.commits.get(&next_branch.head_commit_id)
            .ok_or(Failure::NamespaceSource)?;
        return crate::business::workspace_directory_entries(
            next, result, before, after, maximum,
        );
    }
    let (repository, change) = crate::persist::namespace_directory::mkdir_request(next, result)?;
    let repo = previous.repositories.get(&repository.to_string())
        .filter(|repo| repo.file_manager_root).ok_or(Failure::NamespaceUnmapped)?;
    let branch = previous.branches.get(&repo.main_branch_id).ok_or(Failure::NamespaceSource)?;
    let before = previous.commits.get(&branch.head_commit_id).ok_or(Failure::NamespaceSource)?;
    let next_branch = next.branches.get(&repo.main_branch_id).ok_or(Failure::NamespaceSource)?;
    let after = next.commits.get(&next_branch.head_commit_id).ok_or(Failure::NamespaceSource)?;
    if result.commit_id.as_deref() != Some(after.commit_id.as_str())
        || after.parent_commit_ids != [before.commit_id.clone()]
        || after.operation_id != result.operation_id || after.sequence != next.sequence
        || after.history_tree_id != before.history_tree_id || after.kind != "namespace"
    {
        return Err(Failure::NamespaceSource.into());
    }
    let before_ids = before.entries.iter().map(|entry| entry.entry_id.as_str())
        .collect::<BTreeSet<_>>();
    let added = after.entries.iter().filter(|entry| {
        !before_ids.contains(entry.entry_id.as_str())
    }).cloned().collect::<Vec<_>>();
    if added.is_empty() || added.iter().any(|entry| entry.kind != NamespaceEntryKind::Directory) {
        return Err(Failure::NamespaceSource.into());
    }
    let mut expected = before.entries.clone();
    let mut ids = added.iter();
    crate::business::namespace::apply_namespace_with_ids(&mut expected, &change, &mut |prefix| {
        if prefix != "entry" {
            return Err(Failure::NamespaceSource.into());
        }
        ids.next().map(|entry| entry.entry_id.clone())
            .ok_or_else(|| Failure::NamespaceSource.into())
    })?;
    crate::business::namespace::validate_entries(&mut expected, maximum)?;
    if ids.next().is_some() || expected != after.entries {
        return Err(Failure::NamespaceSource.into());
    }
    Ok((repository, added))
}

/// Hot allocates preparation IDs; cold supplies only the IDs and observations
/// from the authenticated snapshot, then compares the entire physical map.
pub(super) fn project(
    config: &Config, previous: &State, next: &State, result: &OperationResult,
    actual: Option<&Records>,
) -> Result<(Records, BTreeSet<EntryId>)> {
    let mut records = previous.namespace_directories.clone();
    let mut indexed = BranchEntries::new(next);
    let mut additions = BTreeSet::new();
    for record in records.values() {
        let all = indexed.get(record.repository, record.branch)?;
        let entry = all.directory(&record.entry.to_string())?;
        if all.parent(entry)? != record.parent
            || name_policy::LookupName::parse(entry.name())? != record.name
        {
            return Err(Failure::NamespaceSource.into());
        }
    }
    for repo in next.repositories.values().filter(|repo| repo.file_manager_root) {
        let repository = repo.repository_id.parse::<RepositoryId>()?;
        let branch = repo.main_branch_id.parse::<BranchId>()?;
        let all = indexed.get(repository, branch)?;
        for entry in all.all.iter().filter(|entry| entry.kind == NamespaceEntryKind::Directory) {
            if !records.contains_key(&entry.entry_id) {
                additions.insert(entry.entry_id.parse::<EntryId>()?);
            }
        }
    }
    if additions.is_empty() {
        return Ok((records, additions));
    }
    let (repository, introduced) = new_entries(previous, next, result, config.max_entries)?;
    let introduced_ids = introduced.iter().map(|entry| entry.entry_id.parse::<EntryId>())
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    if introduced_ids != additions {
        return Err(Failure::NamespaceSource.into());
    }
    let repo = previous.repositories.get(&repository.to_string()).ok_or(Failure::NamespaceSource)?;
    let branch = repo.main_branch_id.parse::<BranchId>()?;
    let origin = super::namespace::root_origin(previous, repository, branch)?;
    let owner = origin.genesis.registration.disk_id.parse::<DiskId>()?;
    if config.storage_roots.get(&owner) != Some(&origin.genesis) {
        return Err(Failure::RootIdentity.into());
    }
    let all = indexed.get(repository, branch)?;
    for entry in introduced {
        if !crate::persist::layout::namespace::ordinary_path(&entry.path) {
            return Err(Failure::NamespaceSource.into());
        }
        let parent = all.parent(&entry)?;
        let parent_entity = match parent {
            Parent::Root => {
                let entity = origin.genesis.roots.common_data.entity;
                ContainerEntity::new(entity.device(), entity.inode())
            }
            Parent::Directory(id) => records.get(&id.to_string())
                .and_then(|record| record.replicas.get(&owner))
                .ok_or(Failure::NamespaceSource)?.entity,
        };
        let observed = actual.and_then(|records| records.get(&entry.entry_id))
            .and_then(|record| record.replicas.get(&owner));
        let preparation = match observed {
            Some(observed) => observed.preparation,
            None if actual.is_none() => DirectoryId::generate()?,
            None => return Err(Failure::NamespaceSource.into()),
        };
        let entity = observed.map_or(ContainerEntity::new(0, 0), |replica| replica.entity);
        records.insert(entry.entry_id.clone(), Record {
            repository, branch, entry: entry.entry_id.parse()?, parent,
            name: name_policy::LookupName::parse(entry.name())?, owner_disk: owner,
            created_by: result.operation_id,
            replicas: BTreeMap::from([(owner, Replica {
                root_id: origin.genesis.root_id, source_chain: Vec::new(),
                source_prefixes: Vec::new(), parent_entity,
                preparation, entity, lifecycle: BindingLifecycle::Current,
                created_by: result.operation_id,
            })]),
        });
    }
    Ok((records, additions))
}

/// Add occurrences for actual FILE parents and explicit directory-backup scope
/// through the same pure row constructor. No logical EntryId is reallocated.
pub(super) fn project_replicas(
    config: &Config, next: &State, result: &OperationResult, records: &mut Records,
    actual: Option<&Records>,
) -> Result<BTreeSet<(EntryId, DiskId)>> {
    let mut additions = BTreeSet::new();
    for file in next.file_containers.values() {
        let storage_contract::file_container::ContainerParent::Directory(leaf) =
            &file.namespace.parent else { continue; };
        for (disk, replica) in &file.replicas {
            if *disk == file.owner_disk { continue; }
            let binding = config.storage_roots.get(disk).ok_or(Failure::Registration)?;
            if binding.root_id != replica.root_id {
                return Err(Failure::NamespaceSource.into());
            }
            let parent = next.namespace_directories.get(&leaf.to_string())
                .ok_or(Failure::NamespaceUnmapped)?;
            if parent.repository != file.namespace.repository
                || parent.branch != file.namespace.branch
                || parent.owner_disk != file.owner_disk
            {
                return Err(Failure::NamespaceSource.into());
            }
            project_occurrence(config, next, result, records, *leaf, *disk,
                &replica.prefixes, actual, &mut additions)?;
        }
    }
    Ok(additions)
}

pub(super) fn project_directory_backups(
    config: &Config, next: &State, result: &OperationResult, records: &mut Records,
    selected: &crate::protection::DirectoryBackupSelection, actual: Option<&Records>,
    mut prefixes: impl FnMut(&Record, DiskId)
        -> Result<Vec<storage_contract::file_container::ContainerPrefix>>,
) -> Result<BTreeSet<(EntryId, DiskId)>> {
    let mut additions = BTreeSet::new();
    for disk in selected.route() {
        for entry in selected.entries() {
            let record = next.namespace_directories.get(&entry.to_string())
                .ok_or(Failure::NamespaceUnmapped)?;
            if *disk == record.owner_disk { continue; }
            let chain = prefixes(record, *disk)?;
            project_occurrence(config, next, result, records, entry, *disk,
                &chain, actual, &mut additions)?;
        }
    }
    Ok(additions)
}

fn project_occurrence(
    config: &Config, next: &State, result: &OperationResult, records: &mut Records,
    leaf: EntryId, disk: DiskId, prefixes: &[storage_contract::file_container::ContainerPrefix],
    actual: Option<&Records>, additions: &mut BTreeSet<(EntryId, DiskId)>,
) -> Result<()> {
    let leaf_record = next.namespace_directories.get(&leaf.to_string())
        .ok_or(Failure::NamespaceUnmapped)?;
    let binding = config.storage_roots.get(&disk).ok_or(Failure::Registration)?;
    super::containers::prefixes::require_source_geometry(
        config, leaf_record.owner_disk, disk, binding.root_id, prefixes,
    )?;
    let mut parent_entity = prefixes.last().ok_or(Failure::NamespaceSource)?.areas.common_data;
    for parent in chain(next, leaf)? {
        if parent.repository != leaf_record.repository || parent.branch != leaf_record.branch
            || parent.owner_disk != leaf_record.owner_disk
        {
            return Err(Failure::NamespaceSource.into());
        }
        let record = records.get(&parent.entry.to_string()).ok_or(Failure::NamespaceUnmapped)?;
        if let Some(old) = record.replicas.get(&disk) {
            if old.root_id != binding.root_id || old.source_chain != binding.source_chain
                || old.source_prefixes != prefixes || old.parent_entity != parent_entity
            {
                return Err(Failure::ContainerIdentity.into());
            }
            parent_entity = old.entity;
            continue;
        }
        let observed = actual.and_then(|all| all.get(&parent.entry.to_string()))
            .and_then(|record| record.replicas.get(&disk));
        let preparation = match observed {
            Some(replica) => replica.preparation,
            None if actual.is_none() => DirectoryId::generate()?,
            None => return Err(Failure::NamespaceSource.into()),
        };
        let entity = observed.map_or(ContainerEntity::new(0, 0), |replica| replica.entity);
        records.get_mut(&parent.entry.to_string()).ok_or(Failure::NamespaceUnmapped)?
            .replicas.insert(disk, Replica {
                root_id: binding.root_id, source_chain: binding.source_chain.clone(),
                source_prefixes: prefixes.to_vec(), parent_entity, preparation, entity,
                lifecycle: BindingLifecycle::Current, created_by: result.operation_id,
            });
        additions.insert((parent.entry, disk));
        parent_entity = entity;
    }
    Ok(())
}

/// Lossless owner projection into the existing worktree directory vocabulary.
/// Source-slice occurrences are not mapped to an invented anchor DirectoryId.
pub(crate) fn owner_binding(state: &State, record: &Record) -> Result<DirectoryBinding> {
    let replica = record.replicas.get(&record.owner_disk).ok_or(Failure::ContainerShape)?;
    if !replica.source_chain.is_empty() || !replica.source_prefixes.is_empty() {
        return Err(Failure::NamespaceSource.into());
    }
    let parent_directory_id = match record.parent {
        Parent::Root => super::namespace::root_origin(state, record.repository, record.branch)?
            .genesis.roots.common_data.directory_id,
        Parent::Directory(parent) => state.namespace_directories.get(&parent.to_string())
            .and_then(|record| record.replicas.get(&record.owner_disk))
            .ok_or(Failure::NamespaceSource)?.preparation,
    };
    Ok(DirectoryBinding {
        directory_id: replica.preparation, root_id: replica.root_id, parent_directory_id,
        name: record.name.clone(), entity: storage_contract::working_tree::LocalEntityIdentity::new(
            replica.entity.device(), replica.entity.inode(),
        ), lifecycle: replica.lifecycle, operation_id: replica.created_by,
    })
}

/// Same permanent root source as FILE; the logical map alone grants no I/O.
pub(crate) fn read_owner<'a>(
    config: &'a Config, resources: &'a crate::disk::DiskResources, source: &'a State,
    record: &Record, target: DiskId,
) -> Result<super::namespace::RegisteredNamespaceRoot<'a>> {
    if target != record.owner_disk {
        return Err(Failure::NamespaceUnmapped.into());
    }
    let root = super::namespace::registered_root(
        source, config, resources, record.repository, record.branch,
    )?;
    let replica = record.replicas.get(&target).ok_or(Failure::NamespaceUnmapped)?;
    if !replica.source_chain.is_empty() || !replica.source_prefixes.is_empty()
        || replica.root_id != root.binding().root_id
        || root.disk().disk_id.parse::<DiskId>()? != target
    {
        return Err(Failure::NamespaceSource.into());
    }
    Ok(root)
}

pub(crate) fn read_published_backup<'a>(
    config: &'a Config, resources: &'a crate::disk::DiskResources, state: &'a State,
    record: &Record, target: DiskId,
) -> Result<super::namespace::PublishedBackupSourcePath<'a>> {
    if target == record.owner_disk {
        return Err(Failure::NamespaceSource.into());
    }
    let source = super::namespace::published_backup_source_path(
        state, config, resources, record.repository, record.branch, target,
    )?;
    if source.owner() != record.owner_disk {
        return Err(Failure::NamespaceSource.into());
    }
    require_backup_geometry(config, record, target, source.target_binding(), source.destination())?;
    source.require_source(state, config, resources)?;
    Ok(source)
}

pub(crate) fn read_backup<'a>(
    config: &'a Config, resources: &'a crate::disk::DiskResources, source: &'a State,
    record: &Record, target: DiskId,
) -> Result<super::namespace::PreparedBackupSourcePath<'a>> {
    if target == record.owner_disk {
        return Err(Failure::NamespaceSource.into());
    }
    let source = super::namespace::read_backup_source_path(
        source, config, resources, record.repository, record.branch, target,
    )?;
    require_backup_geometry(config, record, target, source.target_binding(), source.destination())?;
    Ok(source)
}

fn require_backup_geometry(
    config: &Config, record: &Record, target: DiskId,
    binding: &storage_contract::working_tree::RootBinding,
    path: &crate::disk_names::configuration::ConfiguredSourcePath<'_>,
) -> Result<()> {
    let replica = record.replicas.get(&target).ok_or(Failure::NamespaceUnmapped)?;
    super::containers::prefixes::require_source_geometry(
        config, record.owner_disk, target, replica.root_id, &replica.source_prefixes,
    )?;
    if replica.root_id != binding.root_id
        || replica.source_chain != path.source_registrations().collect::<Vec<_>>()
    {
        return Err(Failure::NamespaceSource.into());
    }
    Ok(())
}
