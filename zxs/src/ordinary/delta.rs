//! First genuine formal INCR after the original main FULL. The ordinary
//! working editor remains separate and never implicitly calls this command.
pub(in crate::ordinary) mod types;
pub(in crate::ordinary) mod allocation;
pub(in crate::ordinary) mod map;
pub(in crate::ordinary) mod physical;
use super::{allocation as common,draft::types::SaveFrame,formal::{self,types::FormalNode,FormalObservation},
    interval::{types::Saved,physical::{Composed,Operands}},journal::{self,Decision,Journal,WorkWindow},wire::{self,Frame},Cut,CreationStatus,publish_sealed};
use self::{allocation::DeltaAllocation,map::Map,physical::{Input,Verified},types::{Delta,Original}};
use crate::{FileId,OperationId,fs::{self,invalid},pool::lease::{Access,LocalPool}};
use std::{io,path::Path};

pub fn commit_working(system:&Path,op:OperationId,file:FileId,expected_revision:u64)->io::Result<FormalNode>{
    if expected_revision==1{return formal::commit_first(system,op,file,expected_revision);}
    commit_inner(system,op,file,expected_revision,|_|Ok(()))
}
pub fn formal_receipt(system:&Path,op:OperationId)->io::Result<Option<FormalObservation>>{
    let (_,_,s)=LocalPool::with_snapshot(system,Access::Read)?;
    if let Some(d)=s.decided(op){
        let (r,pending)=match d{
            Decision::Formal(f)=>(f.receipt(),s.committing.as_ref().is_some_and(|a|a.original.op==op)),
            Decision::Delta(f)=>(f.receipt(),s.incrementing.as_ref().is_some_and(|a|a.original.op==op)),
            _=>return Err(invalid("original operation is not an explicit formal commit")),
        };
        return Ok(Some(FormalObservation{file:r.file,operation:op,status:if pending{CreationStatus::PublishedInstallationUnconfirmed}else{CreationStatus::Published},published:Some(r)}));
    }
    let file=s.committing.as_ref().filter(|a|a.original.op==op).map(|a|a.original.file)
        .or_else(||s.incrementing.as_ref().filter(|a|a.original.op==op).map(|a|a.original.file));
    Ok(file.map(|file|FormalObservation{file,operation:op,status:match &s.pending{
        Some(p) if p.frame.bytes().len()==s.tail=>CreationStatus::DecisionAwaitingConfirmation,
        Some(_)=>CreationStatus::Prepared,None=>CreationStatus::Reserved,
    },published:None}))
}
struct SourceCut<'a>{c:&'a Frame,initial:&'a SaveFrame,work:WorkWindow<'a>,edge:&'a Saved,map:Map,req:[u8;32]}
enum Choice<'a>{Done(FormalNode),Active(SourceCut<'a>)}
fn choice<'a>(p:&LocalPool,s:&'a Journal,op:OperationId,id:FileId,rev:u64)->io::Result<Choice<'a>>{
    p.require_business_op(op)?;let req=types::request(p.id(),fs::local_uid(),op,id,rev)?;
    if let Some(d)=s.decided(op){
        let original=d.delta()?;if original.request!=req{return Err(invalid("same operation does not have this original INCR request"));}
        if !s.incrementing.as_ref().is_some_and(|a|a.original.op==op){return Ok(Choice::Done(original.receipt()));}
    }
    if s.moving.is_some()||s.allocated.is_some()||s.saving.is_some()||s.committing.is_some()||s.editing.is_some()||
        s.incrementing.as_ref().is_some_and(|a|a.original.op!=op||a.request!=req){return Err(invalid("another original FILE operation needs completion"));}
    if s.delta(id).is_some_and(|d|d.original.op!=op){return Err(invalid("this formal producer requires the original FULL as the current parent"));}
    let c=s.file(id).ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no original FILE"))?;
    let initial=s.saved(id).ok_or_else(||invalid("original FILE has no first saved revision"))?;
    let first=s.head(id).ok_or_else(||invalid("INCR requires a real published formal parent"))?;
    let work=s.submission_input(id,op)?;let parent=work.parent();
    // Until additional immutable packs, historical reads and custody are a
    // single complete feature, no producer admits an incremental baseline.
    if parent!=journal::cycle::Parent::full(first){return Err(invalid("this producer only publishes the first formal INCR"));}
    let edge=work.last().ok_or_else(||invalid("no changed working revision to commit"))?;
    if edge.original.revision()!=rev{return Err(invalid("requested working revision is not the exact current original"));}
    let map=Map::derive(parent.length,work.rows())?;
    if let Some(a)=&s.incrementing{a.original.require(s.placement(id)?,parent,edge,&map)?;}else{s.room_for(wire::MAX_FRAME)?;}
    Ok(Choice::Active(SourceCut{c,initial,work,edge,map,req}))
}
enum Source{Done(FormalNode),Input(Input)}
fn source(system:&Path,op:OperationId,id:FileId,rev:u64)->io::Result<Source>{
    let cut=formal::ParentCut::open(system,Access::Incoming)?;let(p,s)=cut.originals();
    let x=match choice(p,s,op,id,rev)?{Choice::Done(r)=>return Ok(Source::Done(r)),Choice::Active(x)=>x};
    let selected=cut.file(id)?;let site=selected.placement();let v=p.ordinary(site.volume())?;
    let(dir,actual)=selected.on_volume(&v,x.work.parent())?;let parent=std::sync::Arc::new(actual);
    let a=s.incrementing.as_ref();let o=match a{
        Some(a)=>a.original.clone(),None=>Original::select(op,fs::local_uid(),site,x.work.parent(),x.edge,&x.map)?,
    };
    if a.is_none()&&!s.free_content_ids(o.node,o.version,o.storage){return Err(invalid("new formal identity already used"));}
    // The previous INCR can be a parent. It is never the published result
    // of a new operation merely because it is the latest confirmed child.
    let decided=s.decided(op).map(|f|f.delta()).transpose()?;
    let sealed=s.pending.as_ref().map(|seal|seal.frame.delta()).transpose()?.or(decided);
    let (working,published)=if let Some(d)=decided{
        let(confirmed,pin)=selected.on_volume(&v,journal::cycle::Parent::incremental(d))?;
        if confirmed.entity!=dir.entity||!confirmed.same_mount(&dir)?{return Err(invalid("published INCR changed FILE during recovery"));}
        pin.require(&dir)?;(None,Some(pin))
    }else{
        let set=Operands::capture(&dir,site,x.work.parent(),x.work.rows())?;
        (Some(Composed::new(dir.duplicate()?,parent.clone(),set)?),None)
    };
    let retained=x.work.rows().cloned().collect();v.require_current()?;
    Ok(Source::Input(Input::open(dir,o,x.map,working,published,parent,a,sealed,retained)?))
}
fn commit_inner(system:&Path,op:OperationId,id:FileId,rev:u64,mut cut:impl FnMut(Cut)->io::Result<()>)->io::Result<FormalNode>{
    let verified=match source(system,op,id,rev)?{Source::Done(r)=>return Ok(r),Source::Input(i)=>i.verify()?};
    writer(system,op,id,rev,verified,&mut cut)
}
fn writer(system:&Path,op:OperationId,id:FileId,rev:u64,verified:Verified,cut:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<FormalNode>{
    let (p,_,s)=LocalPool::with_snapshot(system,Access::Write)?;
    let x=match choice(&p,&s,op,id,rev)?{Choice::Done(r)=>return Ok(r),Choice::Active(x)=>x};
    let req=x.req;let site=s.placement(id)?;let v=p.ordinary(site.volume())?;let dir=formal::directory(&v,&s,x.c,x.initial)?;let o=verified.original();
    if o.op!=op||o.uid!=fs::local_uid(){return Err(invalid("different explicit INCR principal or operation"));}
    o.require(site,x.work.parent(),x.edge,&x.map)?;v.require_current()?;verified.require_source(&dir,o)?;
    if let Some(a)=&s.incrementing{if a.original!=*o||a.request!=req{return Err(invalid("different original INCR reservation"));}}
    else{
        if !s.free_content_ids(o.node,o.version,o.storage){return Err(invalid("formal INCR identity collision"));}
        let (seq,prev)=s.frontier();DeltaAllocation::reserve(&p.system,p.id(),o.clone(),seq+1,prev)?;
    }
    drop(x);drop(s);cut(Cut::Reservation)?;
    let (_,mut held)=p.snapshot()?;let a=held.incrementing.take().ok_or_else(||invalid("original INCR allocation disappeared"))?;
    if a.original!=*verified.original()||a.request!=req{return Err(invalid("different original INCR allocation"));}
    p.system.confirm_record_sync(common::NAME,&a.stamp,a.bytes.len())?;p.system.file.sync_all()?;
    let named=verified.install(&dir,&a,||cut(Cut::IncrLinked))?;
    // Moving an O_TMPFILE into the FILE changes metadata on that inode. A new
    // stat is not proof of earlier content. Keep the FILE writer, not the
    // SYSTEM/volume locks, while sealing the exact newly named byte stream.
    drop(held);drop(dir);drop(v);drop(p);
    let bound=verified.verify_named(named)?;
    let (p,_,mut held)=LocalPool::with_snapshot(system,Access::Write)?;
    let x=match choice(&p,&held,op,id,rev)?{Choice::Done(r)=>return Ok(r),Choice::Active(x)=>x};
    let site=held.placement(id)?;let v=p.ordinary(site.volume())?;let dir=formal::directory(&v,&held,x.c,x.initial)?;
    if x.req!=req{return Err(invalid("different original request after named INCR feedback"));}
    verified.original().require(site,x.work.parent(),x.edge,&x.map)?;verified.require_source(&dir,verified.original())?;
    drop(x);
    let now=held.incrementing.take().ok_or_else(||invalid("original INCR reservation disappeared during verification"))?;
    if now.stamp!=a.stamp||now.bytes!=a.bytes{return Err(invalid("original held INCR reservation was replaced"));}
    p.system.confirm_record_sync(common::NAME,&now.stamp,now.bytes.len())?;v.require_current()?;
    let material=bound.feedback(&dir)?;
    if let Some(d)=held.decided(op){
        if held.pending.is_none(){
            let f=d.delta()?;if !a.matches(f)||f.material!=material{return Err(invalid("wrong installed INCR after published completion"));}
            v.require_current()?;verified.retire(&dir,&v,&bound,f,|step|cut(step))?;
            p.system.remove_original_record(common::NAME,&a.stamp,&a.bytes)?;return Ok(f.receipt());
        }
    }
    if held.pending.is_none(){
        let f=Delta::new(p.id(),a.seq,a.previous,a.original.clone(),a.digest,material.clone())?;
        if !a.matches(Decision::read(p.id(),&f.bytes)?.delta()?){return Err(invalid("cannot seal original formal INCR"));}
        held.room_for(f.bytes.len())?;cut(Cut::BeforeIntent)?;journal::Intent::seal(&p.system,&f.bytes)?;
    }
    drop(held);cut(Cut::Intent)?;
    // No new scan of the logical FILE here. These are the exact named FD,
    // prior full-read feedback and frozen input census already held by this
    // writer. A complete journal frame does not itself confirm their names.
    let check=|now:&crate::fs::HeldDir|->io::Result<()>{
        v.require_current()?;verified.require_source(now,verified.original())?;
        if bound.feedback(now)?!=material{return Err(invalid("original named INCR changed before acknowledgement"));}Ok(())
    };
    let seal=publish_sealed(&p,op,req,|d|{
        let f=d.delta()?;
        if !a.matches(f)||f.original!=*verified.original()||f.material!=material{return Err(invalid("sealed INCR differs from the exact verified originals"));}
        check(&dir)
    },|step|{
        cut(step)?;
        if step==Cut::FrameSynced{
            // A known inode census alone cannot account for an unexpected
            // additional name. Use the same physical directory admission as
            // a cold retry of this now-complete, still-unconfirmed intent.
            let(_,frozen)=p.snapshot()?;
            let original=frozen.incrementing.as_ref().ok_or_else(||invalid("original incremental reservation no longer present"))?;
            let sealed=frozen.pending.as_ref().ok_or_else(||invalid("original incremental intent no longer present"))?.frame.delta()?;
            if original.stamp!=a.stamp||original.bytes!=a.bytes||!a.matches(sealed)||sealed.material!=material{
                return Err(invalid("changed incremental reservation or sealed FILE target before confirmation"));}
            let c=frozen.file(id).ok_or_else(||invalid("original FILE disappeared at confirmation"))?;
            let saved=frozen.saved(id).ok_or_else(||invalid("original FILE working origin disappeared at confirmation"))?;
            let now=formal::directory(&v,&frozen,c,saved)?;check(&now)?;
        }
        Ok(())
    })?;
    let f=seal.frame.delta()?;cut(Cut::Decision)?;v.require_current()?;
    if bound.feedback(&dir)?!=f.material{return Err(invalid("confirmed native INCR cannot reopen"));}
    verified.retire(&dir,&v,&bound,f,|step|cut(step))?;cut(Cut::Installation)?;
    p.system.remove_original_record(journal::INTENT,&seal.stamp,&seal.bytes)?;cut(Cut::IntentReleased)?;
    p.system.remove_original_record(common::NAME,&a.stamp,&a.bytes)?;cut(Cut::Release)?;Ok(f.receipt())
}
#[cfg(test)]
#[path="../../tests/unit/second_formal.rs"]
mod tests;
