//! The explicit genesis common-root to file-manager namespace registration.
//! A matching path, repository flag or unrecorded directory is never a source.
use super::FreshLayoutError as Failure;
use crate::disk::{DiskResources, DiskRoot, decode_seal, validate_stored_seal};
use crate::engine::PlanningContext;
use crate::{DiskConfig, Engine, OperationResult, Repository, Result, State, Config};
use storage_contract::{BranchId, DiskId, OperationId, RepositoryId, Sequence};
use storage_contract::namespace_root::RootBinding;
use crate::persist::namespace_root::{self, RootOrigin};

mod backup;
pub(crate) use backup::{PreparedBackupSourcePath, PublishedBackupSourcePath};
pub(super) use backup::{read_backup_source_path, published_backup_source_path};

impl PlanningContext<'_> {
    /// Only the actual new file-manager root producer calls this, while its
    /// freshly allocated namespace is still the owning mutation's private State.
    pub(crate) fn prepare_namespace_root_origin(
        &self,
        next: &State,
        repository: &Repository,
        operation: OperationId,
    ) -> Result<RootOrigin> {
        if self.current_namespaces.is_some() || self.config.generation().is_some()
            || self.blocked || next.sequence != self.next_slot
            || self.state.operations.contains_key(&operation)
            || self.state.repositories.contains_key(&repository.repository_id)
            || self.state.histories.contains_key(&repository.history_tree_id)
            || self.state.branches.contains_key(&repository.main_branch_id)
            || next.repositories.get(&repository.repository_id) != Some(repository)
            || !repository.file_manager_root || repository.target_copies != 1
        {
            return Err(Failure::NamespaceSource.into());
        }
        let disk = self.disk(&repository.primary_slot_id)?;
        if disk.level != 1 || disk.upstream_slot_id.is_some()
            || self.state.repositories.values().any(|existing| {
                existing.file_manager_root && existing.primary_slot_id == disk.slot_id
            })
        {
            return Err(Failure::NamespaceSource.into());
        }
        let root = self.config.storage_roots.get(&disk.disk_id.parse::<DiskId>()?)
            .ok_or(Failure::Registration)?;
        let handle = self.disk_resources.get(disk)?;
        super::verify_root(self.config, disk, &handle.root)?;
        let seal = decode_seal(&handle.root.read_bytes(
            &disk.root.join(crate::persist::layout::DISK_IDENTITY.as_path()),
            crate::disk::MAX_METADATA,
        )?)?;
        validate_stored_seal(&seal, &handle.seal)?;
        let common = handle.content_root()?;
        let entity = common.directory_identity(common.path())?;
        if entity != crate::namespace_roots::file_identity(root.roots.common_data.entity) {
            return Err(Failure::RootIdentity.into());
        }
        // Genesis already owns this exact empty common directory. We do not
        // map an empty logical namespace over earlier or unexplained material.
        // Child producers subsequently use exclusive creation, never adoption.
        if common.entries(common.path())?.next().transpose()?.is_some() {
            return Err(Failure::NamespaceSource.into());
        }
        let history = next.histories.get(&repository.history_tree_id)
            .ok_or(Failure::NamespaceSource)?;
        let commit = next.commits.get(&history.root_commit_id)
            .ok_or(Failure::NamespaceSource)?;
        if history.repository_id != repository.repository_id
            || history.main_branch_id.as_deref() != Some(repository.main_branch_id.as_str())
            || !commit.entries.is_empty() || !commit.parent_commit_ids.is_empty()
            || commit.sequence != next.sequence
        {
            return Err(Failure::NamespaceSource.into());
        }
        Ok(RootOrigin {
            repository: repository.repository_id.parse()?,
            history: repository.history_tree_id.parse()?,
            branch: repository.main_branch_id.parse()?,
            operation,
            sequence: Sequence::new(next.sequence),
            genesis: root.clone(),
        })
    }
}

