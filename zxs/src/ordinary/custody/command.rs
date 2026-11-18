//! First public custody transaction: one clean FILE with its actual original
//! FULL and first INCR. Same logical name, two registered ordinary roots,
//! no new FILE, node or version. Recycle is NOT an ordinary destination.
use super::{purpose::Call,allocation::MoveAllocation,destination::Destination,finish,placed::Reopened,source::Clean,transfer::{self,Origin}};
use crate::{EntryId,FileId,HistoryId,OperationId,VolumeId,fs::{self,invalid},ordinary::{self,Cut,journal::{Intent,Journal},wire},
    pool::lease::{Access,LocalPool}};
use std::{io,path::Path};

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum MoveStatus{Reserved,Prepared,AwaitingDecisionConfirmation,DecidedTargetPending,TargetReopenedSourcePending,OriginalPublished,CompleteCheckedNow}
#[derive(Clone,Debug,Eq,PartialEq)]
pub struct MoveReceipt{pub operation:OperationId,pub file:FileId,pub entry:EntryId,pub history:HistoryId,pub from:VolumeId,pub to:VolumeId,
    pub source_epoch:u64,pub destination_epoch:u64,pub name:String,pub decision_sequence:Option<u64>,pub status:MoveStatus}
fn result(o:&Origin,seq:Option<u64>,status:MoveStatus)->io::Result<MoveReceipt>{Ok(MoveReceipt{operation:o.op,file:o.file,entry:o.entry,history:o.history,
    from:o.from,to:o.to,source_epoch:o.epoch,destination_epoch:o.epoch.checked_add(1).ok_or_else(||invalid("custody epoch exhausted"))?,name:o.name.clone(),decision_sequence:seq,status})}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]pub(super) struct Request{pub call:Call,pub op:OperationId,pub id:FileId,pub from:VolumeId,pub to:VolumeId,pub epoch:u64}
