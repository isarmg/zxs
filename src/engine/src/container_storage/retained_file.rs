//! Read-only preparation of two exact parents of one existing ordinary FILE.
//! This is a physical operand, *not* a FINALIZE, a restore token, or a writer.
use crate::business::file_draft;
use crate::disk::{DiskHandle, DiskResources, DiskRoot};
use crate::engine::fresh_layout::{containers, FreshLayoutError as Failure};
use crate::{ContentState, Engine, Result, State};
use std::sync::{Arc, Weak};
use storage_contract::file_container::{
    retained::RetainedAllocation, ContainerEntity, ContainerParent, ContainerReplica,
};
use storage_contract::file_draft::{DraftBase, DraftLocation, FileWorkingState};
use storage_contract::working_tree::{LocalEntityIdentity, RootBinding};
use storage_contract::{DiskId, HistoryTreeId, OperationId, Principal};
use super::ObservedContainerReplica;

/// Actual registered identities and fixed spellings. Even an exact clone is
/// descriptive only. Installation will require an original decided successor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedFileSite {
    pub(crate) target: DraftLocation,
    pub(crate) base: DraftBase,
    pub(crate) history: HistoryTreeId,
    pub(crate) file_birth: OperationId,
    pub(crate) owner: DiskId,
    pub(crate) replica: ContainerReplica,
    pub(crate) root: RootBinding,
    pub(crate) allocation: RetainedAllocation,
    pub(crate) operation: OperationId,
    pub(crate) actor: Principal,
}

pub(crate) struct HeldRetainedEndpoints {
    state: Weak<State>,
    resources: Weak<DiskResources>,
    source: ObservedContainerReplica,
    facts: ReviewedFileSite,
}

impl HeldRetainedEndpoints {
    /// Captures physical feasibility, not absence of external references or
    /// complete FILE/DISK/SYSTEM closure. No journal, file or directory writes.
    pub(crate) fn review(
        engine: &Engine, target: &DraftLocation, actor: &Principal, operation: OperationId,
    ) -> Result<Self> {
        let context=engine.planning_context();
        context.require_write()?;
        let state=context.state;
        let record=state.file_containers.get(&target.file).ok_or(Failure::ContainerScope)?;
        if context.config.generation().is_some() || context.current_namespaces.is_some()
            || state.operations.contains_key(&operation) || state.file_deletions.contains_key(&target.file.to_string())
            || target.path.parent().is_some() || record.namespace.parent!=ContainerParent::Root
            || !matches!(record.working,FileWorkingState::ManualClean{..}) || record.replicas.len()!=1
        { return Err(Failure::WholeContainerLifecycleUnsupported.into()) }
        let selection=file_draft::resolve(state,target)?;
        let replica=record.replicas.get(&record.owner_disk).ok_or(Failure::ContainerScope)?;
        let owner=context.namespace_host_root(target.repository,record.namespace.branch)?;
        let disk=owner.disk();
        let root=owner.binding();
        let id=disk.disk_id.parse::<DiskId>()?;
        if id!=record.owner_disk || root.root_id!=replica.root_id || replica.entity.inode()==0
            || !root.source_chain.is_empty() || !replica.prefixes.is_empty()
            || root.registration.pool_id!=context.config.pool_id || root.registration.disk_id!=disk.disk_id
            || root.registration.slot_id!=disk.slot_id || root.registration.binding_generation!=disk.binding_generation
            || selection.history!=record.history_tree_id || selection.placement.primary_slot_id!=disk.slot_id
        { return Err(Failure::ContainerScope.into()) }
        let originals=context.original_receipts.ok_or(Failure::NamespaceSource)?;
        originals.require_context(&context)?;
        let source=containers::observe_replica(
            context.config,context.disk_resources,state,target.file,disk,
        )?;
        if !std::ptr::eq(owner.common()?,source.disk().content_root()?)
            || !std::ptr::eq(owner.deleted()?,source.disk().deleted_root()?)
        { return Err(Failure::RootIdentity.into()) }
        require_areas(source.disk(),root,replica.entity)?;
        if source.name()!=&record.namespace.name || source.name().as_str()!=target.path.file_name() {
            return Err(Failure::ContainerIdentity.into())
        }
        // Enumerate the complete admitted original version inventory, not a
        // directory listing or just the current Version. Stream bodies one at
        // a time with the normal held FILE verifier and its descriptor budget.
        let mut found=false;
        let file=target.file.to_string();
        let history=record.history_tree_id.to_string();
        let head=selection.base.version_id().to_string();
        if state.versions.len()>context.config.max_entries {return Err(Failure::ContainerBudget.into())}
        for version in state.versions.values() {
            if version.file_id!=file {continue}
            let placement=state.placements.get(&version.version_id).ok_or(Failure::PublicationLocation)?;
            if version.history_tree_id!=history
                || state.content_states.get(&version.version_id)!=Some(&ContentState::Available)
                || placement.target_copies!=1 || placement.route_slot_ids.as_slice()!=[disk.slot_id.as_str()]
                || placement.primary_slot_id!=disk.slot_id || placement.binding_generations.len()!=1
                || placement.binding_generations.get(&disk.slot_id)!=Some(&disk.binding_generation)
            {return Err(Failure::WholeContainerLifecycleUnsupported.into())}
            source.verify_version_body(version)?;
            found |= version.version_id==head;
        }
        if !found {return Err(Failure::ContainerScope.into())}
        let index=context.snapshot_receipts().map_err(|err|crate::Error::SnapshotSource(Box::new(err.into())))?;
        let initial=crate::retained_names::RetainedDiskRegistry::new(context.config.pool_id.parse()?);
        let registry=index.retained_registry().unwrap_or(&initial);
        let name=name_policy::Name::parse(target.path.file_name())?;
        let allocation=crate::retained_names::allocate_file(
            registry,owner.deleted()?,id,operation,&name,index.retained_names(id),context.config.max_entries,
        ).map_err(Failure::RetainedAllocation)?;
        let result=Self {
            state:Arc::downgrade(state), resources:Arc::downgrade(context.disk_resources), source,
            facts:ReviewedFileSite {target:target.clone(),base:selection.base,history:selection.history,
                file_birth:record.created_by,owner:id,replica:replica.clone(),root:root.clone(),
                allocation,operation,actor:actor.clone()},
        };
        result.require_unmoved(engine)?;
        Ok(result)
    }

