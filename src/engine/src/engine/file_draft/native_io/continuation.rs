//! Acknowledging a Born automatic range save gives *this descriptor* one
//! narrow chance to continue after that exact save becomes a formal FILE node.
//! No caller-supplied revision, body hash, name or new open can mint this loan.
use super::{Engine, NativeReader, OperationResult, Principal, Result};
use crate::business::file_draft::{self as business, automatic::{Saved, stale}};
use crate::persist::file_draft as codec;
use std::sync::{Arc, Weak};
use storage_contract::file_container::{ContainerNamespace, ContainerReplica};
use storage_contract::file_draft::{AutomaticBody, DraftBase, DraftLocation, DraftPayload, FileWorkingState};
use storage_contract::{DiskId, EntryId, FileNodeId, HistoryTreeId, OperationId, Placement};

pub(super) struct Acknowledged {
    environment: Weak<crate::disk::DiskResources>,
    save: Saved,
    base: DraftBase,
    payload: DraftPayload,
    placement: Placement,
    history: HistoryTreeId,
    owner: DiskId,
    registration: OperationId,
    namespace: ContainerNamespace,
    replica: ContainerReplica,
    parents: Vec<(EntryId, name_policy::PortablePath)>,
    head: String,
    branch_generation: u64,
    decisions: usize,
}

impl Acknowledged {
    /// Reached only after the actual save completed and its new reader opened.
    /// Unborn, Worktree, FULL, Manual and protected saves cannot issue this
    /// permission; they continue to use the existing strict handle semantics.
    pub(super) fn capture(
        engine:&Engine, target:&DraftLocation, reader:&NativeReader, actor:&Principal,
        handle_operation:Option<OperationId>, acknowledged:&OperationResult,
    ) -> Result<Option<Self>> {
        let NativeReader::Manual(held)=reader else {return Ok(None)};
        let context=engine.planning_context();
        let state=context.state;
        let file=target.file;
        let record=state.file_containers.get(&file).ok_or_else(stale)?;
        let FileWorkingState::Automatic {controller, body:AutomaticBody::Incremental {..},..}=&record.working else {return Ok(None)};
        let Some(pending)=controller.pending_save() else {return Ok(None)};
        if acknowledged.kind!="file.draft.write" || acknowledged.sequence!=state.sequence
            || handle_operation!=Some(acknowledged.operation_id) || pending.operation!=acknowledged.operation_id {
            return Ok(None);
        }
        let (original,owner)=context.original_receipts.ok_or_else(stale)?.file_decision(&context,pending.operation)?;
        if original!=acknowledged || owner!=actor {return Err(stale())}
        let save=Saved::capture(state,target,original,actor)?;
        held.require_record(file,record)?;
        super::super::require_initial(&context,target)?;
        let location=business::ordinary::OrdinaryOccurrence::capture(state,file,record)?;
        location.require_target(target)?;
        // Continuation is optional: a valid dirty save can still refer to
        // a no-longer-selected base. Acknowledge the actual save normally
        // without issuing permission to rebind such a descriptor.
        if state.file_deletions.contains_key(&file.to_string()) {return Ok(None)}
        let before=match business::resolve(state,target) {
            Ok(selected)=>selected,
            Err(crate::Error::FileDraft(crate::DraftError::Selection | crate::DraftError::UnsupportedScope))=>return Ok(None),
            Err(error)=>return Err(error),
        };
        let (base,payload)=record.working.draft().ok_or_else(stale)?;
        if before.base!=*base || held.base()!=*base || crate::node_retirement_engine::is_retired(state,&base.version_id().to_string()) {
            return Ok(None);
        }
        let branch=state.branches.get(&base.branch_id().to_string()).ok_or_else(stale)?;
        let replica=record.replicas.get(&record.owner_disk).ok_or_else(stale)?;
        Ok(Some(Self {
            environment:Arc::downgrade(context.disk_resources), save, base:*base, payload:*payload,
            placement:before.placement, history:record.history_tree_id, owner:record.owner_disk,
            registration:record.created_by, namespace:record.namespace.clone(), replica:replica.clone(),
            parents:location.parents, head:branch.head_commit_id.clone(), branch_generation:branch.generation,
            decisions:state.operations.len(),
        }))
    }

