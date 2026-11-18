//! The first manual working save on an identified Unborn FILE is independent
//! of its empty creation and is not a formal FULL or history node.
pub(super) mod allocation;
pub(super) mod material;
pub(super) mod types;

use super::{allocation as creation_allocation,journal::{self,Decision,Journal},material as container,
    wire::Frame,Cut,CreationStatus,FileInfo,publish_sealed};
use self::{allocation::SaveAllocation,material::{Input,InputHandle,PendingReader,Reader,Verified},types::{DraftClaim,DraftReceipt,Original,SaveFrame}};
use crate::{FileId,OperationId,fs::{self,HeldDir,invalid},pool::lease::{Access,FileVolume,LocalPool}};
use std::{io::{self,Read,Write},path::Path};

#[derive(Clone,Debug,Eq,PartialEq)]
pub struct CurrentFile{pub file:FileInfo,pub working:Option<DraftReceipt>,pub head:Option<super::FormalNode>,pub edit:Option<super::SpanReceipt>}
pub(super) enum Body{Working(Reader),Formal(super::formal::physical::HistoryBody),Composed(super::interval::physical::Composed)}
impl Body{
    fn write_to(&self,out:impl Write)->io::Result<()>{
        match self{Self::Working(b)=>b.write_to(out),Self::Formal(b)=>b.write_to(out),Self::Composed(b)=>b.write_range(0,b.size(),out)}
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub struct FirstSaveObservation{
    pub status:CreationStatus,pub file:FileId,pub operation:OperationId,pub body:DraftClaim,pub decision_sequence:Option<u64>,
}

pub(super) fn admit_current_held(state:&Journal,volume:&FileVolume<'_>,c:&Frame)->io::Result<Option<Body>>{
    let saved=state.saved(c.create.file);
    if let Some(f)=state.head(c.create.file){
        let w=saved.ok_or_else(||invalid("formal node has no original confirmed revision"))?;
        let dir=super::formal::directory(volume,state,c,w)?;
        // The confirmed FULL is authoritative even if old-work cleanup is
        // obstructed. Do not use the old draft as the body, or require it for
        // the selected node. Complete-inventory and retirement verify it separately.
        let base=super::formal::physical::HistoryBody::current(&dir,f)?;
        if let Some(e)=state.interval(c.create.file){
            let extent=super::interval::physical::Stored::reopen(&dir,&e.original,&e.material)?;
            return Ok(Some(Body::Composed(super::interval::physical::Composed{base,extent})));
        }
        if let Some(a)=state.editing.as_ref().filter(|a|a.original.file==c.create.file){
            let sealed=state.pending.as_ref().map(|p|p.frame.span()).transpose()?;
            super::interval::physical::inspect(&dir,a,sealed)?;
        }
        return Ok(Some(Body::Formal(base)));
    }
    if state.committing.as_ref().is_some_and(|a|a.original.file==c.create.file){
        let w=saved.ok_or_else(||invalid("formal reservation without working source"))?;
        let dir=super::formal::directory(volume,state,c,w)?;
        return Reader::current(&dir,&w.original,&w.material).map(|b|Some(Body::Working(b)));
    }
    let pending=state.saving.as_ref().filter(|a|a.original.file==c.create.file);
    let sealed=state.pending.as_ref().filter(|p|pending.is_some_and(|a|p.frame.op()==a.original.op));
    let work=saved.map(|f|(f.original.op,true)).or_else(||pending.map(|a|(a.original.op,sealed.is_some())));
    let dir=container::working_directory_held(volume,&c.create,work)?;
    if let Some(f)=saved{return Reader::current(&dir,&f.original,&f.material).map(|b|Some(Body::Working(b)));}
    // The logical old revision is still empty. A matching private spelling
    // that exists, however, is not harmless or attributable merely by name.
    if let Some(a)=pending{
        let actual=PendingReader::open(&dir,a)?;
        if let Some(p)=sealed{
            if actual.as_ref().map(PendingReader::material)!=Some(&p.frame.save()?.material){
                return Err(invalid("prepared working operand differs from the original sealed frame"));
            }
        }
    }
    Ok(None)
}
pub(super) fn capture_current(system:&Path,id:FileId)->io::Result<(CurrentFile,Option<Body>)>{
    let (pool,_,state)=LocalPool::with_snapshot(system,Access::Read)?;
    let c=state.file(id).ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no published original FILE"))?;
    let volume=pool.ordinary(c.create.volume)?;let body=admit_current_held(&state,&volume,c)?;
    volume.root.file.sync_all()?;volume.require_current()?;
    Ok((CurrentFile{file:FileInfo::from_frame(c),working:if state.head(id).is_some(){None}else{state.saved(id).map(SaveFrame::receipt)},head:state.head(id).map(super::formal::types::Commit::receipt),edit:state.interval(id).map(super::interval::types::Saved::receipt)},body))
}
/// Inspect actual original FILE and (if present) its named working record and
/// authenticated chunk head. This is not a whole-content verification scan.
pub fn current_info(system:&Path,id:FileId)->io::Result<CurrentFile>{capture_current(system,id).map(|(i,_)|i)}
/// Output is pinned to one admitted original revision. No general lock stays
/// held across the output sink. A later corrupt chunk yields error, not success;
/// every preceding byte delivered already matched its authenticated chunk.
pub fn read_current_to(system:&Path,id:FileId,mut output:impl Write)->io::Result<CurrentFile>{
    let (info,body)=capture_current(system,id)?;
    if let Some(r)=&body{r.write_to(&mut output)?;}
    finish_output(&body,&mut output)?;Ok(info)
}
pub(super) fn finish_output(body:&Option<Body>,out:&mut impl Write)->io::Result<()>{
    if let Some(Body::Composed(c))=body{c.unchanged()?;out.flush()?;c.unchanged()}else{out.flush()}
}
pub fn first_save_receipt(system:&Path,op:OperationId)->io::Result<Option<FirstSaveObservation>>{
    let (_pool,_,s)=LocalPool::with_snapshot(system,Access::Read)?;
    if let Some(d)=s.decided(op){
        let f=d.save()?;let held=s.pending.as_ref().is_some_and(|p|p.frame.bytes()==d.bytes());
        return Ok(Some(FirstSaveObservation{status:if held{CreationStatus::PublishedInstallationUnconfirmed}else{CreationStatus::Published},
            file:f.original.file,operation:op,body:f.original.claim,decision_sequence:Some(f.seq)}));
    }
    Ok(s.saving.as_ref().filter(|a|a.original.op==op).map(|a|FirstSaveObservation{
        status:match &s.pending{Some(p) if p.frame.bytes().len()==s.tail=>CreationStatus::DecisionAwaitingConfirmation,
            Some(_)=>CreationStatus::Prepared,None=>CreationStatus::Reserved},
        file:a.original.file,operation:op,body:a.original.claim,decision_sequence:None,
    }))
}

enum Choice<'a>{Done(DraftReceipt),Active(&'a Frame,Original,[u8;32])}
fn choice<'a>(pool:&LocalPool,s:&'a Journal,op:OperationId,file:FileId,claim:DraftClaim)->io::Result<Choice<'a>>{
    pool.require_business_op(op)?;
    let uid=fs::local_uid();let request=types::request(pool.id(),uid,op,file,claim)?;
    if let Some(d)=s.decided(op){
        let f=d.save()?;
        if f.request!=request{return Err(invalid("different original SAVE request under this operation"));}
        // A completed historical receipt neither opens today's data path nor consumes input.
        if !s.saving.as_ref().is_some_and(|a|a.original.op==op){return Ok(Choice::Done(f.receipt()));}
    }
    if s.editing.is_some()||s.committing.is_some()||s.allocated.is_some()||s.saving.as_ref().is_some_and(|a|a.original.op!=op||a.request!=request){
        return Err(invalid("different original FILE operation needs completion"));
    }
    let c=s.file(file).ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no original FILE at first SAVE cutoff"))?;
    if s.saved(file).is_some_and(|f|f.original.op!=op){return Err(invalid("the FILE already has a confirmed working revision"));}
    let original=Original::for_create(op,uid,c,claim);
    if s.saving.as_ref().is_some_and(|a|a.original!=original){return Err(invalid("working-source original no longer matches"));}
    if s.saving.is_none(){s.room_for(super::wire::MAX_FRAME)?;}
    Ok(Choice::Active(c,original,request))
}
fn source_dir(v:&FileVolume<'_>,c:&Frame,state:&Journal,o:&Original)->io::Result<HeldDir>{
    let work=state.saving.as_ref().map(|_|(o.op,state.pending.is_some()||state.decided(o.op).is_some()));
    container::working_directory(v,&c.create,work)
}
enum Source{Done(DraftReceipt),Consume(InputHandle),Verify(PendingReader)}
fn source(system:&Path,op:OperationId,file:FileId,claim:DraftClaim)->io::Result<Source>{
    let (p,_,s)=LocalPool::with_snapshot(system,Access::Incoming)?;
    let (c,o)=match choice(&p,&s,op,file,claim)?{Choice::Done(r)=>return Ok(Source::Done(r)),Choice::Active(c,o,_) =>(c,o)};
    let volume=p.ordinary(c.create.volume)?;let dir=source_dir(&volume,c,&s,&o)?;
    if let Some(a)=&s.saving{
        if let Some(r)=PendingReader::open(&dir,a)?{return Ok(Source::Verify(r));}
        if s.pending.is_some()||s.decided(op).is_some(){return Err(invalid("sealed or published original draft body is missing"));}
    }
    Ok(Source::Consume(InputHandle::open(&volume,dir,o)?))
}
enum Work{Input(Input),Verified(Verified)}
enum Outcome{Done(DraftReceipt),Verify(PendingReader)}