impl Request{
    pub(super) fn digest(self,p:&LocalPool)->io::Result<[u8;32]>{p.require_business_op(self.op)?;
        self.call.request(p.id(),fs::local_uid(),self.op,self.id,self.from,self.to,self.epoch)}
    pub(super) fn observe(self,p:&LocalPool,s:&Journal)->io::Result<Option<MoveReceipt>>{
        let req=self.digest(p)?;
        if let Some(d)=s.decided(self.op){
            let m=d.moved()?;if m.request!=req{return Err(invalid("original MOVE command, participant or expected placement differs"));}
            let status=if let Some(a)=s.moving.as_ref().filter(|a|a.original.op==self.op){
                if !a.matches(m){return Err(invalid("original MOVE reservation differs"));}
                if a.opened(){a.confirm_phase(p)?;MoveStatus::TargetReopenedSourcePending}else{MoveStatus::DecidedTargetPending}
            }else{MoveStatus::OriginalPublished};
            return Ok(Some(result(&m.original,Some(m.seq),status)?));
        }
        let Some(a)=s.moving.as_ref().filter(|a|a.original.op==self.op)else{return Ok(None)};
        if a.request!=req{return Err(invalid("wrong original MOVE request, actor or roots"));}
        let status=match &s.pending{None=>MoveStatus::Reserved,Some(i)if s.tail==i.frame.bytes().len()=>MoveStatus::AwaitingDecisionConfirmation,
            Some(_)=>MoveStatus::Prepared};
        Ok(Some(result(&a.original,None,status)?))
    }
}
pub fn move_receipt(system:&Path,op:OperationId,id:FileId,from:VolumeId,to:VolumeId,epoch:u64)->io::Result<Option<MoveReceipt>>{
    let(p,_,s)=LocalPool::with_snapshot(system,Access::Read)?;Request{call:Call::Move,op,id,from,to,epoch}.observe(&p,&s)
}
pub fn move_file(system:&Path,op:OperationId,id:FileId,from:VolumeId,to:VolumeId,epoch:u64)->io::Result<MoveReceipt>{
    perform(system,Request{call:Call::Move,op,id,from,to,epoch},&mut |_|Ok(()))
}
pub(super) fn perform(system:&Path,r:Request,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<MoveReceipt>{
    if r.call!=Call::Move{return Err(invalid("ordinary MOVE cannot execute a dedicated lifecycle request"));}
    let decided={
        let(p,_,s)=LocalPool::with_snapshot(system,Access::Incoming)?;
        match r.observe(&p,&s)?{
            Some(found) if found.status==MoveStatus::OriginalPublished=>return Ok(found),
            Some(found)=>found.decision_sequence.is_some(),
            None=>{
                s.admit_new_handoff(&p,r.op,r.id,r.from,r.to)?;
                let c=s.file(r.id).ok_or_else(||invalid("missing original FILE"))?;
                let old=p.ordinary(r.from)?;let target=p.ordinary(r.to)?;
                old.require_current()?;ordinary::material::vacant_for_move(&target,&c.create.name)?;
                // Request and ticket are equally wide native IDs. Reject an
                // impossible path before scanning even a large original FILE;
                // reservation independently checks its actual issued ticket.
                if !old.root.same_mount(&target.root)?{transfer::check_private_paths(&old.root,&target.root,r.op)?;}false
            }
        }
    };
    if !decided{
        let src=Clean::capture(system,r.id)?.verify()?;
        let prep=Destination::select(system,&src,r.to)?.for_operation(r.op)?;
        let reserved=prep.under_original_writer(|p,s,_,_|Ok(r.observe(p,s)?.is_some()))?;
        if !reserved{MoveAllocation::reserve(&prep,r.op)?;}
        hook(Cut::Reservation)?;
        let(m,input)=prep.staged_decision(r.op,hook)?;
        let confirmed=input.map(|(input,name)|Reopened::verify(input).map(|r|(r,name))).transpose()?;
        prep.original_writer_after_naming(|p,s,old,new|{
            let req=r.digest(p)?;let a=s.moving.as_ref().ok_or_else(||invalid("original MOVE no longer reserved"))?;
            m.original.require(&src,&prep)?;if req!=m.request||!a.matches(&m){return Err(invalid("different native MOVE reservation"));}
            let current=||{src.require(p,s,old)?;
                if m.original.mode==super::destination::Method::Transfer&&old.root.entry_present(&m.original.quarantine_name())?{
                    return Err(invalid("cannot seal with an occupied original source quarantine"));}
                if let Some((read,name))=&confirmed{read.require(new,name,&m)?;}
                new.root.file.sync_all()?;old.root.file.sync_all()?;new.require_current()?;old.require_current()};
            current()?;p.system.confirm_record_sync(ordinary::allocation::NAME,&a.stamp,a.bytes.len())?;p.system.file.sync_all()?;
            if let Some(i)=&s.pending{
                if i.frame.bytes()!=m.bytes{return Err(invalid("original MOVE sealed different targets"));}
            }else{
                if wire::envelope(p.id(),&m.bytes)?.digest!=m.digest{return Err(invalid("MOVE frame digest mismatch"));}
                s.room_for_effect(m.bytes.len(),m.original.effect())?;hook(Cut::BeforeIntent)?;Intent::seal(&p.system,&m.bytes)?;
            }
            hook(Cut::Intent)?;
            ordinary::publish_sealed(p,r.op,req,|d|{if d.moved()?!=&m{return Err(invalid("different frozen custody decision"));}current()},|c|{
                hook(c)?;
                if c==Cut::FrameSynced{
                    // Complete bytes are not a confirmed placement. An exact
                    // or equivalent competing final name must stop this hot
                    // path at the same boundary as recovery of that frame.
                    // Keep the same original SYSTEM/old/new writer leases.
                    if src.same_mount(&new.root)?!=(m.original.mode==super::destination::Method::Rename){
                        return Err(invalid("original MOVE method changed before confirmation"));
                    }
                    ordinary::material::vacant_for_move(new,&m.original.name)?;
                    current()?;
                }Ok(())
            })?;
            Ok(())
        })?;
        drop(confirmed);drop(prep);drop(src);hook(Cut::Decision)?;
    }
    finish::complete(system,r,hook)?;
    let(p,_,s)=LocalPool::with_snapshot(system,Access::Read)?;
    let mut done=r.observe(&p,&s)?.ok_or_else(||invalid("confirmed MOVE missing"))?;
    if done.status!=MoveStatus::OriginalPublished{return Err(invalid("MOVE completion remains pending"));}
    done.status=MoveStatus::CompleteCheckedNow;Ok(done)
}
