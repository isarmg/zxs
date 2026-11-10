//! One normal object change and one operation receipt for every selected member.
use super::*;
use crate::engine::Prepared;
use history_copy::{CopyRoute, graph::IdentityScope, retained::CopiedLifecycle};
use json_codec::{json, Value};
use storage_contract::{CommitState, CurrentHealth, Event, ProtectionAtPublish,
    file_relocation::{ObjectMoveFact, NamespaceEntryId, OBJECT_MOVE_KIND},
    history_window::{HistoryFileId, HistoryRepositoryId}};
#[path = "graph.rs"]
mod graph;
#[path = "directories.rs"]
mod directories;

#[derive(Serialize,Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ResultBody {
    pub movement: ObjectMoveFact,
    route: CopyRoute,
    namespace_basis: NamespaceBasis,
    pub version_mapping: BTreeMap<String,String>,
    pub retained_version_ids: Vec<String>,
    pub retained_lifecycle: Vec<CopiedLifecycle>,
    pub source_deletions: Vec<FileDeletionNode>,
    pub displaced_deletions: Vec<FileDeletionNode>,
    pub imported_deletions: Vec<FileDeletionNode>,
    pub imported_directory_deletions: Vec<storage_contract::directory_git::DirectoryDeletionNode>,
    branch_id: String,
    history_tree_id: String,
    commit_id: String,
    #[serialization(with="storage_contract::u64_string")]
    branch_generation: u64,
    #[serialization(with="storage_contract::u64_string")]
    timestamp: u64,
}

pub(crate) struct MovePlan {
    state: State,
    prepared: Vec<Prepared>,
    receipt: OperationResult,
    retained: Vec<String>,
}
impl MovePlan {
    pub(crate) fn into_parts(self) -> (State,Vec<Prepared>,OperationResult,Vec<String>) {
        (self.state,self.prepared,self.receipt,self.retained)
    }
}

fn invalid() -> Error {
    Error::new(ErrorCode::PublicationInvalid,"object move differs from its original members or complete write set")
}
fn increment(value:u64)->Result<u64> {
    value.checked_add(1).ok_or_else(|| Error::new(ErrorCode::ResourceLimit,"move generation exhausted"))
}
fn generated(operation:OperationId,prefix:&str,purpose:&str)->Result<String> {
    IdentityScope {operation,member:None}.generated(prefix,purpose)
}

/// Called only for a new operation, after the executor has looked up an original
/// receipt or journal candidate. This function does not publish any member.
pub(crate) fn plan(engine:&Engine,request:&ObjectMoveRequest,principal:&str,operation:OperationId)->Result<MovePlan> {
    Engine::require_actor(principal)?;
    engine.require_write()?;
    let source = engine.state.repositories.get(request.source_repository_id.as_str())
        .ok_or_else(|| Error::new(ErrorCode::RepositoryNotFound,"move source missing"))?;
    require_writable(engine,source)?;
    if let Some(id)=&request.target_repository_id {
        let target=engine.state.repositories.get(id.as_str())
            .ok_or_else(|| Error::new(ErrorCode::RepositoryNotFound,"move target missing"))?;
        require_writable(engine,target)?;
    }
    let disk=engine.disk(&request.target_slot_id)?;
    if let Some(view)=engine.admitted_current_namespaces()? { view.require_write_slot(&disk.slot_id)?; }
    let namespace_basis=basis(engine.state(),engine.current_namespaces.as_deref(),request)?;
    let unit=capture(engine.state(),request,&namespace_basis,engine.config.max_entries)?;
    let copies=match &unit.target { MoveTarget::Existing {repository,..}=>repository.target_copies,MoveTarget::EmptyDisk {..}=>1 };
    let slots=engine.planning_context().route(&request.target_slot_id,copies)?;
    let route=CopyRoute {created_at:now()?,bindings:slots.iter()
        .map(|slot| Ok((slot.clone(),engine.disk(slot)?.binding_generation))).collect::<Result<_>>()?,slots};
    let (state,receipt,result)=compose(engine.state(),unit,namespace_basis,&route,principal,operation,engine.next_slot)?;
    let prepared=result.version_mapping.iter().map(|(source,target)| Ok(Prepared {
        version:state.versions[target].clone(), placement:state.placements[target].clone(),
        source:engine.valid_backup_replica(source)?,
    })).collect::<Result<_>>()?;
    Ok(MovePlan {state,prepared,receipt,retained:result.retained_version_ids})
}

