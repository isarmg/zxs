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