/// Borrowed rows, not a movement or storage capability. This authenticates
/// the ordinary root's original creation on an already admitted State and
/// pairs the exact structural HEAD with its current registry projection.
pub(crate) fn ordinary_root_basis<'a>(
    state: &'a State, current: Option<&'a crate::engine::CurrentNamespaces>,
    repository: RepositoryId,
) -> Result<(&'a str, &'a [crate::Entry])> {
    let owner = state.repositories.get(&repository.to_string()).ok_or(Failure::NamespaceUnmapped)?;
    let original = root_origin(state, repository, owner.main_branch_id.parse()?)?;
    let branch = state.branches.get(&owner.main_branch_id)
        .filter(|b| b.branch_id == owner.main_branch_id && b.file_id.is_none()
            && b.branch_kind == crate::BranchKind::Full && b.history_tree_id == owner.history_tree_id)
        .ok_or(Failure::NamespaceSource)?;
    let head = state.commits.get(&branch.head_commit_id)
        .filter(|c| c.commit_id == branch.head_commit_id && c.history_tree_id == owner.history_tree_id)
        .ok_or(Failure::NamespaceSource)?;
    if original.history.to_string() != head.history_tree_id { return Err(Failure::NamespaceSource.into()); }
    let entries = match current {
        Some(view) => view.branch_entries(&branch.branch_id, &head.commit_id)?,
        None => &head.entries,
    };
    Ok((&head.commit_id, entries))
}

/// A read source for exactly the registered namespace. It supplies neither a
/// metadata decision nor permission to create arbitrary children. Writers must
/// still consume the original namespace/EntryId and their own operation gate.
pub(crate) struct RegisteredNamespaceRoot<'a> {
    // Both adapters borrow the actual admitted source, not an equivalent State.
    _state: &'a State,
    config: &'a Config,
    resources: &'a DiskResources,
    disk: &'a DiskConfig,
    origin: RootOrigin,
}
impl RegisteredNamespaceRoot<'_> {
    pub(crate) fn binding(&self) -> &RootBinding {
        &self.origin.genesis
    }

    pub(crate) fn disk(&self) -> &DiskConfig {
        self.disk
    }

    pub(crate) fn deleted(&self) -> Result<&DiskRoot> {
        // Common() closes the unchanged original root, seal and registration.
        self.common()?;
        let deleted = self.resources.get(self.disk)?.deleted_root()?;
        if deleted.directory_identity(deleted.path())?
            != crate::namespace_roots::file_identity(
                self.origin.genesis.roots.deleted_data.entity,
            )
        {
            return Err(Failure::RootIdentity.into());
        }
        Ok(deleted)
    }

    pub(crate) fn common(&self) -> Result<&DiskRoot> {
        let handle = self.resources.get(self.disk)?;
        super::verify_bound_root(self.config, self.disk, &handle.root)?;
        let seal = decode_seal(&handle.root.read_bytes(
            &self.disk.root.join(crate::persist::layout::DISK_IDENTITY.as_path()),
            crate::disk::MAX_METADATA,
        )?)?;
        validate_stored_seal(&seal, &handle.seal)?;
        let common = handle.content_root()?;
        if common.directory_identity(common.path())?
            != crate::namespace_roots::file_identity(
                self.origin.genesis.roots.common_data.entity,
            )
        {
            return Err(Failure::RootIdentity.into());
        }
        Ok(common)
    }
}

impl Engine {
    /// Management listing of an ordinary root. Enabled retains the physical
    /// original-owner check. A Disabled binding lends only its positively
    /// proved original namespace and current published listing; it cannot
    /// open files, authorize a write, create a root, or expose detached rows.
    pub fn ordinary_metadata_root_for_slot(&self, slot: &str) -> Result<Option<&Repository>> {
        use crate::RegistrationMode;
        let disk = self.disk(slot)?;
        let Some(view) = self.admitted_current_namespaces()? else {
            return self.ordinary_root_for_slot(&disk.slot_id);
        };
        let member = self.registered_disk(&disk.disk_id).ok_or(Failure::Registration)?;
        if member.pool_id != self.config.pool_id || member.slot_id != disk.slot_id
            || member.binding_generation != disk.binding_generation
            || member.level != disk.level || member.upstream_slot_id != disk.upstream_slot_id.as_deref()
        {
            return Err(Failure::Registration.into());
        }
        if member.registration_mode == RegistrationMode::Enabled {
            return self.ordinary_root_for_slot(&disk.slot_id);
        }
        // Absence from a stopped disk is not proof that the slot has no
        // ordinary namespace. There is no open/stat, physical fallback or
        // current-repo-by-name substitution in this branch.
        let repository = self.ordinary_root_candidate(&disk.slot_id)?
            .ok_or(Failure::NamespaceUnmapped)?;
        let meta = registered_namespace(&self.state, &self.config,
            repository.repository_id.parse()?, repository.main_branch_id.parse()?)?;
        let branch = self.state.branches.get(&repository.main_branch_id)
            .ok_or(Failure::NamespaceSource)?;
        if meta.origin.genesis.registration != member.identity()
            || meta.disk.disk_id != disk.disk_id || branch.branch_id != repository.main_branch_id
            || branch.history_tree_id != repository.history_tree_id || branch.file_id.is_some()
        {
            return Err(Failure::NamespaceSource.into());
        }
        view.branch_entries(&branch.branch_id, &branch.head_commit_id)?;
        Ok(Some(repository))
    }
}

