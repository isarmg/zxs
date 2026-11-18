//! The outside half of an explicit Worktree file mapping. Capturing the
//! mapping reads original publication metadata, not BODY bytes. It creates no
//! FILE, retained name, directory snapshot, or write authority.
use super::{Engine, ReadFile};
use crate::container_storage::ObservedContainerReplica;
use crate::disk::{DiskResources, DiskRoot, VersionMaterial};
use crate::engine::{PayloadProofReservation, fresh_layout::FreshLayoutError as Failure};
use crate::{FileVersion, Result, State};
use name_policy::Name;
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use std::{io::{Seek, SeekFrom}, sync::{Arc, Weak}};
use storage_contract::{DiskId, HistoryTreeId, OperationId};
use storage_contract::file_container::{ContainerNamespace, ContainerReplica, site::FileSite};
use storage_contract::file_draft::{DraftBase, DraftLocation};
use storage_contract::working_tree::{LocalEntityIdentity, RootBinding};

/// Descriptive fields for the enclosing mapping writer, not replay evidence
/// or permission to rename an original. No independently serializable grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutsideFileFacts {
    pub(crate) target: DraftLocation,
    pub(crate) selection: DraftBase,
    pub(crate) logical_name: Name,
    pub(crate) history: HistoryTreeId,
    pub(crate) birth: OperationId,
    pub(crate) namespace: ContainerNamespace,
    pub(crate) site: FileSite,
    pub(crate) owner: DiskId,
    pub(crate) root: RootBinding,
    pub(crate) replica: ContainerReplica,
    pub(crate) body_parent: LocalEntityIdentity,
    pub(crate) body: LocalEntityIdentity,
}

/// Actual held ordinary registered source. Non-Created native sites are
/// still rejected: the spelling of a deleted-data entry isn't a positive
/// proof of a retained FILE transition.
pub(crate) struct HeldOutsideFile {
    state: Weak<State>,
    resources: Weak<DiskResources>,
    facts: OutsideFileFacts,
    version: FileVersion,
    scope: Arc<ObservedContainerReplica>,
    parent: DiskRoot,
    leaf: EntryName,
    body: ReadFile,
    _permit: PayloadProofReservation,
}