fn publish_namespace(next:&mut State,repository:&Repository,entries:Vec<Entry>,principal:&str,operation:OperationId,purpose:&str)->Result<String> {
    let branch=next.branches[&repository.main_branch_id].clone();
    let pending=entries.iter().filter(|entry| entry.kind==NamespaceEntryKind::File
        && next.versions[&entry.version_id].file_id!=entry.file_id)
        .map(|entry|(entry.file_id.clone(),entry.version_id.clone())).collect();
    let id=generated(operation,"commit",purpose)?;
    next.commits.insert(id.clone(),Commit {
        commit_id:id.clone(),history_tree_id:repository.history_tree_id.clone(),kind:"object_move".into(),
        parent_commit_ids:vec![branch.head_commit_id],tree_id:generated(operation,"tree",purpose)?,
        sequence:next.sequence,operation_id:operation,author:principal.into(),message:"Move object and complete retained history".into(),
        entries,namespace_deletions:None,pending_file_versions:pending,provenance:None,
    });
    let branch=next.branches.get_mut(&repository.main_branch_id).expect("invariant: captured repository main exists");
    branch.head_commit_id=id.clone();branch.generation=increment(branch.generation)?;
    Ok(id)
}

fn deletion(previous:&State,repository:&Repository,entry:&Entry,principal:&str,operation:OperationId,sequence:u64,time:u64)->Result<FileDeletionNode> {
    let mut heads=BTreeMap::new();
    for branch in previous.branches.values().filter(|branch| branch.file_id.as_deref()==Some(entry.file_id.as_str())) {
        let node=branch.file_node_id.clone().or_else(|| {
            previous.commits[&branch.head_commit_id].entries.iter().find(|member| member.file_id==entry.file_id).map(|member|member.version_id.clone())
        }).ok_or_else(invalid)?;
        heads.insert(branch.branch_id.clone(),node);
    }
    let parents=heads.values().cloned().collect::<BTreeSet<_>>().into_iter().collect();
    Ok(FileDeletionNode {
        node_id:generated(operation,"fdel",&entry.file_id)?,repository_id:repository.repository_id.clone(),
        history_tree_id:repository.history_tree_id.clone(),file_id:entry.file_id.clone(),path:entry.location().clone(),
        branch_heads:heads,parent_node_ids:parents,operation_id:operation,author:principal.into(),sequence,created_at:time,
    })
}

