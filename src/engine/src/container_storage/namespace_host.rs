//! Ordinary namespace hosts use the explicitly registered common-data path.
//! A directory-Git locator, FILE location or matching pathname cannot supply
//! this registration. Missing physical material is never adopted by reading it.
use crate::disk::{DirectoryIoError, DiskRoot};
use crate::engine::RegisteredNamespaceRoot;
use crate::persist::snapshot_worktree::{HostEvidence, NamespaceDirectoryBinding};
use crate::working_tree::records::file_identity;
use crate::working_tree::snapshot::{HeldSnapshotNode, SnapshotNodeBinding, SnapshotStoreError};
use crate::{Engine, NamespaceEntryKind, NamespaceSnapshot, Repository};
use name_policy::{LookupName, PortablePath};
use std::collections::BTreeSet;
use storage_contract::snapshot_worktree::{
    NamespaceDirectoryId, NamespaceHostAddress, SnapshotNamespace,
};
use storage_contract::working_tree::{BindingLifecycle, RootId};
use storage_contract::{BranchId, EntryId, Generation, OperationId, RepositoryId};

mod preparation;
pub(crate) use preparation::{
    NamespacePreparationError, NamespacePreparationObservation, PreparedNamespaceHost,
};
pub(crate) use preparation::PreparedNamespaceNode;