impl Engine {
    pub(crate) fn namespace_host_root(
        &self,
        repository: RepositoryId,
        branch: BranchId,
    ) -> Result<RegisteredNamespaceRoot<'_>> {
        registered_root(&self.state, &self.config, &self.disk_resources, repository, branch)
    }
}

impl PlanningContext<'_> {
    pub(crate) fn namespace_host_root(
        &self,
        repository: RepositoryId,
        branch: BranchId,
    ) -> Result<RegisteredNamespaceRoot<'_>> {
        registered_root(self.state, self.config, self.disk_resources, repository, branch)
    }
}

impl crate::engine::DirectoryReplayPrefix<'_> {
    pub(crate) fn namespace_host_root(
        &self, repository: RepositoryId, branch: BranchId,
    ) -> Result<RegisteredNamespaceRoot<'_>> {
        registered_root(self.previous(), self.config(), self.resources(), repository, branch)
    }
}

// The public adapters above borrow the actual Engine/PlanningContext input.
// Sibling ordinary FILE admission uses this same kernel on its actual input.
// No caller outside this source module family can mint from a decoded record.
pub(super) fn registered_root<'a>(
    state: &'a State,
    config: &'a Config,
    resources: &'a DiskResources,
    repository: RepositoryId,
    branch: BranchId,
) -> Result<RegisteredNamespaceRoot<'a>> {
    let NamespaceRegistration { state, config, disk, origin } =
        registered_namespace(state, config, repository, branch)?;
    let source = RegisteredNamespaceRoot { _state: state, config, resources, disk, origin };
    source.common()?;
    Ok(source)
}

/// Logical ownership from the admitted publication and its exact configured
/// registration. It grants no root access: owner writers still require common(),
/// while an existing backup read separately observes only its actual target.
struct NamespaceRegistration<'a> {
    state: &'a State,
    config: &'a Config,
    disk: &'a DiskConfig,
    origin: RootOrigin,
}

/// Descriptive source from an actual already-published replay loan. Unlike
/// RegisteredNamespaceRoot it cannot open the owner's common area.
pub(crate) struct PublishedNamespaceMetadata<'a>(NamespaceRegistration<'a>);
impl PublishedNamespaceMetadata<'_> {
    pub(crate) fn binding(&self) -> &RootBinding { &self.0.origin.genesis }
    pub(crate) fn disk(&self) -> &DiskConfig { self.0.disk }
    pub(crate) fn require_backup_target(&self, target: DiskId) -> Result<()> {
        backup::require_registered_path(&self.0, target)
    }
}
impl PlanningContext<'_> {
    pub(crate) fn published_namespace_metadata(
        &self, proof: &crate::engine::ReadOnlyPublishedCandidate<'_>,
        repository: RepositoryId, branch: BranchId,
    ) -> Result<PublishedNamespaceMetadata<'_>> {
        proof.require_context(self)?;
        Ok(PublishedNamespaceMetadata(registered_namespace(
            self.state, self.config, repository, branch,
        )?))
    }
}