fn relocate_current_files(previous:&State,next:&mut State,unit:&MoveUnit,target:&Repository,graph:Option<&graph::Graph>,arrived:&[Entry],route:&CopyRoute,principal:&str,operation:OperationId)->Result<()> {
    let same_parent_rename=!unit.copy_history
        && unit.request.source_path.key().rsplit_once('/').map(|(parent,_)|parent)
            ==unit.request.target_path.key().rsplit_once('/').map(|(parent,_)|parent);
    let rename_directory=same_parent_rename && unit.entries.iter()
        .find(|member|member.source.lookup_key()==unit.request.source_path.key())
        .expect("invariant: captured move includes its source").source.kind==NamespaceEntryKind::Directory;
    let mut renamed_locations=BTreeSet::new();
    let mut files=BTreeSet::new();
    let aliased_files=unit.entries.iter().filter(|member|
        member.source.kind==NamespaceEntryKind::File && !files.insert(member.source.file_id.as_str()))
        .map(|member|member.source.file_id.as_str()).collect::<BTreeSet<_>>();
    // Clear old keys first, before installing any destination occurrence.
    for member in &unit.entries {
        if member.source.kind==NamespaceEntryKind::File {
            next.file_contexts.remove(&directory::context_key_from_key(&unit.source_repository.repository_id,member.source.lookup_key()));
        }
    }
    let target_head=next.commits[&next.branches[&target.main_branch_id].head_commit_id].clone();
    let entries_by_path=target_head.entries.iter().map(|entry|(entry.lookup_key(),entry)).collect::<BTreeMap<_,_>>();
    for (member,entry) in unit.entries.iter().zip(arrived) {
        if entry.kind!=NamespaceEntryKind::File { continue; }
        if same_parent_rename {
            // All aliases of this directory rename have the same physical
            // outcome. Apply the existing owner-prefix rule once per file.
            if renamed_locations.insert(entry.file_id.as_str()) {
                directory::set_namespace_location(next,&member.source,unit.request.source_path.as_str(),
                    unit.request.target_path.as_str(),storage_contract::DirectoryBatchKind::Rename,rename_directory);
            }
        } else {
            next.file_names.insert(entry.file_id.clone(),entry.name().into());
            next.file_locations.insert(entry.file_id.clone(),entry.path().into());
        }
        if previous.file_deletions.contains_key(&member.source.file_id) {continue;}
        let old=match graph {Some(graph)=>graph.entry(&member.source)?,None=>member.source.clone()};
        let original=member.context.as_ref().expect("invariant: captured file has context");
        let mut context=match graph {Some(graph)=>graph.context(original)?,None=>original.clone()};
        let mut ordinal=0u64;
        let mut ids=|prefix:&str| {
            ordinal+=1;
            // Preserve the original labels for ordinary single occurrences.
            let purpose=if aliased_files.contains(member.source.file_id.as_str()) {
                format!("current:{}:{ordinal}",member.source.lookup_key())
            } else {format!("current-{ordinal}")};
            IdentityScope {operation,member:Some(&entry.file_id)}.generated(prefix,&purpose)
        };
        if !next.branches.get(&context.branch_id).is_some_and(|branch|branch.file_id.as_deref()==Some(entry.file_id.as_str())) {
            context.branch_id=file::ensure_shared_file_branch_with_ids(next,&target_head,entry,&mut ids)?;
        }
        if old.path()!=entry.path() {
            // Borrowed contents have no file-owned node before their first
            // save. Move the pending occurrence without forging that node.
            if next.versions[&entry.version_id].file_id==entry.file_id {
                context=file_paths::rename_node_with_ids(next,&target.repository_id,&old,entry,Some(context),
                    principal,operation,route.created_at,&mut ids)?;
            }
            file::relocate_namespace_file_branch_with_ids(next,&context,&entries_by_path,entry,operation,&mut ids)?;
        }
        next.file_contexts.insert(directory::context_key_from_key(&target.repository_id,entry.lookup_key()),context);
        if !same_parent_rename {
            next.file_names.insert(entry.file_id.clone(),entry.name().into());
            next.file_locations.insert(entry.file_id.clone(),entry.path().into());
        }
    }
    Ok(())
}

fn refresh_working(next:&mut State,unit:&MoveUnit,target:&Repository,operation:OperationId)->Result<()> {
    let mut counter=0u64;
    let mut ids=|prefix:&str| {
        counter+=1;
        generated(operation,prefix,&format!("working-{counter}"))
    };
    // Refresh working compositions only. Their index, HEAD, and staged child
    // references stay frozen until the user explicitly stages/commits them.
    let mut target_paths=unit.entries.iter().map(|entry| {
        if entry.source.kind==NamespaceEntryKind::Directory {format!("{}/",entry.destination.as_str())}
        else {entry.destination.as_str().into()}
    }).collect::<Vec<_>>();
    if target.repository_id==unit.source_repository.repository_id {
        target_paths.extend(unit.entries.iter().map(|entry|entry.source.path().to_owned()));
        target_paths.sort();target_paths.dedup();
    }
    directory::record_directory_edits_with_ids(next,&target.repository_id,&target_paths,None,&mut ids)?;
    if target.repository_id!=unit.source_repository.repository_id {
        let source_paths=unit.entries.iter().map(|entry|entry.source.path().to_owned()).collect::<Vec<_>>();
        directory::record_directory_edits_with_ids(next,&unit.source_repository.repository_id,&source_paths,None,&mut ids)?;
    }
    Ok(())
}

