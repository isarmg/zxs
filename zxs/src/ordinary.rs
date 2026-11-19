//! First ordinary vertical path: a real, initially empty manual FILE in a
//! selected registered ordinary root or installed directory. Creation does not publish a version.
mod custody;
pub(crate) use custody::{LifecycleSource,IncomingSelection,KnownStage,LifecycleTarget,LifecycleScan,LifecycleChecked,LifecyclePrevious,LifecycleOriginals,LifecycleRetiring};
pub(crate) use custody::admission::{LifecycleGrant,Side};
pub use custody::{apply_recycle,recycle_receipt,list_retained,DeletionKey,DeletionTime,RestoreTarget,RecycleAction,RecycleKind,RecycleReceipt,RecycleCursor,RecycleFilter,RecyclePage,RetainedEntry,move_file,move_file_at,move_receipt,move_receipt_at,MoveReceipt,MoveStatus};
mod wire;
mod storage_name;
pub(crate) mod portable_birth;
mod file_parent;
mod content;
mod indexed;
mod flat;
pub(crate) use flat::work as working_sql;
mod output;
mod interval;
pub use interval::{save_interval,interval_receipt,read_current_range_to,SpanObservation};
pub use interval::types::{SpanEdit,LengthPolicy,SpanReceipt};
mod formal;
mod delta;
pub use delta::{commit_working,formal_receipt};
pub use formal::{PublishedVersion,commit_first,first_commit_receipt,read_node_to,read_node_range_to,FormalObservation};
pub use formal::types::{FormalNode,FormalStorage};
mod draft;
pub use draft::{save_first,first_save_receipt,read_current_to,current_info,CurrentFile,FirstSaveObservation};
pub use draft::types::{DraftClaim,DraftReceipt,DRAFT_CHUNK};
mod directory;
pub use directory::{DirectoryParent,DirectoryEntry,DirectoryInfo,DirectoryListing,make_directory,list_directory};
mod material;
mod target_domain;
use target_domain::ordinary_target_domain;
pub(crate) use directory::path::{HeldParent as OrdinaryParentLease,NameSample as OrdinaryNameSample};
mod placement;
mod allocation;
pub(crate) mod journal;

use crate::{fs::{self,invalid},pool::{self,lease::{Access,LocalPool}},EntryId,FileId,HistoryId,OperationId,VolumeId};
use std::{collections::{BTreeMap,BTreeSet},io,path::Path};
use wire::Frame;