    pub(crate) fn facts(&self)->&ReviewedFileSite { &self.facts }

    /// A later pool state, another Engine, or a replaced parent cannot borrow
    /// this work, even if all user-visible data still have the same spellings.
    pub(crate) fn require_unmoved(&self, engine:&Engine)->Result<()> {
        let context=engine.planning_context();
        if !Weak::ptr_eq(&self.state,&Arc::downgrade(context.state))
            || !Weak::ptr_eq(&self.resources,&Arc::downgrade(context.disk_resources))
        {return Err(Failure::DraftChanged.into())}
        let record=context.state.file_containers.get(&self.facts.target.file).ok_or(Failure::ContainerScope)?;
        let owner=context.namespace_host_root(self.facts.target.repository,record.namespace.branch)?;
        let disk=self.source.disk();
        if owner.binding()!=&self.facts.root || !std::ptr::eq(owner.common()?,disk.content_root()?)
            || !std::ptr::eq(owner.deleted()?,disk.deleted_root()?)
            || record.owner_disk!=self.facts.owner || record.created_by!=self.facts.file_birth
            || record.replicas.len()!=1 || record.replicas.get(&self.facts.owner)!=Some(&self.facts.replica)
        {return Err(Failure::RootIdentity.into())}
        require_areas(disk,&self.facts.root,self.facts.replica.entity)?;
        let held=self.source.container()?;
        let actual=held.directory_identity(held.path())?;
        if (actual.device,actual.inode)!=(self.facts.replica.entity.device(),self.facts.replica.entity.inode()) {
            return Err(Failure::ContainerIdentity.into())
        }
        require_leaf(owner.common()?,self.facts.target.path.file_name(),true,context.config.max_entries)?;
        require_leaf(owner.deleted()?,self.facts.allocation.name.as_str(),false,context.config.max_entries)

    }
}

/// The exact expected spelling is unique across name-policy equivalents. A
/// scan can reject occupancy; it can never identify an object to adopt.
fn require_leaf(parent:&DiskRoot, leaf:&str, existing:bool, limit:usize)->Result<()> {
    let mut found=false;
    let wanted=name_policy::lookup_key(leaf);
    for (index,item) in parent.entries(parent.path())?.enumerate() {
        if index>=limit {return Err(Failure::ContainerBudget.into())}
        let item=item?;
        let name=item.as_os_str().to_str().ok_or(Failure::ContainerIdentity)?;
        if name_policy::lookup_key(name)==wanted {
            if !existing {return Err(Failure::RetainedDestinationOccupied.into())}
            if name!=leaf || found {return Err(Failure::ContainerIdentity.into())}
            found=true;
        }
    }
    if found!=existing {return Err(Failure::ContainerIdentity.into())}
    Ok(())
}

fn require_areas(disk:&DiskHandle, root:&RootBinding, original:ContainerEntity)->Result<()> {
    let roles=&root.roots;
    let mut identities=std::collections::BTreeSet::new();
    for (actual, expected) in [
        (&disk.root,root.root_entity), (disk.content_root()?,roles.common_data.entity),
        (disk.deleted_root()?,roles.deleted_data.entity), (disk.management_root()?,roles.zxs_data.entity),
        (disk.current_management_root()?,roles.zxs_lifecycles.common_data.entity),
        (disk.retained_management_root()?,roles.zxs_lifecycles.deleted_data.entity),
    ] {
        let observed=actual.directory_identity(actual.path())?;
        let identity=LocalEntityIdentity::new(observed.device,observed.inode);
        if observed.inode==0 || observed.device!=original.device() || identity!=expected
            || observed.inode==original.inode() || !identities.insert((observed.device,observed.inode))
        {return Err(Failure::RootIdentity.into())}
    }
    Ok(())
}
