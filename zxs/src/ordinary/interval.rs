//! One native decision per changed extent or explicit resize, relative to an
//! original confirmed FILE revision rooted in its exact formal baseline.
//! Original namespace/custody and the formal branch/node are not changed.
pub(super) mod types;
pub(super) mod allocation;
pub(super) mod physical;
pub(in crate::ordinary) mod layout;
use super::{allocation as ordinary_allocation,draft::{self,Body},formal::{self,types::Commit},
    journal::{self,Decision,Journal},wire::Frame,publish_sealed,Cut,CreationStatus,CurrentFile};
use self::{allocation::SpanAllocation,types::{Original,Saved,SpanEdit,SpanReceipt},physical::{Capture,Verified}};
use crate::{fs::{self,invalid},pool::lease::{Access,LocalPool},FileId,OperationId};
use std::{io::{self,Read,Write},path::Path};

#[derive(Clone,Debug,Eq,PartialEq)]
pub struct SpanObservation{pub file:FileId,pub operation:OperationId,pub status:CreationStatus,pub edit:SpanEdit,pub published:Option<SpanReceipt>}
pub fn interval_receipt(system:&Path,op:OperationId)->io::Result<Option<SpanObservation>>{
    let (_,_,s)=LocalPool::with_snapshot(system,Access::Read)?;
    if let Some(d)=s.decided(op){
        let e=d.span()?;let open=s.editing.as_ref().is_some_and(|a|a.original.op==op);
        return Ok(Some(SpanObservation{file:e.original.file,operation:op,edit:e.original.edit,status:if open{CreationStatus::PublishedInstallationUnconfirmed}else{CreationStatus::Published},published:Some(e.receipt())}));
    }
    Ok(s.editing.as_ref().filter(|a|a.original.op==op).map(|a|SpanObservation{
        file:a.original.file,operation:op,edit:a.original.edit,published:None,status:match &s.pending{
            Some(p) if p.frame.bytes().len()==s.tail=>CreationStatus::DecisionAwaitingConfirmation,
            Some(_)=>CreationStatus::Prepared,None=>CreationStatus::Reserved,
        },
    }))
}
enum Choice<'a>{Done(SpanReceipt),Active(&'a Frame,&'a Commit,Original,[u8;32])}
fn choice<'a>(p:&LocalPool,s:&'a Journal,op:OperationId,id:FileId,revision:u64,edit:SpanEdit)->io::Result<Choice<'a>>{
    p.require_business_op(op)?;let req=types::request(p.id(),fs::local_uid(),op,id,revision,edit)?;
    if let Some(d)=s.decided(op){
        let r=d.span()?;if r.request!=req{return Err(invalid("different original interval request under same operation"));}
        if !s.editing.as_ref().is_some_and(|a|a.original.op==op){return Ok(Choice::Done(r.receipt()));}
    }
    if s.moving.is_some()||s.incrementing.is_some()||s.allocated.is_some()||s.saving.is_some()||s.committing.is_some()||s.editing.as_ref().is_some_and(|a|a.original.op!=op||a.request!=req){
        return Err(invalid("different original FILE operation needs completion"));
    }
    if s.delta(id).is_some(){return Err(invalid("this working editor does not yet support a formal INCR base"));}
    let c=s.file(id).ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no published FILE"))?;
    let f=s.head(id).ok_or_else(||invalid("interval editing requires an original formal FILE baseline"))?;
    let prior=s.intervals(id).rev().find(|e|e.original.op!=op);
    let o=Original::select(op,fs::local_uid(),s.placement(id)?,f,prior,revision,edit)?;
    if s.editing.as_ref().is_some_and(|a|a.original!=o){return Err(invalid("original interval base differs"));}
    if s.editing.is_none(){s.room_for(super::wire::MAX_FRAME)?;}
    Ok(Choice::Active(c,f,o,req))
}