impl crate::engine::ArchivedBackupPublication<'_> {
    /// The immutable original archive needs registered geometry, not access to
    /// the former owner's live common root. The complete container transition
    /// still verifies every changed physical record against this same Config.
    pub(crate) fn namespace_metadata(
        &self, repository: RepositoryId, branch: BranchId,
    ) -> Result<PublishedNamespaceMetadata<'_>> {
        Ok(PublishedNamespaceMetadata(registered_namespace(
            self.previous(), self.config(), repository, branch,
        )?))
    }
}

fn registered_namespace<'a>(
    state: &'a State, config: &'a Config, repository: RepositoryId, branch: BranchId,
) -> Result<NamespaceRegistration<'a>> {
    let origin = root_origin(state, repository, branch)?;
    let disk = config.disks.iter().find(|disk| {
        disk.disk_id == origin.genesis.registration.disk_id
    }).ok_or(Failure::Registration)?;
    let id = disk.disk_id.parse::<DiskId>()?;
    if config.storage_roots.get(&id) != Some(&origin.genesis) {
        return Err(Failure::RootIdentity.into());
    }
    Ok(NamespaceRegistration { state, config, disk, origin })
}

pub(super) fn root_origin(
    state: &State, repository: RepositoryId, branch: BranchId,
) -> Result<RootOrigin> {
    let current = state.repositories.get(&repository.to_string())
        .ok_or(Failure::NamespaceUnmapped)?;
    if !current.file_manager_root || current.main_branch_id != branch.to_string() {
        return Err(Failure::NamespaceUnmapped.into());
    }
    let mut selected = None;
    for receipt in state.operations.values() {
        let Some(origin) = namespace_root::origin(receipt)? else { continue; };
        if origin.repository != repository {
            continue;
        }
        if selected.is_some() || origin.branch != branch {
            return Err(Failure::NamespaceSource.into());
        }
        let event = state.events.iter().find(|event|
            event.operation_id == origin.operation && event.kind == receipt.kind);
        verify_origin(state, receipt, current, &origin, event)?;
        selected = Some(origin);
    }
    selected.ok_or_else(|| Failure::NamespaceUnmapped.into())
}

/// Complete native State, not a standalone history summary, proves which
/// ordinary structural roots exist. A true flag, a current name or the absence
/// of a current row cannot substitute for the original registration.
pub(crate) fn require_root_origins(state: &State) -> Result<()> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut events = BTreeMap::new();
    for event in state.events.iter().filter(|event| event.kind == "repository.create") {
        if events.len() == crate::namespace_roots::MAX_ROOTS
            || events.insert(event.operation_id, event).is_some()
        {
            return Err(Failure::NamespaceSource.into());
        }
    }
    let mut registered = BTreeSet::new();
    let mut geometry = Vec::new();
    for (id, receipt) in state.operations.iter().filter(|(_, row)| row.kind == "repository.create") {
        let origin = namespace_root::origin(receipt)?.ok_or(Failure::NamespaceSource)?;
        if *id != origin.operation || !origin.genesis.source_chain.is_empty()
            || registered.len() == crate::namespace_roots::MAX_ROOTS
            || !registered.insert(origin.repository)
        {
            return Err(Failure::NamespaceSource.into());
        }
        let current = state.repositories.get(&origin.repository.to_string())
            .ok_or(Failure::NamespaceSource)?;
        verify_origin(state, receipt, current, &origin, events.remove(id))?;
        geometry.push(origin.genesis);
    }
    if !events.is_empty() || registered.len() != state.repositories.len() {
        return Err(Failure::NamespaceSource.into());
    }
    crate::namespace_roots::inspect(geometry.iter())
        .map_err(|_| Failure::NamespaceSource)?;
    Ok(())
}