impl HeldOutsideFile {
    /// Caller holds the source account/SMB authorization and the destination
    /// Worktree authorization separately. Neither is inferred from these facts.
    pub(crate) fn capture(engine: &Engine, target: &DraftLocation) -> Result<Self> {
        let context = engine.planning_context();
        let state = context.state;
        let record = state.file_containers.get(&target.file).ok_or(Failure::ContainerScope)?;
        if !matches!(record.site, FileSite::Created {})
            || record.namespace.parent.ordinary().is_none()
            || record.working.unborn().is_some() || record.working.draft().is_some()
        {
            return Err(Failure::WholeContainerLifecycleUnsupported.into());
        }
        // Exactly the occurrence and formal node the native SMB reader exposes.
        // No branch override, global FILE fallback, draft, or "latest by time".
        let selection = engine.file_draft_base(target)?;
        let version_id = selection.version_id().to_string();
        let version = state.versions.get(&version_id).ok_or(Failure::ContainerScope)?;
        let placement = state.placements.get(&version_id).ok_or(Failure::ContainerScope)?;
        if version.logical_size > context.config.max_file_bytes {
            return Err(Failure::ContainerBudget.into());
        }
        let logical_name = crate::business::file_nodes::committed_name(state, target.file, selection.node_id())?;
        let registered = context.namespace_host_root(target.repository, record.namespace.branch)?;
        let disk = registered.disk();
        let root = registered.binding();
        let owner = disk.disk_id.parse::<DiskId>()?;
        let replica = record.replicas.get(&owner).ok_or(Failure::ContainerScope)?;
        if record.owner_disk != owner || record.replicas.len() != 1 || !replica.prefixes.is_empty()
            || root.root_id != replica.root_id || !root.source_chain.is_empty()
            || placement.target_copies != 1 || placement.primary_slot_id != disk.slot_id
            || placement.route_slot_ids.as_slice() != [disk.slot_id.as_str()]
            || placement.binding_generations.get(&disk.slot_id) != Some(&disk.binding_generation)
        {
            return Err(Failure::ContainerScope.into());
        }
        let permit = crate::engine::payload_proof_budget().reserve(4).ok_or(Failure::ContainerBudget)?;
        // with_replica admits the immutable Version's original publication.
        // The singleton route above prevents substituting a healthy backup.
        let (scope, parent, leaf, body, parent_id, entity) = engine.with_replica(
            &version_id, |scope, material, exact, _lease| {
                if exact != version || !Arc::ptr_eq(scope.disk(), context.disk_resources.get(disk)?) {
                    return Err(Failure::ContainerScope.into());
                }
                let container = material.container()?;
                let versions = container.path().join(crate::persist::layout::VERSIONS.as_path());
                let path = material.body_path()?;
                if path.parent() != Some(versions.as_path()) { return Err(Failure::ContainerScope.into()); }
                let leaf = EntryName::new(path.file_name().ok_or(Failure::ContainerScope)?)
                    .map_err(|_| Failure::ContainerScope)?;
                let parent_id = container.directory_identity(&versions)?;
                let parent = container.private_child_matching(&versions, parent_id)?;
                let body = parent.open_read_entry(&leaf)?;
                let info = body.metadata()?;
                if info.len() != version.logical_size { return Err(Failure::ContainerIdentity.into()); }
                let entity = FileIdentity::from_metadata(&info);
                Ok((Arc::clone(scope), parent, leaf, body, parent_id, entity))
            },
        )?;
        let held = Self {
            state: Arc::downgrade(state), resources: Arc::downgrade(context.disk_resources),
            facts: OutsideFileFacts {
                target: target.clone(), selection, logical_name, history: record.history_tree_id,
                birth: record.created_by, namespace: record.namespace.clone(), site: record.site.clone(),
                owner, root: root.clone(), replica: replica.clone(),
                body_parent: crate::working_tree::records::local_entity(parent_id),
                body: crate::working_tree::records::local_entity(entity),
            },
            version: version.clone(), scope, parent, leaf, body, _permit: permit,
        };
        held.require_current(engine)?;
        Ok(held)
    }

    pub(crate) fn facts(&self) -> &OutsideFileFacts { &self.facts }

    /// The enclosing permanent mapping can record these exact source facts.
    /// It never stores FDs or physical pathnames, and no FILE reader accepts
    /// an unaccompanied decoded witness in place of native occurrence proof.
    pub(crate) fn witness(&self) -> Result<crate::persist::snapshot_worktree::outside_selection::OutsideSelectionWitness> {
        use crate::persist::snapshot_worktree::outside_selection::{OutsideSelectionFields, OutsideSelectionWitness};
        let source = &self.facts;
        let reg = &source.root.registration;
        let fields = OutsideSelectionFields {
            target: source.target.clone(), selection: source.selection, logical_name: source.logical_name.clone(),
            history: source.history, birth: source.birth, namespace_branch: source.namespace.branch,
            original_parent: source.namespace.parent.ordinary().ok_or(Failure::ContainerScope)?,
            site: source.site.witness(source.target.file)?, pool: reg.pool_id.parse()?, owner: source.owner,
            slot: name_policy::SlotId::parse(&reg.slot_id)?, owner_generation: storage_contract::Generation::new(reg.binding_generation),
            root: source.root.root_id, root_entity: source.root.root_entity,
            container_preparation: source.replica.preparation, container: source.replica.entity,
            body_parent: source.body_parent, body: source.body, storage: self.version.storage_id,
            size: self.version.logical_size, hash: self.version.content_hash,
            version_sequence: storage_contract::Sequence::new(self.version.version_sequence),
        };
        OutsideSelectionWitness::checked(fields).ok_or_else(|| Failure::ContainerScope.into())
    }