fn compose(previous:&State,unit:MoveUnit,namespace_basis:NamespaceBasis,route:&CopyRoute,principal:&str,operation:OperationId,sequence:u64)->Result<(State,OperationResult,ResultBody)> {
    let mut next=previous.clone();next.begin_tracking();
    crate::container_storage::capture_names(&mut next);
    next.sequence=sequence;next.business_generation=increment(previous.business_generation)?;
    let target=unit.project_target_root(&mut next,principal,operation)?;
    let mut imported_directory_deletions=Vec::new();
    let graph=if unit.copy_history {
        let graph=graph::copy(previous,&mut next,&unit,&target,route,principal,operation)?;
        imported_directory_deletions=directories::copy(previous,&mut next,&unit,&target,&graph,principal,operation,route.created_at)?;
        Some(graph)
    } else {None};
    let arrived=unit.entries.iter().map(|member| {
        let mut entry=match graph.as_ref() {Some(graph)=>graph.entry(&member.source)?,None=>member.source.clone()};
        entry.relocate(member.destination.clone());Ok(entry)
    }).collect::<Result<Vec<_>>>()?;
    // Stable identities may occur at several paths. Remove only the captured
    // occurrences, keeping unrelated aliases and their original contexts.
    let source_occurrences=unit.entries.iter().map(|entry|
        (entry.source.entry_id.as_str(),entry.source.lookup_key())).collect::<BTreeSet<_>>();
    let displaced=match &unit.target {MoveTarget::Existing {displaced_entries,..}=>displaced_entries.as_slice(),MoveTarget::EmptyDisk {..}=>&[]};
    let displaced_occurrences=displaced.iter().map(|entry|
        (entry.entry_id.as_str(),entry.lookup_key())).collect::<BTreeSet<_>>();
    let target_original=match &unit.target {MoveTarget::Existing {namespace,..}=>namespace.as_slice(),MoveTarget::EmptyDisk {..}=>&[]};
    let mut target_entries=target_original.iter().filter(|entry| {
        !displaced_occurrences.contains(&(entry.entry_id.as_str(),entry.lookup_key()))
            && (target.repository_id!=unit.source_repository.repository_id
                || !source_occurrences.contains(&(entry.entry_id.as_str(),entry.lookup_key())))
    }).cloned().collect::<Vec<_>>();
    target_entries.extend(arrived.iter().cloned());
    namespace::validate_entries(&mut target_entries,usize::MAX)?;
    let commit=publish_namespace(&mut next,&target,target_entries,principal,operation,"target")?;
    let mut removed=displaced.iter().map(|entry|storage_contract::DeletedPath {path:entry.path().into(),kind:entry.kind}).collect::<Vec<_>>();
    if target.repository_id==unit.source_repository.repository_id {
        removed.extend(unit.entries.iter().map(|entry|storage_contract::DeletedPath {path:entry.source.path().into(),kind:entry.source.kind}));
    }
    next.commits.get_mut(&commit).expect("invariant: new target commit exists").namespace_deletions=namespace::deletion_record(removed,route.created_at);
    if target.repository_id!=unit.source_repository.repository_id {
        let source_entries=unit.source_namespace.iter().filter(|entry|
            !source_occurrences.contains(&(entry.entry_id.as_str(),entry.lookup_key()))).cloned().collect();
        let source_commit=publish_namespace(&mut next,&unit.source_repository,source_entries,principal,operation,"source")?;
        let removed=unit.entries.iter().map(|entry|storage_contract::DeletedPath {path:entry.source.path().into(),kind:entry.source.kind}).collect();
        next.commits.get_mut(&source_commit).expect("invariant: new source commit exists").namespace_deletions=namespace::deletion_record(removed,route.created_at);
    }
    relocate_current_files(previous,&mut next,&unit,&target,graph.as_ref(),&arrived,route,principal,operation)?;
    let mut imported_deletions=Vec::new();
    if let Some(graph)=&graph {
        for original in previous.file_deletions.values().filter(|node|unit.files.contains_key(&node.file_id)) {
            let mut node=original.clone();
            node.node_id=graph.id(&original.node_id)?;node.file_id=graph.id(&original.file_id)?;
            node.repository_id=target.repository_id.clone();node.history_tree_id=target.history_tree_id.clone();
            node.branch_heads=original.branch_heads.iter().map(|(branch,head)|Ok((graph.id(branch)?,graph.id(head)?))).collect::<Result<_>>()?;
            node.parent_node_ids=original.parent_node_ids.iter().map(|id|graph.id(id)).collect::<Result<_>>()?;
            node.parent_node_ids.sort();
            node.operation_id=operation;node.author=principal.into();node.sequence=sequence;node.created_at=route.created_at;
            next.file_deletions.insert(node.file_id.clone(),node.clone());imported_deletions.push(node);
        }
    }
    let mut source_deletions=Vec::new();
    let mut displaced_deletions=Vec::new();
    let mut seen=BTreeSet::new();
    if unit.copy_history {
        let source_files=unit.entries.iter().filter(|member|member.source.kind==NamespaceEntryKind::File)
            .map(|member|member.source.file_id.as_str()).collect::<BTreeSet<_>>();
        let committed=directory_git::committed_files(previous,&source_files);
        for member in unit.entries.iter().filter(|member|member.source.kind==NamespaceEntryKind::File) {
            if !unit.files[&member.source.file_id].version_ids.is_empty()
                && !previous.file_deletions.contains_key(&member.source.file_id)
                && !committed.contains(&member.source.file_id)
                && seen.insert(member.source.file_id.as_str()) && !unit.source_namespace.iter().any(|entry|
                entry.kind==NamespaceEntryKind::File && entry.file_id==member.source.file_id
                    && !source_occurrences.contains(&(entry.entry_id.as_str(),entry.lookup_key()))) {
                let node=deletion(previous,&unit.source_repository,&member.source,principal,operation,sequence,route.created_at)?;
                next.file_deletions.insert(node.file_id.clone(),node.clone());source_deletions.push(node);
            }
        }
    }
    seen.clear();
    for entry in displaced.iter().filter(|entry|entry.kind==NamespaceEntryKind::File) {
        if previous.versions.values().any(|version|version.file_id==entry.file_id)
            && seen.insert(entry.file_id.as_str()) && !next.commits[&commit].entries.iter().any(|current|
            current.kind==NamespaceEntryKind::File && current.file_id==entry.file_id) {
            let node=deletion(previous,&target,entry,principal,operation,sequence,route.created_at)?;
            next.file_deletions.insert(node.file_id.clone(),node.clone());displaced_deletions.push(node);
        }
    }
    refresh_working(&mut next,&unit,&target,operation)?;
    crate::container_storage::capture_changed_names(previous,&mut next);
    let root=unit.entries.iter().find(|entry|entry.source.lookup_key()==unit.request.source_path.key()).expect("invariant: capture includes root");
    let arrived_root=arrived.iter().find(|entry|entry.lookup_key()==unit.request.target_path.key()).expect("invariant: projection includes target root");
    let displaced_root=match &unit.target {MoveTarget::Existing {displaced,..}=>displaced.as_ref(),MoveTarget::EmptyDisk {..}=>None};
    let movement=ObjectMoveFact {
        request:unit.request.clone(),principal:principal.into(),target_repository_id:HistoryRepositoryId::parse(&target.repository_id)?,
        target_entry_id:NamespaceEntryId::parse(&arrived_root.entry_id)?,
        source_file_id:(root.source.kind==NamespaceEntryKind::File).then(||HistoryFileId::parse(&root.source.file_id)).transpose()?,
        displaced_target_file_id:displaced_root.filter(|entry|entry.kind==NamespaceEntryKind::File).map(|entry|HistoryFileId::parse(&entry.file_id)).transpose()?,
    };
    let mut retained=unit.retained_version_ids().into_iter().map(str::to_owned).collect::<BTreeSet<_>>();
    let displaced_files=displaced.iter().map(|entry|entry.file_id.as_str()).collect::<BTreeSet<_>>();
    retained.extend(previous.versions.values().filter(|version|displaced_files.contains(version.file_id.as_str())).map(|version|version.version_id.clone()));
    retained.extend(previous.branches.values().filter(|branch| branch.file_id.as_ref()
        .is_some_and(|file|displaced_files.contains(file.as_str())))
        .flat_map(|branch|branch.pending_file_versions.values()).cloned());
    let mut dependencies=retained.clone();
    while let Some(id)=dependencies.pop_first() {
        for source in &previous.versions[&id].source_version_ids {
            if retained.insert(source.clone()) {dependencies.insert(source.clone());}
        }
    }
    let (version_mapping,retained_lifecycle)=match graph {Some(graph)=>(graph.versions,graph.lifecycle),None=>(BTreeMap::new(),Vec::new())};
    let result=ResultBody {
        movement,route:route.clone(),namespace_basis,version_mapping,retained_version_ids:retained.into_iter().collect(),
        retained_lifecycle,source_deletions,displaced_deletions,imported_deletions,imported_directory_deletions,
        branch_id:target.main_branch_id.clone(),history_tree_id:target.history_tree_id.clone(),commit_id:commit.clone(),
        branch_generation:next.branches[&target.main_branch_id].generation,timestamp:route.created_at,
    };
    history_copy::retained::bounded(&result)?;
    let receipt=OperationResult {
        operation_id:operation,kind:OBJECT_MOVE_KIND.into(),commit_id:Some(commit.clone()),
        history_tree_id:Some(target.history_tree_id),branch_id:Some(target.main_branch_id),
        commit_state:CommitState::Published,protection_at_publish:if result.version_mapping.is_empty() {ProtectionAtPublish::NotApplicable}else{ProtectionAtPublish::TargetMet},
        current_health:CurrentHealth::Healthy,request_digest:unit.request.digest(principal)?,sequence,result:json!(result),
    };
    next.operations.insert(operation,receipt.clone());next.operation_principals.insert(operation,principal.into());
    next.events.push(Event {sequence,operation_id:operation,kind:OBJECT_MOVE_KIND.into(),object_id:commit});
    Ok((next,receipt,result))
}