    pub(super) fn continued(
        &self, engine:&Engine, target:&DraftLocation, actor:&Principal, acknowledged:&OperationResult,
    ) -> Result<Option<NativeReader>> {
        let context=engine.planning_context();
        let state=context.state;
        let record=state.file_containers.get(&target.file).ok_or_else(stale)?;
        let FileWorkingState::Automatic {revision,controller,body:AutomaticBody::Clean {}}=&record.working
            else {return Ok(None)};
        let Some(node)=controller.last_node() else {return Ok(None)};
        if node.save!=self.save.decision || node.epoch!=self.save.epoch {return Ok(None)}
        // No "latest receipt" or sequence arithmetic is a source. This first
        // live-handle permission also requires exactly one intervening durable
        // mutation, the independently recovered automatic original below.
        if !Weak::ptr_eq(&self.environment,&Arc::downgrade(context.disk_resources))
            || self.decisions.checked_add(1)!=Some(state.operations.len())
            || node.sequence.get()!=state.sequence || node.operation!=self.save.operation(target)?
            || controller.epoch()!=node.epoch || controller.latest_save()!=Some(&node.save)
            || controller.pending_save().is_some() || controller.protection().is_some()
            || self.save.decision.revision.next().map_err(|_|stale())?!=*revision
            || record.history_tree_id!=self.history || record.owner_disk!=self.owner
            || record.created_by!=self.registration || record.namespace!=self.namespace
            || record.replicas.len()!=1 || record.replicas.get(&self.owner)!=Some(&self.replica)
        {return Err(stale())}
        super::super::require_initial(&context,target)?;
        let occurrence=business::ordinary::OrdinaryOccurrence::capture(state,target.file,record)?;
        occurrence.require_target(target)?;
        if occurrence.parents!=self.parents {return Err(stale())}
        let originals=context.original_receipts.ok_or_else(stale)?;
        let (saved,save_actor)=originals.file_decision(&context,self.save.decision.operation)?;
        let (published,publisher)=originals.file_decision(&context,node.operation)?;
        if saved!=acknowledged || save_actor!=actor || publisher!=actor || published.kind!="file.draft.commit"
            || published.sequence!=node.sequence.get() {return Err(stale())}
        let decoded=codec::decode(state,published)?;
        // The installed original crossed the transition barrier against its
        // actual *prior* state. Do not replay that transition against its
        // now-clean result or reconstruct a historical draft from today's head.
        let proved=codec::observed_node(published)?.ok_or_else(stale)?;
        let out=decoded.outcome();
        let codec::Request::Automatic(request)=&out.request else {return Err(stale())};
        let content=out.content.as_ref().ok_or_else(stale)?;
        let origin=out.origin.as_ref().ok_or_else(stale)?;
        let id=&content.version.version_id;
        let body=FileNodeId::Version(id.parse()?);
        if origin!=&proved || codec::digest(&out.request,actor.as_str())?!=published.request_digest
            || origin.ancestors.iter().copied().ne(self.parents.iter().map(|(id,_)|*id))
            || state.file_deletions.contains_key(&target.file.to_string())
            || crate::node_retirement_engine::is_retired(state,id)
            || crate::node_retirement_engine::is_retired(state,&self.base.version_id().to_string())
            || request.target!=*target || request.saved!=self.save || request.base!=self.base
            || request.payload!=self.payload || out.working!=record.working
            || origin.reason!=self.save.reason() || origin.target!=*target || origin.owner!=self.owner
            || origin.node!=body || node.node!=body || origin.operation!=node.operation
            || origin.sequence!=node.sequence || origin.published_at!=node.published_at
            || decoded.timestamp()!=node.published_at || content.version.file_id!=target.file.to_string()
            || content.version.history_tree_id!=self.history.to_string()
            || state.versions.get(id)!=Some(&content.version) || state.placements.get(id)!=Some(&content.placement)
            || state.placements.get(&self.base.version_id().to_string())!=Some(&self.placement)
            || content.version.source_version_ids!=[self.base.version_id().to_string()]
            || state.file_node_parents.get(id).is_none_or(|p|p.as_slice()!=[self.base.node_id().to_string()])
        {return Err(stale())}
        let after=business::resolve(state,target)?;
        let branch=state.branches.get(&self.base.branch_id().to_string()).ok_or_else(stale)?;
        let commit=state.commits.get(&branch.head_commit_id).ok_or_else(stale)?;
        if after.history!=self.history || after.base.branch_id()!=self.base.branch_id()
            || after.base.node_id()!=body || after.base.version_id().to_string()!=*id
            || self.base.selection_generation().get().checked_add(1)!=Some(after.base.selection_generation().get())
            || self.branch_generation.checked_add(1)!=Some(branch.generation)
            || commit.operation_id!=node.operation || commit.author!=actor.as_str()
            || commit.parent_commit_ids.as_slice()!=[self.head.as_str()] || after.placement!=content.placement
        {return Err(stale())}
        // The original held inode is still owned by the caller. Observe the
        // independently verified new FULL through the exact same registered
        // root, prefixes, device/inode and namespace, not by hash or pathname.
        let reader=engine.open_manual_working(target)?;
        if reader.base()!=after.base || reader.manifest().is_some() {return Err(stale())}
        reader.require_record(target.file,record)?;
        Ok(Some(NativeReader::Manual(reader)))
    }
}