fn verify_origin(
    state: &State,
    receipt: &OperationResult,
    current: &Repository,
    origin: &RootOrigin,
    event: Option<&crate::Event>,
) -> Result<()> {
    let created = namespace_root::created_repository(receipt)?;
    let principal = state.operation_principals.get(&origin.operation)
        .ok_or(Failure::NamespaceSource)?;
    namespace_root::require_creation_request(receipt, &created, principal)?;
    let history = state.histories.get(&current.history_tree_id)
        .ok_or(Failure::NamespaceSource)?;
    let root_commit = state.commits.get(&history.root_commit_id)
        .ok_or(Failure::NamespaceSource)?;
    if root_commit.kind != "root" || root_commit.sequence != origin.sequence.get()
        || root_commit.operation_id != origin.operation || root_commit.author != *principal
        || root_commit.history_tree_id != created.history_tree_id
        || !root_commit.entries.is_empty() || !root_commit.parent_commit_ids.is_empty()
        || !root_commit.pending_file_versions.is_empty() || root_commit.provenance.is_some()
        || root_commit.message != "Create repository"
        || created.name != format!("ZXS disk {}", created.primary_slot_id)
        || !created.smb_enabled || created.smb_binding_generation != 1
        || receipt.kind != "repository.create" || receipt.operation_id != origin.operation
        || receipt.sequence != origin.sequence.get()
        || !created.file_manager_root || created.target_copies != 1
        || created.repository_id != origin.repository.to_string()
        || created.history_tree_id != origin.history.to_string()
        || created.main_branch_id != origin.branch.to_string()
        || !current.file_manager_root || current.repository_id != created.repository_id
        || current.history_tree_id != created.history_tree_id
        || current.main_branch_id != created.main_branch_id
        || current.primary_slot_id != created.primary_slot_id
        || receipt.commit_state != storage_contract::CommitState::Published
        || receipt.protection_at_publish != storage_contract::ProtectionAtPublish::NotApplicable
        || created.primary_slot_id != origin.genesis.registration.slot_id
        || receipt.commit_id.as_deref() != Some(history.root_commit_id.as_str())
        || receipt.branch_id.as_deref() != Some(created.main_branch_id.as_str())
        || receipt.history_tree_id.as_deref() != Some(created.history_tree_id.as_str())
        || !event.is_some_and(|event| event.operation_id == origin.operation
            && event.sequence == origin.sequence.get() && event.kind == receipt.kind
            && event.object_id == history.root_commit_id)
    {
        return Err(Failure::NamespaceSource.into());
    }
    Ok(())
}

/// Pure address projection shared by planning and cold transition admission.
/// Only the actual registered file-manager main namespace is supported here.
/// Subdirectory parents require their own published DirectoryBinding chain.
pub(super) fn file_target(
    state: &State,
    file: storage_contract::FileId,
    history: storage_contract::HistoryTreeId,
) -> Result<storage_contract::file_container::ContainerNamespace> {
    if let Some(record) = state.file_containers.get(&file)
        .filter(|record| record.working.unborn().is_some() || record.namespace.parent.is_worktree())
    {
        if record.history_tree_id != history { return Err(Failure::NamespaceSource.into()); }
        if record.namespace.parent.is_worktree() {
            crate::business::file_draft::worktree::validate_record(state, file, record)?;
        } else {
            crate::business::file_draft::unborn::validate_current_record(state, file, record)?;
        }
        return Ok(record.namespace.clone());
    }
    let history = state.histories.get(&history.to_string()).ok_or(Failure::NamespaceSource)?;
    let repository = state.repositories.get(&history.repository_id)
        .filter(|repository| repository.file_manager_root)
        .ok_or(Failure::NamespaceUnmapped)?;
    let branch = state.branches.get(&repository.main_branch_id)
        .filter(|branch| branch.file_id.is_none() && branch.state == crate::BranchState::Active)
        .ok_or(Failure::NamespaceSource)?;
    let commit = state.commits.get(&branch.head_commit_id).ok_or(Failure::NamespaceSource)?;
    let file_key = file.to_string();
    let mut entries = commit.entries.iter().filter(|entry| {
        entry.kind == crate::NamespaceEntryKind::File && entry.file_id == file_key
    });
    let entry = entries.next().ok_or(Failure::NamespaceUnmapped)?;
    if entries.next().is_some() {
        return Err(Failure::NamespaceUnmapped.into());
    }
    Ok(storage_contract::file_container::ContainerNamespace {
        repository: repository.repository_id.parse()?,
        branch: branch.branch_id.parse()?,
        entry: entry.entry_id.parse()?,
        parent: super::directories::parent(&commit.entries, entry)?.into(),
        name: name_policy::LookupName::parse(entry.name())?,
    })
}