/// Decode the original operation facts once for database projection and replay.
/// Full candidate admission still calls verify_transition; this reader does not
/// authorize a fresh operation from today's namespace.
pub(crate) fn read_result(receipt:&OperationResult)->Result<ResultBody> {
    if receipt.kind!=OBJECT_MOVE_KIND {return Err(invalid());}
    history_copy::retained::bounded(&receipt.result)?;
    let result:ResultBody=json_codec::from_value(receipt.result.clone())?;
    if receipt.request_digest!=result.movement.request.digest(&result.movement.principal)?
        || receipt.commit_id.as_deref()!=Some(result.commit_id.as_str())
        || receipt.branch_id.as_deref()!=Some(result.branch_id.as_str())
        || receipt.history_tree_id.as_deref()!=Some(result.history_tree_id.as_str()) {
        return Err(invalid());
    }
    let mut files=BTreeSet::new();
    for lifecycle in &result.retained_lifecycle {
        if !files.insert(lifecycle.file_id.as_str()) {return Err(invalid());}
        history_copy::retained::validate_payload(receipt,&lifecycle.file_id,lifecycle)?;
    }
    let mut nodes=BTreeSet::new();
    for node in result.source_deletions.iter().chain(&result.displaced_deletions).chain(&result.imported_deletions) {
        if !nodes.insert(node.node_id.as_str()) || node.operation_id!=receipt.operation_id
            || node.sequence!=receipt.sequence || node.author!=result.movement.principal
            || node.created_at!=result.timestamp {return Err(invalid());}
    }
    for node in &result.imported_directory_deletions {
        if !nodes.insert(node.node_id.as_str()) || node.operation_id!=receipt.operation_id
            || node.sequence!=receipt.sequence || node.author!=result.movement.principal
            || node.created_at!=result.timestamp {return Err(invalid());}
    }
    Ok(result)
}

pub(crate) fn verify_transition(previous:&State,next:&State,view:Option<&crate::engine::CurrentNamespaces>,receipt:&OperationResult)->Result<()> {
    if receipt.kind!=OBJECT_MOVE_KIND || receipt.sequence!=next.sequence {return Err(invalid());}
    let original=read_result(receipt)?;
    if receipt.request_digest!=original.movement.request.digest(&original.movement.principal)?
        || original.namespace_basis!=basis(previous,view,&original.movement.request)? {return Err(invalid());}
    let unit=capture(previous,&original.movement.request,&original.namespace_basis,usize::MAX)?;
    let (expected,expected_receipt,_)=compose(previous,unit,original.namespace_basis,&original.route,
        &original.movement.principal,receipt.operation_id,receipt.sequence)?;
    if &expected!=next || expected_receipt!=*receipt {return Err(invalid());}
    Ok(())
}
