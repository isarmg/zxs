//! The first explicit manual main-branch commit consumes one particular
//! confirmed working revision. Ordinary reads, writes and close never enter
//! this command. All authority is the existing SYSTEM publication frontier.
pub(super) mod types;
pub(super) mod allocation;
pub(super) mod physical;
mod indexed;
use super::{allocation as ordinary_allocation,draft::{material::Reader,types::SaveFrame},journal::{self,Decision,Journal},
    material as container,wire::Frame,Cut,CreationStatus,publish_sealed};
use crate::{FileId,NodeId,OperationId,fs::{self,HeldDir,invalid},pool::lease::{Access,FileVolume,LocalPool}};
use self::{allocation::FormalAllocation,types::{Commit,FormalNode,Original},physical::{HistoryBody,Input,Verified}};
use std::{io::{self,Write},path::Path};

#[derive(Clone,Debug,Eq,PartialEq)]
pub struct FormalObservation{pub file:FileId,pub operation:OperationId,pub status:CreationStatus,pub published:Option<FormalNode>}
pub fn first_commit_receipt(system:&Path,op:OperationId)->io::Result<Option<FormalObservation>>{
    let (_,_,s)=LocalPool::with_snapshot(system,Access::Read)?;
    if let Some(d)=s.decided(op){
        let f=d.formal()?;let pending=s.committing.as_ref().is_some_and(|a|a.original.op==op);
        return Ok(Some(FormalObservation{file:f.original.file,operation:op,status:if pending{CreationStatus::PublishedInstallationUnconfirmed}
            else{CreationStatus::Published},published:Some(f.receipt())}));
    }
    Ok(s.committing.as_ref().filter(|a|a.original.op==op).map(|a|FormalObservation{
        file:a.original.file,operation:op,published:None,status:match &s.pending{
            Some(p) if p.frame.bytes().len()==s.tail=>CreationStatus::DecisionAwaitingConfirmation,
            Some(_)=>CreationStatus::Prepared,None=>CreationStatus::Reserved,
        },
    }))
}
enum Choice<'a>{Done(FormalNode),Active(&'a Frame,&'a SaveFrame,[u8;32])}
fn choice<'a>(p:&LocalPool,s:&'a Journal,op:OperationId,file:FileId,rev:u64)->io::Result<Choice<'a>>{
    p.require_business_op(op)?;let req=types::request(p.id(),fs::local_uid(),op,file,rev)?;
    if let Some(d)=s.decided(op){
        let f=d.formal()?;if f.request!=req{return Err(invalid("different initial formal commit request for original operation"));}
        if !s.committing.as_ref().is_some_and(|a|a.original.op==op){return Ok(Choice::Done(f.receipt()));}
    }
    if s.moving.is_some()||s.incrementing.is_some()||s.editing.is_some()||s.allocated.is_some()||s.saving.is_some()||s.committing.as_ref().is_some_and(|a|a.original.op!=op||a.request!=req){
        return Err(invalid("another original FILE operation needs completion"));
    }
    let c=s.file(file).ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no original FILE for formal commit"))?;
    let w=s.saved(file).ok_or_else(||invalid("no confirmed working revision; explicit first commit needs one"))?;
    if s.head(file).is_some_and(|f|f.original.op!=op){return Err(invalid("FILE already has a formal node; not a first commit"));}
    if s.committing.is_none(){s.room_for(super::wire::MAX_FRAME)?;}
    Ok(Choice::Active(c,w,req))
}
/// Enroll every actual old working name from this FILE's authenticated prefix.
/// Current explicitly requires every retained extent. A historical read admits
/// issued names if present, never adopts their bytes or requires them available.
pub(in crate::ordinary) fn working_names(s:&Journal,c:&Frame,w:&SaveFrame,history:bool)->Vec<(OperationId,bool)>{
    let file=c.create.file;
    if s.head(file).is_none(){return vec![(w.original.op,true)];}
    if s.committing.as_ref().is_some_and(|a|a.original.file==file){return vec![(w.original.op,false)];}
    let done=s.delta(file).is_some();
    if done&&!s.incrementing.as_ref().is_some_and(|a|a.original.file==file){return Vec::new();}
    let mut names:Vec<_>=s.intervals(file).map(|e|(e.original.op,!done&&!history)).collect();
    if let Some(a)=s.editing.as_ref().filter(|a|a.original.file==file){
        if s.decided(a.original.op).is_none(){names.push((a.original.op,!history&&s.pending.is_some()));}
    }
    names
}
fn directory_at(v:&FileVolume<'_>,s:&Journal,c:&Frame,work:Vec<(OperationId,bool)>,
    f:Option<(&Original,bool)>,d:Option<(&super::delta::types::Original,bool)>)->io::Result<HeldDir>{
    if let Some(dir)=super::custody::placed::active_dir(s,v,c)?{
        container::require_entries(&dir,work,f,d)?;dir.require_named()?;return Ok(dir)
    }
    container::directory_all(v,&c.create,work,f,d)
}
pub(in crate::ordinary) fn directory(v:&FileVolume<'_>,s:&Journal,c:&Frame,w:&SaveFrame)->io::Result<HeldDir>{
    let a=s.committing.as_ref().filter(|a|a.original.file==c.create.file);
    let f=s.head(c.create.file);
    let formal=f.map(|f|(&f.original,true)).or_else(||a.map(|a|(&a.original,false)));
    let delta=s.delta(c.create.file).map(|d|(&d.original,true)).or_else(||s.incrementing.as_ref()
        .filter(|i|i.original.file==c.create.file).map(|i|(&i.original,s.pending.is_some())));
    let dir=directory_at(v,s,c,working_names(s,c,w,false),formal,delta)?;
    if let Some(a)=a{
        let sealed=s.pending.as_ref().map(|p|p.frame.formal()).transpose()?;
        physical::inspect_prepared(&dir,a,sealed.or(f))?;
    }
    Ok(dir)
}
enum NodeBody{Full(HistoryBody),Incr(super::delta::physical::Body)}
fn open_node(system:&Path,file:FileId,node:NodeId)->io::Result<(FormalNode,NodeBody)>{
    let (p,_,s)=LocalPool::with_snapshot(system,Access::Read)?;
    let selected_delta=s.delta_node(file,node);
    let f=if selected_delta.is_some(){s.head(file)}else{s.node(file,node)}.ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no such published node in this FILE"))?;
    let c=s.file(file).ok_or_else(||invalid("formal node lacks original FILE"))?;
    let w=s.saved(file).ok_or_else(||invalid("formal node lacks its original save"))?;let v=p.ordinary(s.volume_of(c))?;
    let site=s.placement(file)?;
    let incr=s.delta(file).map(|d|(&d.original,selected_delta.is_some())).or_else(||s.incrementing.as_ref()
        .filter(|a|a.original.file==file).map(|a|(&a.original,false)));
    let dir=directory_at(&v,&s,c,working_names(&s,c,w,true),Some((&f.original,true)),incr)?;
    let parent=site.full(&dir,f)?.user_read()?;
    let result=match selected_delta{
        None=>(f.receipt(),NodeBody::Full(parent)),
        Some(d)=>{
            (d.receipt(),NodeBody::Incr(site.incremental(&dir,parent,d)?))
        }
    };
    v.root.file.sync_all()?;v.require_current()?;Ok(result)
}
pub fn read_node_to(system:&Path,file:FileId,node:NodeId,mut out:impl Write)->io::Result<FormalNode>{
    let (receipt,body)=open_node(system,file,node)?;
    match &body{NodeBody::Full(b)=>b.write_to(&mut out)?,NodeBody::Incr(b)=>b.whole(&mut out)?,}
    out.flush()?;if let NodeBody::Incr(b)=body{b.unchanged()?;}Ok(receipt)
}
pub fn read_node_range_to(system:&Path,file:FileId,node:NodeId,offset:u64,length:u64,mut out:impl Write)->io::Result<FormalNode>{
    let (receipt,body)=open_node(system,file,node)?;
    match &body{NodeBody::Full(b)=>b.write_range(offset,length,&mut out)?,NodeBody::Incr(b)=>b.range(offset,length,&mut out)?,}
    out.flush()?;if let NodeBody::Incr(b)=body{b.unchanged()?;}Ok(receipt)
}
enum Source{Done(FormalNode),Input(Input)}
fn source(system:&Path,op:OperationId,file:FileId,revision:u64)->io::Result<Source>{
    let (p,_,s)=LocalPool::with_snapshot(system,Access::Incoming)?;
    let (c,w)=match choice(&p,&s,op,file,revision)?{Choice::Done(f)=>return Ok(Source::Done(f)),Choice::Active(c,w,_)=>(c,w)};
    let a=s.committing.as_ref();let decided=s.head(file);let v=p.ordinary(c.create.volume)?;
    let dir=directory(&v,&s,c,w)?;
    let old=Reader::current_if_named(&dir,&w.original,&w.material)?;
    let o=match a{Some(a)=>a.original.clone(),None=>Original::select(op,fs::local_uid(),c,w)?};
    if !o.matches(c,w){return Err(invalid("working FILE no longer has this precise first-commit source"));}
    v.require_current()?;
    Ok(Source::Input(Input::open(dir,o,old,a,decided)?))
}
pub fn commit_first(system:&Path,op:OperationId,file:FileId,expected_revision:u64)->io::Result<FormalNode>{
    commit_inner(system,op,file,expected_revision,|_|Ok(()))
}
fn commit_inner(system:&Path,op:OperationId,file:FileId,revision:u64,mut cut:impl FnMut(Cut)->io::Result<()>)->io::Result<FormalNode>{
    let prepared=match source(system,op,file,revision)?{Source::Done(f)=>return Ok(f),Source::Input(i)=>i.verify()?};
    writer(system,op,file,revision,prepared,&mut cut)
}
fn writer(system:&Path,op:OperationId,file:FileId,revision:u64,prepared:Verified,cut:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<FormalNode>{
    let (p,_,s)=LocalPool::with_snapshot(system,Access::Write)?;
    let (c,w,request)=match choice(&p,&s,op,file,revision)?{
        Choice::Done(f)=>return Ok(f),Choice::Active(c,w,r)=>(c,w,r),
    };
    let v=p.ordinary(c.create.volume)?;let dir=directory(&v,&s,c,w)?;let o=prepared.original();
    if o.op!=op||o.uid!=fs::local_uid()||!o.matches(c,w)||s.committing.as_ref().is_some_and(|a|a.original!=*o){
        return Err(invalid("prepared FULL does not belong to the same original FILE, save and original operation"));
    }
    prepared.source_current(&dir,o)?;v.require_current()?;
    if s.committing.is_none(){
        s.require_free_formal_ids(o)?;
        let (seq,prev)=s.frontier();FormalAllocation::reserve(&p.system,p.id(),o.clone(),seq+1,prev)?;
    }
    drop(s);cut(Cut::Reservation)?;
    let (_,mut held)=p.snapshot()?;
    let a=held.committing.take().ok_or_else(||invalid("original formal allocation disappeared"))?;
    if a.original!=*prepared.original()||a.request!=request{return Err(invalid("another formal reservation cannot accept this material"));}
    p.system.confirm_record_sync(ordinary_allocation::NAME,&a.stamp,a.bytes.len())?;p.system.file.sync_all()?;
    let body=prepared.install(&dir,&a,||cut(Cut::FormalIndex))?;
    let materials=body.feedback(&dir)?;
    if let Some(d)=held.decided(op){
        if held.pending.is_none(){
            let f=d.formal()?;if !a.matches(f)||materials!=f.materials{return Err(invalid("different confirmed formal FILE operands"));}
            v.require_current()?;prepared.retire(&dir,f,||cut(Cut::WorkReleased))?;
            p.system.remove_original_record(ordinary_allocation::NAME,&a.stamp,&a.bytes)?;return Ok(f.receipt());
        }
    }
    if held.pending.is_none(){
        let f=Commit::new(p.id(),a.seq,a.previous,a.original.clone(),a.digest,materials)?;
        if !a.matches(Decision::read(p.id(),&f.bytes)?.formal()?){return Err(invalid("cannot seal real formal FILE node"));}
        held.room_for(f.bytes.len())?;cut(Cut::BeforeIntent)?;journal::Intent::seal(&p.system,&f.bytes)?;
    }
    drop(held);cut(Cut::Intent)?;
    let sealed=publish_sealed(&p,op,request,|d|{
        let f=d.formal()?;v.require_current()?;prepared.source_current(&dir,&f.original)?;
        if !a.matches(f)||body.feedback(&dir)?!=f.materials{return Err(invalid("formal FILE changed before publication"));}Ok(())
    },|s|cut(s))?;
    let f=sealed.frame.formal()?;cut(Cut::Decision)?;
    v.require_current()?;if body.feedback(&dir)?!=f.materials{return Err(invalid("original published FULL cannot reopen"));}
    // Do not remove or clear the working source before its independent FULL
    // and actual local history metadata are bound by the one confirmed frame.
    prepared.retire(&dir,f,||cut(Cut::WorkReleased))?;cut(Cut::Installation)?;
    p.system.remove_original_record(journal::INTENT,&sealed.stamp,&sealed.bytes)?;cut(Cut::IntentReleased)?;
    p.system.remove_original_record(ordinary_allocation::NAME,&a.stamp,&a.bytes)?;cut(Cut::Release)?;
    Ok(f.receipt())
}
#[cfg(test)]
#[path="../../tests/unit/first_formal.rs"]
mod tests;