pub fn save_first(system:&Path,op:OperationId,file:FileId,claim:DraftClaim,input:impl Read)->io::Result<DraftReceipt>{
    save_inner(system,op,file,claim,input,|_|Ok(()))
}
fn save_inner(system:&Path,op:OperationId,file:FileId,claim:DraftClaim,input:impl Read,mut cut:impl FnMut(Cut)->io::Result<()>)->io::Result<DraftReceipt>{
    DraftClaim::new(claim.bytes,claim.sha256)?;
    let work=match source(system,op,file,claim)?{
        Source::Done(r)=>return Ok(r),
        Source::Consume(handle)=>Work::Input(handle.capture(input)?),
        Source::Verify(reader)=>Work::Verified(reader.verify()?),
    };
    match writer(system,op,file,claim,work,&mut cut)?{
        Outcome::Done(r)=>Ok(r),
        // Between streaming and reacquisition, the same original request may
        // have supplied the same, now-named material. Verify out of lock once.
        Outcome::Verify(reader)=>match writer(system,op,file,claim,Work::Verified(reader.verify()?),&mut cut)?{
            Outcome::Done(r)=>Ok(r),Outcome::Verify(_)=>Err(invalid("verified original SAVE unexpectedly changed")),
        },
    }
}
fn writer(system:&Path,op:OperationId,file:FileId,claim:DraftClaim,work:Work,cut:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<Outcome>{
    let (p,_,s)=LocalPool::with_snapshot(system,Access::Write)?;
    let (c,o,request)=match choice(&p,&s,op,file,claim)?{Choice::Done(r)=>return Ok(Outcome::Done(r)),Choice::Active(c,o,r)=>(c,o,r)};
    let volume=p.ordinary(c.create.volume)?;let dir=source_dir(&volume,c,&s,&o)?;
    if s.saving.is_none(){
        let Work::Input(input)=&work else{return Err(invalid("no original reservation for existing working holder"))};
        input.require(&dir,&o)?;let (seq,previous)=s.frontier();
        SaveAllocation::reserve(&p.system,p.id(),o,seq+1,previous)?;
    }
    drop(s);cut(Cut::Reservation)?;
    let (_,mut held)=p.snapshot()?;
    let a=held.saving.take().ok_or_else(||invalid("original first SAVE reservation disappeared"))?;
    if a.original.op!=op||a.request!=request{return Err(invalid("different original first SAVE reservation"));}
    p.system.confirm_record_sync(creation_allocation::NAME,&a.stamp,a.bytes.len())?;p.system.file.sync_all()?;
    let verified=match work{
        Work::Verified(v)=>v,
        Work::Input(input)=>{
            if let Some(existing)=PendingReader::open(&dir,&a)?{return Ok(Outcome::Verify(existing));}
            if held.pending.is_some()||held.decided(op).is_some(){return Err(invalid("missing previously sealed original working material"));}
            input.link(&dir,&a)?
        }
    };
    let physical=verified.require(&dir,&a)?;
    if let Some(d)=held.decided(op){
        if held.pending.is_none(){
            let f=d.save()?;if f.material!=physical||!a.matches(f){return Err(invalid("confirmed SAVE differs from original working material"));}
            volume.require_current()?;p.system.remove_original_record(creation_allocation::NAME,&a.stamp,&a.bytes)?;
            return Ok(Outcome::Done(f.receipt()));
        }
    }
    if held.pending.is_none(){
        let f=SaveFrame::new(p.id(),a.seq,a.previous,a.original.clone(),a.digest,physical)?;
        if !a.matches(Decision::read(p.id(),&f.bytes)?.save()?){return Err(invalid("cannot seal this original SAVE"));}
        held.room_for(f.bytes.len())?;cut(Cut::BeforeIntent)?;journal::Intent::seal(&p.system,&f.bytes)?;
    }
    drop(held);cut(Cut::Intent)?;
    let sealed=publish_sealed(&p,op,request,|d|{
        let f=d.save()?;volume.require_current()?;
        if !a.matches(f)||verified.require(&dir,&a)?!=f.material{return Err(invalid("different material at first SAVE decision"));}
        Ok(())
    },|phase|cut(phase))?;
    let f=sealed.frame.save()?;cut(Cut::Decision)?;
    volume.require_current()?;
    if verified.require(&dir,&a)?!=f.material{return Err(invalid("published draft cannot reopen its original bytes"));}
    cut(Cut::Installation)?;p.system.remove_original_record(journal::INTENT,&sealed.stamp,&sealed.bytes)?;
    cut(Cut::IntentReleased)?;p.system.remove_original_record(creation_allocation::NAME,&a.stamp,&a.bytes)?;cut(Cut::Release)?;
    Ok(Outcome::Done(f.receipt()))
}

#[cfg(test)]
#[path="../../tests/unit/first_save.rs"]
mod tests;