enum Source{Done(SpanReceipt),Input(Capture)}
fn source(system:&Path,op:OperationId,id:FileId,rev:u64,edit:SpanEdit)->io::Result<Source>{
    let (p,_,s)=LocalPool::with_snapshot(system,Access::Incoming)?;
    let (c,f,o)=match choice(&p,&s,op,id,rev,edit)?{Choice::Done(r)=>return Ok(Source::Done(r)),Choice::Active(c,f,o,_)=>(c,f,o)};
    let w=s.saved(id).ok_or_else(||invalid("formal original lacks its first confirmed save"))?;
    let site=s.placement(c.create.file)?;let v=p.ordinary(site.volume())?;let dir=formal::directory(&v,&s,c,w)?;
    let base=site.full(&dir,f)?;
    let prior=physical::Operands::capture(&dir,site,f.original.content.bytes,s.intervals(id).filter(|r|r.original.op!=op))?;
    v.require_current()?;
    Ok(Source::Input(Capture::open(dir,base,prior,o,s.editing.as_ref(),s.pending.is_some()||s.decided(op).is_some())?))
}
pub fn save_interval(system:&Path,op:OperationId,file:FileId,revision:u64,e:SpanEdit,body:impl Read)->io::Result<SpanReceipt>{
    save_inner(system,op,file,revision,e,body,|_|Ok(()))
}
fn save_inner(system:&Path,op:OperationId,file:FileId,revision:u64,e:SpanEdit,input:impl Read,mut cut:impl FnMut(Cut)->io::Result<()>)->io::Result<SpanReceipt>{
    let checked=match source(system,op,file,revision,e)?{Source::Done(r)=>return Ok(r),Source::Input(capture)=>capture.verify(input)?};
    writer(system,op,file,revision,e,checked,&mut cut)
}
fn writer(system:&Path,op:OperationId,file:FileId,rev:u64,e:SpanEdit,checked:Verified,cut:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<SpanReceipt>{
    let (p,_,s)=LocalPool::with_snapshot(system,Access::Write)?;
    let (c,o,request)=match choice(&p,&s,op,file,rev,e)?{Choice::Done(r)=>return Ok(r),Choice::Active(c,_,o,r)=>(c,o,r)};
    let w=s.saved(file).ok_or_else(||invalid("formal base lacks its exact save original"))?;
    let site=s.placement(c.create.file)?;let v=p.ordinary(site.volume())?;let dir=formal::directory(&v,&s,c,w)?;v.require_current()?;
    checked.source_current(&dir,&o)?;
    if s.editing.is_none(){let (seq,prev)=s.frontier();SpanAllocation::reserve(&p.system,p.id(),o,seq+1,prev)?;}
    drop(s);cut(Cut::Reservation)?;
    let (_,mut s)=p.snapshot()?;
    let a=s.editing.take().ok_or_else(||invalid("original working interval reservation disappeared"))?;
    if a.original!=*checked.original()||a.request!=request{return Err(invalid("another original interval reservation"));}
    p.system.confirm_record_sync(ordinary_allocation::NAME,&a.stamp,a.bytes.len())?;p.system.file.sync_all()?;
    let bound=checked.place(&dir,&a)?;let material=bound.feedback(&dir)?;
    if let Some(d)=s.decided(op){
        if s.pending.is_none(){
            let f=d.span()?;if !a.matches(f)||f.material!=material{return Err(invalid("different retained interval material"));}
            v.require_current()?;p.system.remove_original_record(ordinary_allocation::NAME,&a.stamp,&a.bytes)?;return Ok(f.receipt());
        }
    }
    if s.pending.is_none(){
        let f=Saved::new(p.id(),a.seq,a.previous,a.original.clone(),a.digest,material)?;
        if !a.matches(Decision::read(p.id(),&f.bytes)?.span()?){return Err(invalid("cannot seal original working range"));}
        s.room_for(f.bytes.len())?;cut(Cut::BeforeIntent)?;journal::Intent::seal(&p.system,&f.bytes)?;
    }
    drop(s);cut(Cut::Intent)?;
    let seal=publish_sealed(&p,op,request,|d|{
        v.require_current()?;checked.base_current(&dir)?;let f=d.span()?;
        if !a.matches(f)||bound.feedback(&dir)?!=f.material{return Err(invalid("original span or baseline changed before decision"));}Ok(())
    },|s|cut(s))?;
    let f=seal.frame.span()?;cut(Cut::Decision)?;v.require_current()?;checked.base_current(&dir)?;
    if bound.feedback(&dir)?!=f.material{return Err(invalid("published interval cannot reopen"));}cut(Cut::Installation)?;
    p.system.remove_original_record(journal::INTENT,&seal.stamp,&seal.bytes)?;cut(Cut::IntentReleased)?;
    p.system.remove_original_record(ordinary_allocation::NAME,&a.stamp,&a.bytes)?;cut(Cut::Release)?;Ok(f.receipt())
}
/// Random current reads apply to a clean formal or a confirmed working revision.
/// The initial uncommitted single-stream representation offers current full
/// reads only; do not hide a whole-prefix scan behind a random-read contract.
pub fn read_current_range_to(system:&Path,file:FileId,offset:u64,length:u64,mut out:impl Write)->io::Result<CurrentFile>{
    let (info,body)=draft::capture_current(system,file)?;
    match &body{
        Some(Body::Formal(b))=>b.write_range(offset,length,&mut out)?,
        Some(Body::Incr(b))=>b.range(offset,length,&mut out)?,
        Some(Body::Composed(b))=>b.write_range(offset,length,&mut out)?,
        Some(Body::Working(_))=>return Err(io::Error::new(io::ErrorKind::Unsupported,"first uncommitted draft supports full sequential read only")),
        None if offset==0&&length==0=>{},
        None=>return Err(invalid("range outside an empty original FILE")),
    }
    draft::finish_output(&body,&mut out)?;Ok(info)
}
#[cfg(test)]
#[path="../../tests/unit/working_interval.rs"]
mod tests;
