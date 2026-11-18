//! Inner-source half of an explicit outer selection. This value by itself
//! proves neither outer containment, outer write permission nor publication.
use crate::directory_worktree::ledger::{Binding,Checkpoint};
use crate::directory_worktree::projection::selection::FileBranchOccurrence;
use crate::directory_worktree::store::SparseWorktreeBinding;
use crate::persist::snapshot_worktree::HostEvidence;
use crate::snapshot_worktree::{RegistrationError,Result};
use crate::working_tree::records::local_entity;
use crate::working_tree::snapshot::SnapshotNodeBinding;
use crate::{Engine,ReadFile};
use name_policy::PortablePath;
use storage_contract::{BranchKind,BranchState,CommitId,ContentHash,FileNodeId,Generation,HistoryTreeId,OperationId,VersionId};
use storage_contract::file_container::{ContainerEntity, ContainerNamespace};
use storage_contract::snapshot_worktree::{SnapshotHandle, WorktreeHandle, WorktreeRevision};
use storage_contract::working_tree::LocalEntityIdentity;

pub(crate) mod within;

/// No caller supplies a node or version.
pub(crate) struct Pick<'a> {
    pub(crate) worktree: WorktreeHandle, pub(crate) visible: OperationId,
    pub(crate) revision: WorktreeRevision, pub(crate) path: &'a PortablePath,
}

#[derive(Clone,Debug,Eq,PartialEq)]
pub(crate) struct BranchHead {
    pub(crate) commit: CommitId, pub(crate) commit_operation: OperationId, pub(crate) commit_sequence: u64,
    pub(crate) generation: Generation, pub(crate) node: FileNodeId,
    pub(crate) version: VersionId, pub(crate) history: HistoryTreeId,
    pub(crate) body_sequence: u64, pub(crate) storage: storage_contract::VersionStorageId,
    pub(crate) size: u64, pub(crate) hash: ContentHash,
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(crate) struct InnerOccurrence {
    pub(crate) worktree: WorktreeHandle, pub(crate) visible: OperationId, pub(crate) revision: WorktreeRevision,
    pub(crate) host: HostEvidence, pub(crate) mount: crate::directory_worktree::lifecycle::Mount,
    pub(crate) source: SnapshotHandle, pub(crate) source_node: SnapshotNodeBinding,
    pub(crate) published: Option<(SnapshotHandle, SnapshotNodeBinding)>,
    pub(crate) sparse: SparseWorktreeBinding, pub(crate) ledger: Binding, pub(crate) checkpoint: Checkpoint,
    pub(crate) selected: FileBranchOccurrence, pub(crate) head: BranchHead,
    pub(crate) container_namespace: ContainerNamespace, pub(crate) container_creation: OperationId,
    pub(crate) container: ContainerEntity, pub(crate) body_parent: LocalEntityIdentity,
    pub(crate) body_name: platform_io::fs_safety::EntryName, pub(crate) body: LocalEntityIdentity,
}
pub(crate) struct HeldBranchFile {
    facts: InnerOccurrence,
    body: crate::engine::HeldDirectoryFileBody,
}
impl HeldBranchFile {
    pub(crate) fn facts(&self) -> &InnerOccurrence { &self.facts }
    /// Only an authorized enclosing writer may copy. This is the original
    /// held immutable source, never a worktree write, tentative RANGE, or MOVE.
    pub(crate) fn reader(&mut self) -> &mut ReadFile { self.body.reader() }

    pub(crate) fn capture(engine: &Engine,pick: &Pick<'_>) -> Result<Self> {
        let index = engine.snapshot_receipts()?;
        Self::capture_at(engine,&index,pick)
    }