    /// Warm read of exactly the immutable node that this original physical
    /// selector saw. Advancing the current FILE/draft never advances a stored
    /// mapping. This is still only its FILE-side operand: it does not prove a
    /// mapping operation, current authority over the Worktree, or its revision.
    pub(crate) fn open_pinned_node(&self, engine: &Engine) -> Result<ReadFile> {
        let context = engine.planning_context();
        let entity = crate::working_tree::records::file_identity(self.facts.body);
        if !Weak::ptr_eq(&self.resources, &Arc::downgrade(context.disk_resources))
            || FileIdentity::from_metadata(&self.body.metadata()?) != entity
        {
            return Err(Failure::ContainerIdentity.into());
        }
        read_pinned(engine, &self.witness()?)
    }

    /// Enclosing writer checks this while revalidating its current session
    /// and original occurrence. No inference that an occurrence was (or was
    /// not) previously included in a permanent Worktree commit.
    pub(crate) fn require_current(&self, engine: &Engine) -> Result<()> {
        let context = engine.planning_context();
        if !Weak::ptr_eq(&self.state, &Arc::downgrade(context.state))
            || !Weak::ptr_eq(&self.resources, &Arc::downgrade(context.disk_resources))
        {
            return Err(Failure::DraftChanged.into());
        }
        let container = self.scope.container()?;
        let material = VersionMaterial::current(&self.scope, &self.version)?;
        let versions = container.path().join(crate::persist::layout::VERSIONS.as_path());
        let parent_id = crate::working_tree::records::file_identity(self.facts.body_parent);
        let entity = crate::working_tree::records::file_identity(self.facts.body);
        if self.parent.path() != versions || container.directory_identity(&versions)? != parent_id
            || material.body_path()? != versions.join(self.leaf.as_path())
        {
            return Err(Failure::ContainerIdentity.into());
        }
        let actual = self.parent.open_read_entry(&self.leaf)?;
        let info = actual.metadata()?;
        if info.len() != self.version.logical_size || FileIdentity::from_metadata(&info) != entity
            || FileIdentity::from_metadata(&self.body.metadata()?) != entity
        {
            return Err(Failure::ContainerIdentity.into());
        }
        self.scope.verify()
    }

    /// Only when separately authorized to copy, verify the immutable BODY
    /// and its authenticated checksums. An equal replacement is not the
    /// source. A serialized mapping alone cannot open this descriptor.
    pub(crate) fn open_verified_for_copy(&self, engine: &Engine) -> Result<ReadFile> {
        self.require_current(engine)?;
        let mut file = VersionMaterial::current(&self.scope, &self.version)?.verify_payload()?;
        if FileIdentity::from_metadata(&file.metadata()?)
            != crate::working_tree::records::file_identity(self.facts.body)
        {
            return Err(Failure::ContainerIdentity.into());
        }
        file.seek(SeekFrom::Start(0))?;
        self.require_current(engine)?;
        Ok(file)
    }
}

