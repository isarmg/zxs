//! First ordinary vertical path: a real, initially empty manual FILE in a
//! selected registered ordinary root. Creation does not publish a version.
mod custody;
pub use custody::{move_file,move_receipt,MoveReceipt,MoveStatus};
mod wire;
mod content;
mod indexed;
mod output;
mod interval;
pub use interval::{save_interval,interval_receipt,read_current_range_to,SpanObservation};
pub use interval::types::{SpanEdit,LengthPolicy,SpanReceipt};
mod formal;
mod delta;
pub use delta::{commit_working,formal_receipt};
pub use formal::{commit_first,first_commit_receipt,read_node_to,read_node_range_to,FormalObservation};
pub use formal::types::{FormalNode,FormalStorage};
mod draft;
pub use draft::{save_first,first_save_receipt,read_current_to,current_info,CurrentFile,FirstSaveObservation};
pub use draft::types::{DraftClaim,DraftReceipt,DRAFT_CHUNK};
mod material;
mod allocation;
pub(crate) mod journal;

use crate::{fs::{self,invalid},pool::{self,lease::{Access,LocalPool}},EntryId,FileId,HistoryId,OperationId,VolumeId};
use std::{collections::{BTreeMap,BTreeSet},io,path::Path};
use wire::Frame;

#[derive(Clone,Debug,Eq,PartialEq)]
pub struct FileInfo{
    pub entry:EntryId,pub file:FileId,pub history:HistoryId,pub volume:VolumeId,
    pub name:String,pub created_by:OperationId,pub decision_sequence:Option<u64>,
}
impl FileInfo{
    fn from_frame(f:&Frame)->Self{Self::at(f,f.create.volume)}
    fn at(f:&Frame,volume:VolumeId)->Self{
        let c=&f.create;
        Self{entry:c.entry,file:c.file,history:c.history,volume,name:c.name.clone(),
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
pub struct RootFiles{pub files:Vec<FileInfo>,pub reserved_private_entries:usize,pub pending_moves:Vec<MoveReceipt>}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Cut { Reservation,UnboundMkdir,MoveLeafLinked,MoveInstalled,MoveTargetFlagBeforeSync,MoveTargetChecked,MoveRetireFlagBeforeSync,MoveSourceAdmitted,MoveSourceIsolated,MoveEraseFlagBeforeSync,MoveLeafRetired,MoveEmptyFlagBeforeSync,MoveEmptyConfirmed,MoveDirectoryUnlinked,FormalIndex,IncrLinked,WorkUnlinked,WorkReleased,BeforeIntent,Intent,FrameSynced,MarkedBeforeSync,Decision,Installation,IntentReleased,Release }

fn expected(pool:&LocalPool,op:OperationId,v:VolumeId,name:&str)->io::Result<[u8;32]>{
    wire::request_digest(pool.id(),op,fs::local_uid(),v,name)
}
fn same_request(frame:&Frame,digest:[u8;32])->io::Result<()>{
    if frame.request!=digest{return Err(invalid("operation ID already belongs to another original request"));}
    Ok(())
}

/// The result is the original publication, not an online health check. An
/// unresolved original intent can only be continued by its exact original request.
pub fn create_empty(system:&Path,op:OperationId,volume:VolumeId,name:&str)->io::Result<FileInfo>{
    create_inner(system,op,volume,name,|_|Ok(()))
}
fn create_inner(system:&Path,op:OperationId,id:VolumeId,name:&str,mut cut:impl FnMut(Cut)->io::Result<()>)->io::Result<FileInfo>{
    wire::human_name(name.as_bytes())?;
    let (pool,_,state)=LocalPool::with_snapshot(system,Access::Write)?;
    pool.require_business_op(op)?;
    let request=expected(&pool,op,id,name)?;
    if let Some(d)=state.decided(op){
        let done=d.creation()?;same_request(done,request)?;
        if !state.allocated.as_ref().is_some_and(|a|a.op==op){
            return Ok(FileInfo::from_frame(done)); // Do not open a later same-named root.
        }
    }
    if state.moving.is_some()||state.incrementing.is_some()||state.editing.is_some()||state.saving.is_some()||state.committing.is_some(){return Err(invalid("a different original FILE command needs completion"));}
    if let Some(a)=&state.allocated{
        if a.op!=op || a.request!=request {return Err(invalid("another original FILE allocation needs completion"));}
    }else{
        if state.named(id,name).is_some(){return Err(io::Error::new(io::ErrorKind::AlreadyExists,"FILE name is already registered"));}
        state.room_for(wire::MAX_FRAME)?;
    }
    let volume=pool.ordinary(id)?;
    if state.allocated.is_none(){
        let file=FileId::new()?;let entry=EntryId::new()?;let history=HistoryId::new()?;
        if state.file(file).is_some() || state.on_all().any(|f|f.create.entry==entry||f.create.history==history){
            return Err(invalid("new FILE identity collides with an original decision"));
        }
        let (seq,previous)=state.frontier();
        let (attempt,root)=material::select_ticket(&volume,name)?;
        if state.ticket_used(attempt)||op==attempt{return Err(invalid("new private ticket collides with earlier operation"));}
        allocation::Allocation::reserve(&pool.system,pool.id(),op,id,name,seq+1,previous,entry,file,history,attempt,root)?;
    }
    drop(state);
    cut(Cut::Reservation)?;
    let (_,mut held)=pool.snapshot()?;
    let mut a=held.allocated.take().ok_or_else(||invalid("original FILE allocation disappeared"))?;
    if a.op!=op||a.request!=request{return Err(invalid("unexpected FILE allocation"));}
    pool.system.confirm_record_sync(allocation::NAME,&a.stamp,a.bytes.len())?;
    pool.system.file.sync_all()?;
    if a.stage.is_none(){
        material::bind_fresh_stage(&volume,&a,||cut(Cut::UnboundMkdir))?;
        let original=a.digest;
        a=allocation::Allocation::read(&pool.system,pool.id())?.ok_or_else(||invalid("FILE allocation lost during stage binding"))?;
        if a.digest!=original||a.stage.is_none(){return Err(invalid("stage did not bind to the original reservation"));}
    }
    // A prior completion may have removed only its sealed material intent.
    if let Some(done)=held.operation(op){
        if held.pending.is_none(){
            material::install_decided(&volume,&done.create)?;
            pool.system.remove_original_record(allocation::NAME,&a.stamp,&a.bytes)?;
            return Ok(FileInfo::from_frame(done));
        }
    }
    if held.pending.is_none(){
        let creation=material::prepare(&volume,&a)?;
        let f=Frame::new(pool.id(),a.seq,a.previous,creation)?;
        if Frame::read(pool.id(),&f.bytes)?!=f||!a.matches(&f){return Err(invalid("could not seal complete FILE decision"));}
        held.room_for(f.bytes.len())?;
        cut(Cut::BeforeIntent)?;
        journal::Intent::seal(&pool.system,&f.bytes)?;
    }
    drop(held);
    cut(Cut::Intent)?;
    let p=publish_sealed(&pool,op,request,|decision|{
        material::require_staged(&volume,&decision.creation()?.create)
    },|step|cut(step))?;
    let f=p.frame.creation()?;
    cut(Cut::Decision)?;
    material::install_decided(&volume,&f.create)?;
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
    material:impl FnOnce(&journal::Decision)->io::Result<()>,mut cut:impl FnMut(Cut)->io::Result<()>)->io::Result<journal::Intent>{
    let (stamp,mut state)=pool.snapshot()?;
    let p=state.pending.take().ok_or_else(||invalid("original prepared FILE intent missing"))?;
    if p.frame.op()!=op||p.frame.request()!=request{return Err(invalid("sealed FILE operation or request differs"));}
    pool.system.confirm_record_sync(journal::INTENT,&p.stamp,p.bytes.len())?;pool.system.file.sync_all()?;
    if state.decided(op).is_none(){
        material(&p.frame)?;
        pool.system.append_exact(pool::READY,&stamp,stamp.stat.size(),&p.frame.bytes()[state.tail..],pool::MAX_SYSTEM)?;
        cut(Cut::FrameSynced)?;drop(state);
        let (st,synced)=pool.snapshot()?;let (before,after)=synced.acknowledgement(&p.frame)?;
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
        CreationReceipt{status,file:FileInfo{entry:a.entry,file:a.file,history:a.history,volume:a.volume,
            name:a.name.clone(),created_by:a.op,decision_sequence:None}}
    }))
}

/// Read the initial FILE by stable ID, not by a name or content hash. Only a
/// physically checked local Unborn/Manual record can yield this zero-byte read.
pub fn read_initial_file(system:&Path,id:FileId)->io::Result<(FileInfo,Vec<u8>)>{
    let (info,reader)=draft::capture_current(system,id)?;
    if reader.is_some(){return Err(invalid("FILE has a saved working revision; use current content read"));}
    Ok((info.file,Vec::new()))
}

/// Every current business name must match its material; an unknown normal
/// name is never skipped. Only original issued private slots can be separated
/// from that namespace, and observed residues are explicitly counted.
pub fn list_initial_files(system:&Path,id:VolumeId)->io::Result<RootFiles>{
    let (pool,_,state)=LocalPool::with_snapshot(system,Access::Read)?;
    let volume=pool.ordinary(id)?;
    let mut names=BTreeSet::new();let mut tickets=BTreeMap::new();let mut files=Vec::new();
    for f in state.on_all().filter(|f|f.create.volume==id){
        // A CREATE-private ticket was issued forever on that original root,
        // never on the relocated FILE's current volume.
        // Only lower, previously empty private occupants were passed over. The
        // successful original slot must be absent once its name is installed.
        tickets.insert(f.create.attempt,f.create.slot);
    }
    if let Some(a)=state.allocated.as_ref().filter(|a|a.volume==id&&state.operation(a.op).is_none()){
        tickets.insert(a.attempt,a.stage.as_ref().map_or(allocation::SLOTS,|s|s.slot+1));
    }
    let handoff=custody::inventory::view(&state,&volume,&mut tickets)?;
    for f in state.on(id).filter(|f|handoff.deferred!=Some(f.create.file)){
        names.insert(f.create.name.as_bytes().to_vec());files.push(FileInfo::at(f,id));
    }
    let actual:BTreeSet<_>=volume.root.names()?.into_iter().collect();
    let mut observed=BTreeSet::new();let mut reserved_private_entries=0;
    for raw in &actual{
        if handoff.hidden.contains(raw){continue;}
        if handoff.named_stage.as_ref()==Some(raw)||material::private_slot(raw).is_some_and(|(ticket,slot)|tickets.get(&ticket).is_some_and(|&limit|slot<limit)){
            reserved_private_entries+=1;
        }else{observed.insert(raw.clone());}
    }
    if observed!=names{return Err(invalid("missing or unrecognized business inventory; refusing a partial list"));}
    for f in state.on(id).filter(|f|handoff.deferred!=Some(f.create.file)){
        let _=draft::admit_current_held(&state,&volume,f)?;
        if state.head(f.create.file).is_some()&&state.committing.as_ref().is_some_and(|a|a.original.file==f.create.file){
            let saved=state.saved(f.create.file).ok_or_else(||invalid("no original saved source for formal cleanup inventory"))?;
            let dir=formal::directory(&volume,&state,f,saved)?;
            let _=draft::material::Reader::current_if_named(&dir,&saved.original,&saved.material)?;
        }
        if state.delta(f.create.file).is_some()&&state.incrementing.as_ref().is_some_and(|a|a.original.file==f.create.file){
            let saved=state.saved(f.create.file).ok_or_else(||invalid("no FILE origin for INCR cleanup inventory"))?;
            let dir=formal::directory(&volume,&state,f,saved)?;
            for original in state.intervals(f.create.file){interval::physical::inspect_retained_name(&dir,original)?;}
        }
    }
    volume.root.file.sync_all()?;volume.require_current()?;
    if volume.root.names()?.into_iter().collect::<BTreeSet<_>>()!=actual{return Err(invalid("root changed during FILE enumeration"));}
    files.sort_by(|a,b|a.name.cmp(&b.name));Ok(RootFiles{files,reserved_private_entries,pending_moves:handoff.pending})
}

#[cfg(test)]
#[path="../tests/unit/ordinary.rs"]
mod tests;