#[derive(Clone,Debug,Eq,PartialEq)]
pub struct FileInfo{
    pub entry:EntryId,pub file:FileId,pub history:HistoryId,pub volume:VolumeId,pub parent:DirectoryParent,
    pub name:String,pub created_by:OperationId,pub decision_sequence:Option<u64>,
}
impl FileInfo{
    fn from_frame(f:&Frame)->Self{Self::at(f,f.create.parent.address(f.create.volume))}
    fn at(f:&Frame,parent:DirectoryParent)->Self{
        let c=&f.create;
        Self{entry:c.entry,file:c.file,history:c.history,volume:parent.volume(),parent,name:c.name.clone(),
            created_by:c.op,decision_sequence:Some(f.seq)}
    }
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum CreationStatus{Reserved,Prepared,DecisionAwaitingConfirmation,PublishedInstallationUnconfirmed,Published}
#[derive(Clone,Debug,Eq,PartialEq)]
pub struct CreationReceipt{pub status:CreationStatus,pub file:FileInfo}
/// Complete current business entries. Separately reported issued private
/// slots have not been certified as FILEs or as safe to remove.
#[derive(Clone,Debug,Eq,PartialEq)]
pub struct RootFiles{pub files:Vec<FileInfo>,pub reserved_private_entries:usize,pub pending_moves:Vec<MoveReceipt>,pub pending_lifecycle:Vec<RecycleReceipt>}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Cut { Reservation,UnboundMkdir,DirectoryBoundBeforeSync,DirectoryRenamed,MoveLeafLinked,MoveInstalled,MoveTargetFlagBeforeSync,MoveTargetChecked,MoveRetireFlagBeforeSync,MoveSourceAdmitted,MoveSourceIsolated,MoveEraseFlagBeforeSync,MoveLeafRetired,MoveEmptyFlagBeforeSync,MoveEmptyConfirmed,MoveDirectoryUnlinked,FormalIndex,FormalRaw,FormalReadback,IncrLinked,WorkUnlinked,WorkReleased,BeforeIntent,Intent,FrameSynced,MarkedBeforeSync,Decision,Installation,IntentReleased,Release }

fn same_request(frame:&Frame,digest:[u8;32])->io::Result<()>{
    if frame.request!=digest{return Err(invalid("operation ID already belongs to another original request"));}
    Ok(())
}

/// The result is the original publication, not an online health check. An
/// unresolved original intent can only be continued by its exact original request.
pub fn create_empty_at(system:&Path,op:OperationId,parent:DirectoryParent,name:&str)->io::Result<FileInfo>{
    create_at_inner(system,op,parent,name,|_|Ok(()))
}
pub fn create_empty(system:&Path,op:OperationId,volume:VolumeId,name:&str)->io::Result<FileInfo>{
    create_empty_at(system,op,DirectoryParent::Root(volume),name)
}
#[cfg(test)]
fn create_inner(system:&Path,op:OperationId,id:VolumeId,name:&str,cut:impl FnMut(Cut)->io::Result<()>)->io::Result<FileInfo>{
    create_at_inner(system,op,DirectoryParent::Root(id),name,cut)
}
fn create_at_inner(system:&Path,op:OperationId,address:DirectoryParent,name:&str,mut cut:impl FnMut(Cut)->io::Result<()>)->io::Result<FileInfo>{
    let id=address.volume();
    wire::human_name(name.as_bytes())?;
    let (pool,_,state)=LocalPool::with_snapshot(system,Access::Write)?;
    pool.require_business_op(op)?;
    let request=file_parent::request(pool.id(),op,fs::local_uid(),address,name)?;
    if let Some(d)=state.decided(op){
        let done=d.creation()?;same_request(done,request)?;
        if !state.allocated.as_ref().is_some_and(|a|a.op==op){
            return Ok(FileInfo::from_frame(done)); // Do not open a later same-named root.
        }
    }
    state.require_file_token(op)?;
    if state.moving.is_some()||state.incrementing.is_some()||state.editing.is_some()||state.saving.is_some()||state.committing.is_some(){return Err(invalid("a different original FILE command needs completion"));}
    if let Some(a)=&state.allocated{
        if a.op!=op || a.request!=request {return Err(invalid("another original FILE allocation needs completion"));}
    }else{
        if state.ticket_used(op){return Err(invalid("new FILE operation reuses an original private ticket"));}
        if state.occupied(address,name){return Err(io::Error::new(io::ErrorKind::AlreadyExists,"FILE name is already registered"));}
        state.room_for(wire::MAX_FRAME)?;
    }
    let volume=pool.ordinary(id)?;
    if state.allocated.is_none(){match address{
      DirectoryParent::Child(_,parent)=>{directory::file_start::Unissued::select(&volume,parent,op,name)?.issue()?;}
      DirectoryParent::Root(_)=>{
        let file=FileId::new()?;let entry=EntryId::new()?;let history=HistoryId::new()?;
        if state.file(file).is_some() || state.entry_used(entry) || state.on_all().any(|f|f.create.history==history){
            return Err(invalid("new FILE identity collides with an original decision"));
        }
        state.require_new_file_name(address,name,entry,file)?;
        let (seq,previous)=state.frontier();
        let (attempt,root)=material::select_ticket(&volume,name,file)?;
        pool.require_business_op(attempt)?;
        if state.ticket_used(attempt)||op==attempt{return Err(invalid("new private ticket collides with earlier operation"));}
        allocation::Allocation::reserve(&pool.system,pool.id(),op,id,name,seq+1,previous,entry,file,history,attempt,root)?;
    }}}
    drop(state);
    cut(Cut::Reservation)?;
    let parent=directory::path::HeldParent::open(&volume,address)?;
    let (_,mut held)=pool.snapshot()?;
    let mut a=held.allocated.take().ok_or_else(||invalid("original FILE allocation disappeared"))?;
    if a.op!=op||a.request!=request||a.parent.address(a.volume)!=address{return Err(invalid("unexpected FILE allocation or parent"));}
    pool.system.confirm_record_sync(allocation::NAME,&a.stamp,a.bytes.len())?;
    pool.system.file.sync_all()?;
    if a.stage.is_none(){
        a.bind_fresh(&parent,||cut(Cut::UnboundMkdir))?;
        let original=a.digest;
        a=pool.snapshot()?.1.allocated.ok_or_else(||invalid("FILE allocation lost during stage binding"))?;
        if a.digest!=original||a.stage.is_none(){return Err(invalid("stage did not bind to the original reservation"));}
    }
    // A prior completion may have removed only its sealed material intent.
    if let Some(done)=held.operation(op){
        if held.pending.is_none(){
            material::install_decided(&parent,&done.create)?;
            pool.system.remove_original_record(allocation::NAME,&a.stamp,&a.bytes)?;
            return Ok(FileInfo::from_frame(done));
        }
    }
    if held.pending.is_none(){
        let creation=material::prepare(&parent,&a)?;
        let f=Frame::new(pool.id(),a.seq,a.previous,creation)?;
        if journal::Decision::read(pool.id(),&f.bytes)?.creation()?!=&f||!a.matches(&f){return Err(invalid("could not seal complete FILE decision"));}
        held.room_for(f.bytes.len())?;
        cut(Cut::BeforeIntent)?;
        journal::Intent::seal(&pool.system,&f.bytes)?;
    }
    drop(held);
    cut(Cut::Intent)?;
    let at_floor=|s:&journal::Journal,d:&journal::Decision|->io::Result<()>{
        let f=d.creation()?;let active=s.allocated.as_ref().ok_or_else(||invalid("lost original CREATE reservation at confirmation"))?;
        if active.stamp!=a.stamp||active.bytes!=a.bytes||!a.matches(f)||!active.matches(f){
            return Err(invalid("CREATE reservation, bound private slot or directory changed before confirmation"));}
        // Check the original private directory, its only FILE record, and the
        // actual derived name at the held ordinary parent before making CREATE visible.
        material::require_staged(&parent,&f.create)
    };
    let p=publish_sealed_checked(&pool,op,request,|decision|{
        material::require_staged(&parent,&decision.creation()?.create)
    },Some(&at_floor),|step|cut(step))?;
    let f=p.frame.creation()?;
    cut(Cut::Decision)?;
    material::install_decided(&parent,&f.create)?;
    cut(Cut::Installation)?;
    pool.system.remove_original_record(journal::INTENT,&p.stamp,&p.bytes)?;
    cut(Cut::IntentReleased)?;
    pool.system.remove_original_record(allocation::NAME,&a.stamp,&a.bytes)?;
    cut(Cut::Release)?;
    Ok(FileInfo::from_frame(f))
}

/// All FILE commands cross the same original decision and acknowledged
/// frontier here. The command owner must prove its exact durable physical
/// operands again immediately before appending any previously undecided bytes.
fn publish_sealed(pool:&LocalPool,op:OperationId,request:[u8;32],
    material:impl FnOnce(&journal::Decision)->io::Result<()>,cut:impl FnMut(Cut)->io::Result<()>)->io::Result<journal::Intent>{
    publish_sealed_checked(pool,op,request,material,None,cut)
}
// First CREATE, SAVE and FULL recheck their respective held operands against
// the already-read, still-unconfirmed snapshot. Other FILE operations retain
// their existing per-command checks. Do not read a slow BODY a second time.
fn publish_sealed_checked(pool:&LocalPool,op:OperationId,request:[u8;32],
    material:impl FnOnce(&journal::Decision)->io::Result<()>,
    at_floor:Option<&dyn Fn(&journal::Journal,&journal::Decision)->io::Result<()>>,
    mut cut:impl FnMut(Cut)->io::Result<()>)->io::Result<journal::Intent>{
    let (stamp,mut state)=pool.snapshot()?;
    let p=state.pending.take().ok_or_else(||invalid("original prepared FILE intent missing"))?;
    if p.frame.op()!=op||p.frame.request()!=request{return Err(invalid("sealed FILE operation or request differs"));}
    pool.system.confirm_record_sync(journal::INTENT,&p.stamp,p.bytes.len())?;pool.system.file.sync_all()?;
    if state.decided(op).is_none(){
        material(&p.frame)?;
        pool.system.append_exact(pool::READY,&stamp,stamp.stat.size(),&p.frame.bytes()[state.tail..],pool::MAX_SYSTEM)?;
        cut(Cut::FrameSynced)?;drop(state);
        let (st,synced)=pool.snapshot()?;let (before,after)=synced.acknowledgement(&p.frame)?;
        if let Some(check)=at_floor{
            let now=synced.pending.as_ref().ok_or_else(||invalid("original sealed FILE intent disappeared at confirmation"))?;
            if now.stamp!=p.stamp||now.bytes!=p.bytes||synced.decided(op).is_some(){
                return Err(invalid("original FILE intent or acknowledgement changed at confirmation"));}
            check(&synced,&p.frame)?;
        }
        pool.system.advance_frontier(pool::READY,&st,pool::MAX_SYSTEM,&before,&after,||cut(Cut::MarkedBeforeSync))?;
    }
    let (_,after)=pool.snapshot()?;
    if !after.decided(op).is_some_and(|d|d.bytes()==p.frame.bytes()){return Err(invalid("different confirmed SYSTEM decision at original FILE cut"));}
    Ok(p)
}

/// Original outcome only. It does not open a data volume or grant access to
/// the FILE body. Published does not assert current installation/availability.
pub fn creation_receipt(system:&Path,op:OperationId)->io::Result<Option<CreationReceipt>>{
    let (_pool,_,state)=LocalPool::with_snapshot(system,Access::Read)?;
    if let Some(f)=state.operation(op){
        let waiting=state.pending.as_ref().is_some_and(|p|p.frame.bytes()==f.bytes.as_slice());
        return Ok(Some(CreationReceipt{status:if waiting{CreationStatus::PublishedInstallationUnconfirmed}else{CreationStatus::Published},
            file:FileInfo::from_frame(f)}));
    }
    Ok(state.allocated.as_ref().filter(|a|a.op==op).map(|a|{
        let status=match state.pending.as_ref(){
            Some(p) if p.frame.bytes().len()==state.tail=>CreationStatus::DecisionAwaitingConfirmation,
            Some(_)=>CreationStatus::Prepared,
            None=>CreationStatus::Reserved,
        };
        CreationReceipt{status,file:FileInfo{entry:a.entry,file:a.file,history:a.history,volume:a.volume,parent:a.parent.address(a.volume),
            name:a.name.clone(),created_by:a.op,decision_sequence:None}}
    }))
}

/// Read the initial FILE by stable ID, not by a name or content hash. Only a
/// physically checked local Unborn/Manual record can yield this zero-byte read.
pub fn read_initial_file(system:&Path,id:FileId)->io::Result<(FileInfo,Vec<u8>)>{
    draft::capture_current(system,id)?.deliver(|(info,reader)|{
        if reader.is_some(){return Err(invalid("FILE has a saved working revision; use current content read"));}
        Ok((info.file,Vec::new()))
    })
}

/// Look up this exact display spelling under a registered ordinary root.
/// No case or normalization fallback and no inference from a physical leaf.
pub fn lookup_file(system:&Path,id:VolumeId,display:&str)->io::Result<Option<FileInfo>>{
    lookup_file_at(system,DirectoryParent::Root(id),display)
}
pub fn lookup_file_at(system:&Path,address:DirectoryParent,display:&str)->io::Result<Option<FileInfo>>{
    storage_name::FileName::parse(display.as_bytes())?;
    let cut=formal::ParentCut::open(system,Access::Read)?;let(p,s)=cut.originals();
    let v=p.ordinary(address.volume())?;let parent=OrdinaryParentLease::at_cut(&v,address,s)?;
    let Some(f)=s.lookup_at(address,display)?else{parent.require_at(s)?;return Ok(None)};
    let _body=draft::admit_current_held(&cut,&v,f)?;
    parent.directory().file.sync_all()?;parent.require_at(s)?;Ok(Some(FileInfo::at(f,address)))
}

/// Every current business name must match its material; an unknown normal
/// name is never skipped. Only original issued private slots can be separated
/// from that namespace, and observed residues are explicitly counted.
pub fn list_initial_files(system:&Path,id:VolumeId)->io::Result<RootFiles>{
    let cut=formal::ParentCut::open(system,Access::Read)?;parent_listing(&cut,DirectoryParent::Root(id)).map(|(files,_)|files)
}
fn parent_listing(cut:&formal::ParentCut,address:DirectoryParent)->io::Result<(RootFiles,Vec<DirectoryInfo>)>{
    let(pool,state)=cut.originals();let volume=pool.ordinary(address.volume())?;
    let parent=OrdinaryParentLease::at_cut(&volume,address,state)?;let actual=parent.begin_listing(state)?.names();
    let mut names=BTreeSet::new();let mut tickets=BTreeMap::new();let mut files=Vec::new();
    for f in state.on_all().filter(|f|f.create.parent.address(f.create.volume)==address){
        // A CREATE-private ticket was issued forever on that original parent,
        // never on the relocated FILE's current volume.
        // Only lower, previously empty private occupants were passed over. The
        // successful original slot must be absent once its name is installed.
        tickets.insert(f.create.attempt,f.create.slot);
    }
    if let Some(a)=state.allocated.as_ref().filter(|a|a.parent.address(a.volume)==address&&state.operation(a.op).is_none()){
        tickets.insert(a.attempt,a.stage.as_ref().map_or(allocation::SLOTS,|s|s.slot+1));
    }
    let handoff=custody::inventory::view(state,&parent,&mut tickets)?;
    for f in state.at_parent(address).filter(|f|handoff.deferred!=Some(f.create.file)){
        if !names.insert(storage_name::file_leaf(&f.create.name,f.create.file)?.into_bytes()){return Err(invalid("duplicate physical FILE name"));}
        files.push(FileInfo::at(f,address));
    }
    let dirs=directory::inventory::observe(state,address,parent.directory(),actual)?;
    if dirs.pending_install{return Err(invalid("published ordinary directory still needs its original installation to finish"));}
    let mut observed=BTreeSet::new();let mut reserved_private_entries=dirs.private_count;
    for raw in actual{
        if handoff.hidden.contains(raw)||dirs.consumed.contains(raw){continue;}
        if handoff.named_stage.as_ref()==Some(raw)||material::private_slot(raw).is_some_and(|(ticket,slot)|tickets.get(&ticket).is_some_and(|&limit|slot<limit)){
            reserved_private_entries+=1;
        }else{observed.insert(raw.clone());}
    }
    if observed!=names{return Err(invalid("missing or unrecognized business inventory; refusing a partial list"));}
    for f in state.at_parent(address).filter(|f|handoff.deferred!=Some(f.create.file)){
        let _=draft::admit_current_held(cut,&volume,f)?;
        if state.head(f.create.file).is_some()&&state.committing.as_ref().is_some_and(|a|a.original.file==f.create.file){
            let saved=state.saved(f.create.file).ok_or_else(||invalid("no original saved source for formal cleanup inventory"))?;
            let dir=formal::directory(&volume,&state,f,saved)?;
            let _=draft::material::Reader::current_if_named(&dir,&saved.original,&saved.material)?;
        }
        if let Some(d)=state.incrementing.as_ref().filter(|a|a.original.file==f.create.file).and_then(|a|state.decided(a.original.op)){
            let saved=state.saved(f.create.file).ok_or_else(||invalid("no FILE origin for INCR cleanup inventory"))?;
            let dir=formal::directory(&volume,&state,f,saved)?;
            for original in state.published_work(d.delta()?)?.rows(){interval::physical::inspect_retained_name(&dir,original)?;}
        }
    }
    parent.directory().file.sync_all()?;parent.require_at(state)?;
    // Check the actual directory entities again, then make one final parent
    // census. The initial held sample is shared by FILEs and directories.
    let again=directory::inventory::observe(state,address,parent.directory(),actual)?;
    if again.pending_install||again.consumed!=dirs.consumed{return Err(invalid("ordinary directories changed during enumeration"));}
    parent.finish_listing(state)?;
    let folders=again.rows.into_iter().map(DirectoryInfo::original).collect();
    files.sort_by(|a,b|a.name.cmp(&b.name));Ok((RootFiles{files,reserved_private_entries,pending_moves:handoff.pending,pending_lifecycle:handoff.lifecycle},folders))
}

#[cfg(test)]
#[path="../tests/unit/ordinary.rs"]
mod tests;
