//! The explicit genesis common-root to file-manager namespace registration.
//! A matching path, repository flag or unrecorded directory is never a source.
use super::FreshLayoutError as Failure;
use crate::disk::{DiskResources, DiskRoot, decode_seal, validate_stored_seal};
use crate::engine::PlanningContext;
use crate::{DiskConfig, Engine, OperationResult, Repository, Result, State, Config};
use storage_contract::{BranchId, DiskId, OperationId, RepositoryId, Sequence};
use storage_contract::working_tree::RootBinding;
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
        if entity != crate::working_tree::records::file_identity(root.roots.common_data.entity) {
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
            != crate::working_tree::records::file_identity(
                self.origin.genesis.roots.common_data.entity,
            )
        {
            return Err(Failure::RootIdentity.into());
        }
        Ok(common)
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
        verify_origin(state, receipt, current, &origin)?;
        selected = Some(origin);
    }
    selected.ok_or_else(|| Failure::NamespaceUnmapped.into())
}

fn verify_origin(
    state: &State,
    receipt: &OperationResult,
    current: &Repository,
    origin: &RootOrigin,
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
        || current.repository_id != created.repository_id
        || current.history_tree_id != created.history_tree_id
        || current.main_branch_id != created.main_branch_id
        || current.primary_slot_id != created.primary_slot_id
        || receipt.commit_state != storage_contract::CommitState::Published
        || receipt.protection_at_publish != storage_contract::ProtectionAtPublish::NotApplicable
        || created.primary_slot_id != origin.genesis.registration.slot_id
        || receipt.commit_id.as_deref() != Some(history.root_commit_id.as_str())
        || receipt.branch_id.as_deref() != Some(created.main_branch_id.as_str())
        || receipt.history_tree_id.as_deref() != Some(created.history_tree_id.as_str())
        || !state.events.iter().any(|event| {
            event.operation_id == origin.operation && event.sequence == origin.sequence.get()
                && event.kind == receipt.kind && event.object_id == history.root_commit_id
        })
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
        .filter(|record| record.working.unborn().is_some())
    {
        if record.history_tree_id != history { return Err(Failure::NamespaceSource.into()); }
        crate::business::file_draft::unborn::validate_current_record(state, file, record)?;
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
        parent: super::directories::parent(&commit.entries, entry)?,
        name: name_policy::LookupName::parse(entry.name())?,
    })
}