// Private intentionally: a public-in-crate obligation cannot be discharged
// by deserializing a witness. The warm caller above already holds the original
// selected FILE. Cold callers need the original admitted native mapping loan.
fn read_pinned(engine: &Engine,
    witness: &crate::persist::snapshot_worktree::outside_selection::OutsideSelectionWitness,
) -> Result<ReadFile> {
    use crate::business::file_draft::ordinary::OrdinaryOccurrence;
    use crate::business::file_nodes;
    use crate::{BranchKind, BranchState, ContentState};
    let f = witness.fields();
    let context = engine.planning_context();
    let state = context.state;
    if context.config.generation().is_some() || context.current_namespaces.is_some()
        || state.file_deletions.contains_key(&f.target.file.to_string())
    {
        return Err(Failure::WholeContainerLifecycleUnsupported.into());
    }
    let record = state.file_containers.get(&f.target.file).ok_or(Failure::ContainerScope)?;
    let owner = context.namespace_host_root(f.target.repository, f.namespace_branch)?;
    let root = owner.binding();
    let replica = record.replicas.get(&f.owner).ok_or(Failure::ContainerScope)?;
    if record.owner_disk != f.owner || record.history_tree_id != f.history || record.created_by != f.birth
        || record.namespace.repository != f.target.repository || record.namespace.branch != f.namespace_branch
        || record.namespace.entry != f.target.entry || record.namespace.parent.ordinary().is_none()
        || record.working.unborn().is_some() || !matches!(record.site, FileSite::Created {})
        || record.site.witness(f.target.file)? != f.site
        || record.replicas.len() != 1 || !replica.prefixes.is_empty()
        || replica.preparation != f.container_preparation || replica.entity != f.container || replica.root_id != f.root
        || root.root_id != f.root || root.root_entity != f.root_entity || !root.source_chain.is_empty()
        || root.registration.pool_id != f.pool.to_string() || root.registration.disk_id != f.owner.to_string()
        || root.registration.slot_id != f.slot.as_str() || root.registration.binding_generation != f.owner_generation.get()
    {
        return Err(Failure::ContainerScope.into());
    }
    // The source can have a newly committed branch HEAD or a dirty current
    // draft. Locate its true current registration independently; only the
    // original mapped immutable node, not current visible bytes, is read.
    let at = OrdinaryOccurrence::capture(state, f.target.file, record)?;
    let index = engine.snapshot_receipts().map_err(crate::business::file_draft::worktree::failure)?;
    if index.contains_nested_draft_host(f.target.repository, f.namespace_branch, &at.parents) {
        return Err(Failure::ContainerScope.into());
    }
    let file = f.target.file.to_string();
    let id = f.selection.version_id().to_string();
    let selected = f.selection.node_id();
    let branch = state.branches.get(&f.selection.branch_id().to_string()).ok_or(Failure::ContainerScope)?;
    let placement = state.placements.get(&id).ok_or(Failure::ContainerScope)?;
    if branch.branch_kind != BranchKind::File || branch.state != BranchState::Active
        || branch.file_id.as_deref() != Some(file.as_str()) || branch.history_tree_id != f.history.to_string()
        || state.file_branch_aliases.contains_key(&branch.branch_id)
        || state.content_states.get(&id) != Some(&ContentState::Available)
        || file_nodes::node_version(state, &file, &selected.to_string())? != id
        || file_nodes::committed_name(state, f.target.file, selected)? != f.logical_name
        || placement.target_copies != 1 || placement.primary_slot_id != f.slot.as_str()
        || placement.route_slot_ids.as_slice() != [f.slot.as_str()]
        || placement.binding_generations.get(f.slot.as_str()) != Some(&f.owner_generation.get())
    {
        return Err(Failure::ContainerScope.into());
    }
    let disk = owner.disk();
    engine.with_replica(&id, |scope, material, version, lease| {
        if version.file_id != file || version.history_tree_id != f.history.to_string()
            || version.storage_id != f.storage || version.logical_size != f.size
            || version.content_hash != f.hash || version.version_sequence != f.version_sequence.get()
            || !Arc::ptr_eq(context.disk_resources.get(disk)?, scope.disk())
        {
            return Err(Failure::ContainerScope.into());
        }
        let _permit = crate::engine::payload_proof_budget().reserve(3).ok_or(Failure::ContainerBudget)?;
        let held = scope.container()?;
        let directory = held.path().join(crate::persist::layout::VERSIONS.as_path());
        let parent = crate::working_tree::records::file_identity(f.body_parent);
        if held.directory_identity(&directory)? != parent { return Err(Failure::ContainerIdentity.into()); }
        let versions = held.private_child_matching(&directory, parent)?;
        let expected = material.body_path()?;
        if expected.parent() != Some(directory.as_path()) { return Err(Failure::ContainerScope.into()); }
        let leaf = EntryName::new(expected.file_name().ok_or(Failure::ContainerScope)?)
            .map_err(|_| Failure::ContainerScope)?;
        let mut actual = super::verify_read_payload(material, version, lease)?;
        let named = versions.open_read_entry(&leaf)?;
        let entity = crate::working_tree::records::file_identity(f.body);
        if FileIdentity::from_metadata(&actual.metadata()?) != entity || FileIdentity::from_metadata(&named.metadata()?) != entity
            || held.directory_identity(&directory)? != parent
        {
            return Err(Failure::ContainerIdentity.into());
        }
        actual.seek(SeekFrom::Start(0))?;
        Ok(ReadFile::in_container(actual, Arc::clone(scope)))
    })
}