    fn capture_at(engine: &Engine, index: &super::registry::ReceiptIndex<'_>, pick: &Pick<'_>) -> Result<Self> {
        index.require_source(engine.state())?;
        if engine.admitted_current_namespaces().map_err(RegistrationError::Publication)?.is_some()
            || engine.config.generation().is_some()
        {
            return Err(RegistrationError::InitialOnly);
        }
        let registered = index.worktree(pick.worktree)?;
        if registered.mount.presence != super::registry::Presence::Visible(pick.visible)
            || registered.revision() != pick.revision { return Err(RegistrationError::PublicationChanged); }
        let _mount_descriptors = crate::engine::payload_proof_budget().reserve(7).ok_or_else(||
            RegistrationError::Publication(crate::engine::fresh_layout::FreshLayoutError::ContainerBudget.into()))?;
        let mount = registered.mount.mount.clone();
        let host = registered.source().host().clone();
        let root = engine.namespace_host_root(host.namespace.repository_id, host.namespace.branch_id)
            .map_err(RegistrationError::Publication)?;
        index.require_selection_mount(&host, &mount, root.binding(), pick.worktree, engine.config().max_entries)?;
        crate::container_storage::namespace_host::require_free_mount_at(&root, &mount.name, engine.config().max_entries)
            .map_err(RegistrationError::NamespaceHost)?;
        let source = registered.source().handle();
        let source_node = registered.source().binding();
        let selected_node = index.reopen_node(engine,source)?;
        let session = registered.reopen(&selected_node)?;
        let published = registered.published_node().map(|id| index.reopen_node(engine,id)
            .map(|node| (id,node))).transpose()?;
        use crate::directory_worktree::session::ReadMaterial;
        let material = published.as_ref().map_or(ReadMaterial::Original, |(_,node)| ReadMaterial::Published(node));
        let view = session.read(material).map_err(RegistrationError::ReadView)?;
        let selected = view.select_file_branch(pick.path).map_err(RegistrationError::ReadView)?
            .ok_or(RegistrationError::SelectedFileUnmapped)?;
        let published = published.as_ref().map(|(id,node)| (*id,node.binding()));
        let state = engine.state();
        let container = state.file_containers.get(&selected.file).ok_or(RegistrationError::Source)?;
        let disk = engine.config().disks.iter().find(|d| d.disk_id == host.registration.disk_id
            && d.slot_id == host.registration.slot_id && d.binding_generation == host.registration.binding_generation)
            .ok_or(RegistrationError::Source)?;
        if let crate::directory_worktree::projection::selection::Occurrence::Added {target,birth,..} = &selected.occurrence {
            if container.created_by != *birth || container.namespace.entry != target.entry
                || !matches!(&container.namespace.parent, storage_contract::file_container::ContainerParent::Worktree {
                    target: inner, path, .. } if inner == &target.worktree && path == &target.path)
            { return Err(RegistrationError::SelectedFileUnmapped); }
        }
        let owner = container.owner_disk;
        let replica = container.replicas.get(&owner).ok_or(RegistrationError::Source)?;
        let branch = state.branches.get(&selected.branch.to_string()).ok_or(RegistrationError::Source)?;
        let file = selected.file.to_string();
        if owner.to_string() != disk.disk_id || replica.root_id != registered.source().initialization().host.root_id {
            return Err(RegistrationError::SelectedFileDisk);
        }
        if container.namespace.repository != host.namespace.repository_id
            || container.namespace.branch != host.namespace.branch_id
            || branch.branch_kind != BranchKind::File || branch.state != BranchState::Active
            || state.file_branch_aliases.contains_key(&branch.branch_id)
            || branch.file_id.as_deref() != Some(file.as_str()) || branch.history_tree_id != container.history_tree_id.to_string()
        { return Err(RegistrationError::SelectedFileBranch); }
        let node = crate::business::file_nodes::branch_head(state,&branch.branch_id).map_err(RegistrationError::Publication)?;
        let version = crate::business::file_nodes::node_version(state,&file,&node).map_err(RegistrationError::Publication)?;
        let material = state.versions.get(&version).ok_or(RegistrationError::Source)?;
        let placement = state.placements.get(&version).ok_or(RegistrationError::Source)?;
        if material.history_tree_id != branch.history_tree_id || material.file_id != file
            || placement.version_id != version || !placement.route_slot_ids.contains(&disk.slot_id)
            || placement.binding_generations.get(&disk.slot_id) != Some(&disk.binding_generation)
        { return Err(RegistrationError::SelectedFileDisk); }
        let commit = state.commits.get(&branch.head_commit_id).ok_or(RegistrationError::SelectedFileBranch)?;
        if commit.commit_id != branch.head_commit_id || commit.history_tree_id != branch.history_tree_id {
            return Err(RegistrationError::SelectedFileBranch);
        }
        let version = version.parse().map_err(RegistrationError::Allocation)?;
        let head = BranchHead { commit: branch.head_commit_id.parse().map_err(RegistrationError::Allocation)?,
            commit_operation:commit.operation_id, commit_sequence:commit.sequence, generation: Generation::new(branch.generation),
            node: node.parse().map_err(RegistrationError::Allocation)?, body_sequence:material.version_sequence,storage:material.storage_id,
            version,history:container.history_tree_id,size:material.logical_size,hash:material.content_hash };
        let held = engine.open_directory_file_body(selected.file,version,disk).map_err(RegistrationError::Publication)?;
        let (parent,name,body) = held.identity();
        let facts = InnerOccurrence { worktree:pick.worktree, visible:pick.visible,revision:pick.revision,host,mount,
            source,source_node,published,sparse:session.instance(),ledger:registered.binding(),checkpoint:registered.checkpoint(),
            selected,head,container_namespace:container.namespace.clone(),container_creation:container.created_by,
            container:replica.entity,body_parent:local_entity(parent),body_name:name.clone(),body:local_entity(body) };
        Ok(Self { facts,body:held })
    }

    /// A detached prepare can never install against a changed branch, mount,
    /// physical parent or same-hash replacement inode without a fresh capture.
    pub(crate) fn require_current(&self,engine: &Engine) -> Result<()> {
        let f = &self.facts;
        let again = Self::capture(engine,&Pick { worktree:f.worktree,visible:f.visible,revision:f.revision,path:&f.selected.path })?;
        if again.facts != *f { return Err(RegistrationError::PublicationChanged); }
        Ok(())
    }
}