#[derive(Debug, error_definition::Error)]
pub(crate) enum NamespaceHostError {
    #[error("reading the current namespace host source")]
    Source(#[source] crate::Error),
    #[error("decoding a current namespace host identity")]
    Identity(#[source] storage_contract::Error),
    #[error("resolving an ordinary namespace component")]
    Name(#[source] name_policy::Error),
    #[error("snapshot host is not in the current repository main namespace")]
    Main,
    #[error("snapshot host namespace generation changed")]
    Generation,
    #[error("snapshot host is absent, hidden, or not an ordinary directory")]
    Directory,
    #[error("snapshot operation requires an unoccupied name in its registered root")]
    RootMount,
    #[error("the registered root contains an unsupported or unexplained namespace item")]
    RootInventory,
    #[error("snapshot host names reserved management material")]
    Management,
    #[error("snapshot host has no matching published root registration")]
    Root,
    #[error("snapshot host source or its exact entity changed")]
    Changed,
    #[error("namespace directory is occupied by incomplete reservation {0}")]
    Pending(OperationId),
    #[error("namespace host already has an operation or conflicting allocated identity")]
    Allocated,
    #[error("namespace host exceeds the directory depth bound")]
    Depth,
    #[error("ordinary namespace preparation requires the actual root's metadata reserve")]
    Reserve,
    #[error("the exact ordinary directory preparation or readback failed")]
    Physical(#[source] DirectoryIoError),
    #[error("a new ordinary directory component exceeds its actual root budget")]
    NameBudget(#[source] crate::name_budget::NameBudgetError),
    #[error("reading the independently registered snapshot node")]
    Snapshot(#[source] SnapshotStoreError),
}
type Result<T> = std::result::Result<T, NamespaceHostError>;

pub(crate) struct NamespaceHost<'a> {
    engine: &'a Engine,
    address: NamespaceHostAddress,
    snapshot: NamespaceSnapshot,
    root: RegisteredNamespaceRoot<'a>,
    origin: NamespaceOrigin,
}

enum NamespaceOrigin {
    Request(SnapshotNamespace),
    Registered,
}

struct Component {
    entry: EntryId,
    name: LookupName,
}

impl<'a> NamespaceHost<'a> {
    pub(crate) fn for_request(engine: &'a Engine, source: &SnapshotNamespace) -> Result<Self> {
        engine.require_write().map_err(NamespaceHostError::Source)?;
        let branch = source.branch_id.to_string();
        let (repository, snapshot) = current_main(engine, source.branch_id)?;
        if engine.state().branches[&branch].generation != source.expected_branch_generation.get() {
            return Err(NamespaceHostError::Generation);
        }
        let disk = engine.disk(&repository.primary_slot_id).map_err(NamespaceHostError::Source)?;
        preparation::require_namespace_write(engine, source.branch_id, disk)?;
        let repository_id = repository.repository_id.parse()
            .map_err(NamespaceHostError::Identity)?;
        let directory = match &source.root {
            None => NamespaceDirectoryId::Repository(repository_id),
            Some(path) => {
                let entry = snapshot.tree().get_key(path.key())
                    .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
                    .ok_or(NamespaceHostError::Directory)?;
                require_ordinary(&entry.path)?;
                NamespaceDirectoryId::Entry(entry.entry_id.parse()
                    .map_err(NamespaceHostError::Identity)?)
            }
        };
        Self::bind(engine, NamespaceHostAddress {
            repository_id, branch_id: source.branch_id, directory,
        }, snapshot, repository, NamespaceOrigin::Request(source.clone()))
    }

    pub(crate) fn registered(engine: &'a Engine, address: NamespaceHostAddress) -> Result<Self> {
        let (repository, snapshot) = current_main(engine, address.branch_id)?;
        let repository_id: RepositoryId = repository.repository_id.parse()
            .map_err(NamespaceHostError::Identity)?;
        if repository_id != address.repository_id {
            return Err(NamespaceHostError::Main);
        }
        let selected = Self::bind(
            engine, address, snapshot, repository, NamespaceOrigin::Registered,
        )?;
        selected.components()?;
        Ok(selected)
    }

    /// Only reserve the logical mount. Physical exposure needs its own manager
    /// decision; no existing visible or unknown local entry may be overlaid.
    pub(crate) fn require_root_mount(
        &self,
        parent: storage_contract::working_tree::DirectoryId,
        generation: Generation,
        name: &name_policy::Name,
    ) -> Result<()> {
        self.engine.require_write().map_err(NamespaceHostError::Source)?;
        preparation::require_namespace_write(
            self.engine, self.address.branch_id, self.root.disk(),
        )?;
        if self.address.directory != NamespaceDirectoryId::Repository(self.address.repository_id)
            || parent != self.root.binding().roots.common_data.directory_id
            || self.snapshot.tree().get_key(&name.key()).is_some()
        {
            return Err(NamespaceHostError::RootMount);
        }
        let management = crate::persist::layout::SNAPSHOT_REPOSITORY.as_os_str().to_str()
            .expect("invariant: snapshot management name is ASCII");
        if name.key() == name_policy::lookup_key(management) {
            return Err(NamespaceHostError::Management);
        }
        let common = self.held_root(self.root.binding().root_id, generation)?;
        let maximum = common.component_name_maximum().map_err(NamespaceHostError::Source)?;
        crate::name_budget::NameBudget::from_maximum(maximum)
            .map_err(NamespaceHostError::NameBudget)?
            .component(name.as_str(), name.as_str()).map_err(NamespaceHostError::NameBudget)?;
        self.require_root_inventory(true)
    }

    /// This is an exact root inventory check, not a FILE content proof. Only
    /// published namespace-bound container names are allowed; the original BODY
    /// reader separately admits the exact entity and complete selected bytes.
    fn require_root_inventory(&self, has_repository: bool) -> Result<()> {
        if self.address.directory != NamespaceDirectoryId::Repository(self.address.repository_id) {
            return Err(NamespaceHostError::RootInventory);
        }
        let common = self.root.common().map_err(NamespaceHostError::Source)?;
        let mut expected = BTreeSet::new();
        for entry in self.snapshot.tree().visible_entries() {
            require_ordinary(&entry.path)?;
            if entry.kind == NamespaceEntryKind::Directory {
                let id = entry.entry_id.parse::<EntryId>()
                    .map_err(NamespaceHostError::Identity)?;
                let record = self.engine.state().namespace_directories.get(&entry.entry_id)
                    .ok_or(NamespaceHostError::RootInventory)?;
                let owner = self.root.binding().registration.disk_id.parse()
                    .map_err(NamespaceHostError::Identity)?;
                if record.repository != self.address.repository_id
                    || record.branch != self.address.branch_id || record.owner_disk != owner
                {
                    return Err(NamespaceHostError::RootInventory);
                }
                super::ordinary_preparation::verify_namespace_directory(
                    &self.engine.config, &self.engine.disk_resources, self.engine.state(),
                    id, owner,
                ).map_err(NamespaceHostError::Source)?;
                if entry.path.parent().is_none() {
                    expected.insert(crate::persist::layout::namespace::directory(&record.name));
                }
                continue;
            }
            if entry.kind != NamespaceEntryKind::File {
                return Err(NamespaceHostError::RootInventory);
            }
            let file = entry.file_id.parse::<storage_contract::FileId>()
                .map_err(NamespaceHostError::Identity)?;
            let record = self.engine.state().file_containers.get(&file)
                .ok_or(NamespaceHostError::RootInventory)?;
            if record.namespace.repository != self.address.repository_id
                || record.namespace.branch != self.address.branch_id
                || record.namespace.entry.to_string() != entry.entry_id
                || record.namespace.name.as_str() != entry.name()
                || record.owner_disk != self.root.binding().registration.disk_id
                    .parse().map_err(NamespaceHostError::Identity)?
            {
                return Err(NamespaceHostError::RootInventory);
            }
            if entry.path.parent().is_none() {
                expected.insert(crate::persist::layout::namespace::directory(
                    &record.namespace.name,
                ));
            }
        }
        if has_repository {
            expected.insert(crate::persist::layout::SNAPSHOT_REPOSITORY.clone());
        }
        for entry in common.root_entries().map_err(NamespaceHostError::Physical)? {
            let entry = entry.map_err(|source| NamespaceHostError::Physical(
                DirectoryIoError::Filesystem(source),
            ))?;
            if !expected.remove(&entry) {
                return Err(NamespaceHostError::RootInventory);
            }
        }
        if !expected.is_empty() {
            return Err(NamespaceHostError::RootInventory);
        }
        Ok(())
    }

    fn bind(
        engine: &'a Engine,
        address: NamespaceHostAddress,
        snapshot: NamespaceSnapshot,
        repository: &'a Repository,
        origin: NamespaceOrigin,
    ) -> Result<Self> {
        // The producer's permanent origin, not file_manager_root or a matching
        // Config row, proves this common-data namespace mapping.
        let repository_id = repository.repository_id.parse()
            .map_err(NamespaceHostError::Identity)?;
        let root = engine.namespace_host_root(repository_id, address.branch_id)
            .map_err(NamespaceHostError::Source)?;
        Ok(Self { engine, address, snapshot, root, origin })
    }

    fn components(&self) -> Result<Vec<Component>> {
        let path = match self.address.directory {
            NamespaceDirectoryId::Repository(repository) => {
                if repository != self.address.repository_id {
                    return Err(NamespaceHostError::Directory);
                }
                return Ok(Vec::new());
            }
            NamespaceDirectoryId::Entry(entry) => {
                let key = entry.to_string();
                self.snapshot.tree().visible_entries().find(|item| {
                    item.kind == NamespaceEntryKind::Directory && item.entry_id == key
                }).ok_or(NamespaceHostError::Directory)?.path.clone()
            }
        };
        require_ordinary(&path)?;
        let mut paths = vec![path.clone()];
        let mut parent = path.parent();
        while let Some(path) = parent {
            if paths.len() == crate::working_tree::admission::MAX_DIRECTORY_DEPTH {
                return Err(NamespaceHostError::Depth);
            }
            parent = path.parent();
            paths.push(path);
        }
        let tree = self.snapshot.tree();
        paths.into_iter().rev().map(|path| {
            let entry = tree.get_key(path.key())
                .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
                .ok_or(NamespaceHostError::Directory)?;
            Ok(Component {
                entry: entry.entry_id.parse().map_err(NamespaceHostError::Identity)?,
                name: LookupName::parse(entry.path.file_name()).map_err(NamespaceHostError::Name)?,
            })
        }).collect()
    }

    fn held_root(&self, root: RootId, generation: Generation) -> Result<&DiskRoot> {
        if self.root.binding().root_id != root || self.root.binding().generation != generation {
            return Err(NamespaceHostError::Root);
        }
        self.root.common().map_err(NamespaceHostError::Source)
    }

    pub(crate) fn reopen_registered(
        self,
        root: RootId,
        generation: Generation,
        evidence: &HostEvidence,
        node: SnapshotNodeBinding,
    ) -> Result<HeldSnapshotNode> {
        if evidence.namespace != self.address
            || evidence.registration != self.root.binding().registration
            || evidence.root_entity != self.root.binding().root_entity
            || evidence.common_data != self.root.binding().roots.common_data
        {
            return Err(NamespaceHostError::Changed);
        }
        let components = self.components()?;
        if components.len() != evidence.directories.len() {
            return Err(NamespaceHostError::Directory);
        }
        let mut parent = self.root.binding().roots.common_data.directory_id;
        for (component, registered) in components.iter().zip(&evidence.directories) {
            require_component(self.root.binding().root_id, parent, component, registered)?;
            parent = registered.directory.directory_id;
        }
        let common = self.held_root(root, generation)?;
        let directories = open_chain(common, &evidence.directories)?;
        let host = directories.last().unwrap_or(common);
        let opened = HeldSnapshotNode::reopen_at_host(host, node)
            .map_err(NamespaceHostError::Snapshot)?;
        if !self.engine.namespace_snapshot_matches(&self.snapshot.witness()) {
            return Err(NamespaceHostError::Changed);
        }
        self.held_root(root, generation)?;
        verify_chain(common, &evidence.directories, &directories)?;
        Ok(opened)
    }
}

fn require_component(
    root: RootId,
    parent: storage_contract::working_tree::DirectoryId,
    component: &Component,
    registered: &NamespaceDirectoryBinding,
) -> Result<()> {
    let directory = &registered.directory;
    if registered.entry != component.entry || directory.root_id != root
        || directory.parent_directory_id != parent || directory.name != component.name
        || directory.lifecycle != BindingLifecycle::Current
    {
        return Err(NamespaceHostError::Changed);
    }
    Ok(())
}

fn open_chain(common: &DiskRoot, records: &[NamespaceDirectoryBinding]) -> Result<Vec<DiskRoot>> {
    let mut held = Vec::with_capacity(records.len());
    for record in records {
        let parent = held.last().unwrap_or(common);
        let name = crate::persist::layout::namespace::directory(&record.directory.name);
        let directory = parent.private_entry_matching(&name, file_identity(record.directory.entity))
            .map_err(NamespaceHostError::Physical)?;
        held.push(directory);
    }
    Ok(held)
}

fn verify_chain(
    common: &DiskRoot,
    records: &[NamespaceDirectoryBinding],
    directories: &[DiskRoot],
) -> Result<()> {
    let mut parent = common;
    for (record, held) in records.iter().zip(directories) {
        let name = crate::persist::layout::namespace::directory(&record.directory.name);
        let expected = file_identity(record.directory.entity);
        if held.held_identity().map_err(NamespaceHostError::Physical)? != expected {
            return Err(NamespaceHostError::Changed);
        }
        parent.private_entry_matching(&name, expected).map_err(NamespaceHostError::Physical)?;
        parent = held;
    }
    Ok(())
}

fn current_main(engine: &Engine, branch: BranchId) -> Result<(&Repository, NamespaceSnapshot)> {
    let branch = branch.to_string();
    let snapshot = engine.namespace_snapshot(&branch).map_err(NamespaceHostError::Source)?;
    let repository = engine.repository_for_branch(&branch).map_err(NamespaceHostError::Source)?;
    if repository.main_branch_id != branch {
        return Err(NamespaceHostError::Main);
    }
    Ok((repository, snapshot))
}

fn require_ordinary(path: &PortablePath) -> Result<()> {
    if !ordinary_path(path) {
        return Err(NamespaceHostError::Management);
    }
    Ok(())
}

pub(crate) fn ordinary_path(path: &PortablePath) -> bool {
    let management = name_policy::lookup_key(
        crate::persist::layout::SNAPSHOT_REPOSITORY.as_os_str().to_str()
            .expect("invariant: snapshot management name is ASCII"),
    );
    !path.key().split('/').any(|component| component == management)
}
