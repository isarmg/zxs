//! Temporary, private byte staging for an original SMB writer. A WRITE
//! accepts only its bytes into this handle. FLUSH submits one effective
//! interval map to the existing work writer. No operation creates a Node.
use super::{first_work::Current,work_change::Writer};
use crate::{OperationId,fs::{HeldDir,invalid,local_uid},ordinary::working_round::{Span,Used}};
use std::{
    cell::Cell,fs::{File,Permissions},io::{self,Write},rc::Rc,os::{fd::{AsRawFd,FromRawFd},unix::fs::{FileExt,MetadataExt,PermissionsExt}},
    sync::atomic::{AtomicU64,Ordering},
};

const SPANS:usize=131_072; // bounds metadata in each accepted direct map, not history
const BLOCK:u64=64*1024;
const RAM_HANDLE:u64=32*1024*1024;
const RAM_TOTAL:u64=128*1024*1024;
static RAM:AtomicU64=AtomicU64::new(0);
unsafe extern "C"{fn zxs_smb_scratch_fd(directory:i32,kind:*mut u8)->i32;}
fn full()->io::Error{io::Error::new(io::ErrorKind::StorageFull,"anonymous SMB staging space unavailable")}

struct Charge{amount:u64,writer:Rc<Cell<u64>>}
impl Charge{
    fn reserve_to(&mut self,end:u64)->io::Result<()>{
        let extra=end.saturating_sub(self.amount);
        if extra==0{return Ok(());}
        let writer=self.writer.get().checked_add(extra).filter(|v|*v<=RAM_HANDLE).ok_or_else(full)?;
        RAM.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|n.checked_add(extra).filter(|v|*v<=RAM_TOTAL))
            .map_err(|_|full())?;
        self.writer.set(writer);self.amount+=extra;Ok(())
    }
    fn release_above(&mut self,end:u64){
        let released=self.amount.saturating_sub(end);
        if released!=0{
            self.amount-=released;
            self.writer.set(self.writer.get()-released);
            RAM.fetch_sub(released,Ordering::AcqRel);
        }
    }
}
impl Drop for Charge{fn drop(&mut self){self.release_above(0);}}
struct Spool{file:File,charge:Charge,ram:bool,len:u64,poisoned:bool} // close FD before releasing RAM budget
impl Spool {
    fn new(dir:&HeldDir,writer:Rc<Cell<u64>>)->io::Result<Self>{
        let mut kind=0;
        // The descriptor is an owned unlinked regular file, never the
        // caller's path/FD or a persistent FILE member.
        let n=unsafe{zxs_smb_scratch_fd(dir.file.as_raw_fd(),&mut kind)};
        if n<0{return Err(io::Error::last_os_error());}
        let file=unsafe{File::from_raw_fd(n)};
        file.set_permissions(Permissions::from_mode(0o600))?;
        let st=file.metadata()?;
        if !matches!(kind,0|1|3)||!st.is_file()||st.len()!=0||st.nlink()!=0||st.uid()!=local_uid()||
           st.mode()&0o7777!=0o600||kind&2==0&&!dir.shares_file_mount(&file)?{
            return Err(invalid("unsafe or substituted private SMB staging file"));
        }
        Ok(Self{file,charge:Charge{amount:0,writer},ram:kind&1!=0,len:0,poisoned:false})
    }
    fn quota(&mut self,end:u64)->io::Result<()>{
        if self.ram{self.charge.reserve_to(end)?;}
        Ok(())
    }
    fn append(&mut self,bytes:&[u8])->io::Result<()>{
        if self.poisoned{return Err(invalid("failed private staging must be discarded"));}
        let end=self.len.checked_add(bytes.len() as u64).filter(|v|*v<=i64::MAX as u64).ok_or_else(full)?;
        self.quota(end)?;
        if let Err(e)=self.file.write_all_at(bytes,self.len){
            // These bytes were never selected or acknowledged. Keep any
            // prior selected extent and do not waive reserve charging.
            if self.file.set_len(self.len).is_err(){self.poisoned=true;}
            return Err(e);
        }
        self.len=end;Ok(())
    }
    fn discard_tail(&mut self,old:u64){
        if self.len!=old{
            if self.file.set_len(old).is_ok(){self.len=old;}else{self.poisoned=true;}
        }
    }
    /// Only the already selected contiguous prefix is handed to SAVE. The
    /// unselected tail is neither needed by this draft nor visible to readers.
    fn trim_unselected_tail(&mut self,end:u64)->io::Result<()>{
        if self.poisoned||end>self.len{return Err(invalid("invalid staged input prefix"));}
        if end<self.len{
            self.file.set_len(end)?;
            self.len=end;
            self.charge.release_above(end);
        }
        Ok(())
    }
    fn place(&mut self,at:u64,data:&[u8])->io::Result<()>{
        if at==self.len{return self.append(data);}
        if self.poisoned||at.checked_add(data.len() as u64).is_none_or(|end|end>self.len){
            return Err(invalid("invalid unreferenced staging extent"));
        }
        self.file.write_all_at(data,at)
    }
    fn read_at(&self,at:u64,out:&mut[u8])->io::Result<()>{
        if self.poisoned||at.checked_add(out.len() as u64).is_none_or(|n|n>self.len){
            return Err(invalid("staged range is not inside acknowledged bytes"));
        }
        self.file.read_exact_at(out,at)
    }
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Part{lo:u64,hi:u64,stored:u64}
impl Part{fn len(self)->u64{self.hi-self.lo}}
fn append_part(parts:&mut Vec<Part>,next:Part)->io::Result<()>{
    if let Some(last)=parts.last_mut(){
        if last.hi==next.lo&&last.stored.checked_add(last.len())==Some(next.stored){last.hi=next.hi;return Ok(());}
        if last.hi>next.lo{return Err(invalid("non-orderable staged write ranges"));}
    }
    if parts.len()>=SPANS{return Err(invalid("too many effective staged intervals"));}
    parts.try_reserve(1).map_err(|_|full())?;parts.push(next);Ok(())
}
fn selected_bytes(parts:&[Part])->u64{
    // Logical intervals never overlap and all fall inside an i64-bounded EOF.
    parts.iter().copied().map(Part::len).sum()
}
fn physically_unused(parts:&[Part],length:u64,live:u64,needed:u64)->io::Result<Option<u64>>{
    let available=length.checked_sub(live).ok_or_else(||invalid("selected bytes exceed the staging spool"))?;
    if available<needed{return Ok(None);}
    let mut by_offset=Vec::new();by_offset.try_reserve_exact(parts.len()).map_err(|_|full())?;
    for p in parts{by_offset.push((p.stored,p.stored.checked_add(p.len()).ok_or_else(full)?));}
    by_offset.sort_unstable();let mut at=0;
    for (lo,hi) in by_offset{
        if hi>length||lo<at{return Err(invalid("staged physical intervals are not independent"));}
        if lo-at>=needed{return Ok(Some(at));}at=hi;
    }
    Ok((length-at>=needed).then_some(at))
}
fn overlay(parts:&[Part],add:Part)->io::Result<Vec<Part>>{
    let mut result=Vec::new();let mut pending=Some(add);
    for &p in parts{
        if p.hi<=add.lo{append_part(&mut result,p)?;continue;}
        if p.lo<add.lo{append_part(&mut result,Part{hi:add.lo,..p})?;}
        if let Some(q)=pending.take(){append_part(&mut result,q)?;}
        if p.lo>=add.hi{append_part(&mut result,p)?;}
        else if p.hi>add.hi{append_part(&mut result,Part{lo:add.hi,stored:p.stored+(add.hi-p.lo),..p})?;}
    }
    if let Some(q)=pending{append_part(&mut result,q)?;}
    Ok(result)
}

pub(super) struct Scratch(Spool);
impl Scratch{
    pub(super) fn before_birth(dir:&HeldDir)->io::Result<Self>{
        Ok(Self(Spool::new(dir,Rc::new(Cell::new(0)))?))
    }
}

pub(super) struct Staged<'t,'s,'r>{
    // Drop the read witnesses and the scratch before releasing FILE EX.
    base:Current<'t,'s,'r>,
    draft:DraftStage,
    writer:Writer<'t,'s,'r>,
}
impl<'t,'s,'r> Staged<'t,'s,'r>{
    pub(super) fn open(writer:Writer<'t,'s,'r>)->io::Result<Self>{
        let scratch=Scratch::before_birth(writer.work_directory())?;
        Self::prepared(writer,scratch)
    }
    pub(super) fn prepared(writer:Writer<'t,'s,'r>,scratch:Scratch)->io::Result<Self>{
        let base=writer.current_read()?;
        let eof=base.size();
        Ok(Self{base,draft:DraftStage::new(scratch,eof),writer})
    }
    pub(super) fn base(&self)->&Current<'t,'s,'r>{&self.base}
    pub(super) fn eof(&self)->u64{self.draft.eof}
    pub(super) fn transfer_ready(&mut self)->io::Result<()>{
        self.refresh()?;
        self.draft.transfer_ready(self.base.size())
    }
    fn writable(&self,alive:&impl Fn()->io::Result<()>)->io::Result<()>{
        alive()?;self.writer.require_work()} // same original Tree, credential and FILE
    fn refresh(&mut self)->io::Result<()>{
        let writer=&self.writer;
        if let Some(latest)=self.draft.refresh_with(||{
            let latest=writer.current_read()?;
            let op=latest.working_original();let eof=latest.size();
            Ok((latest,op,eof))
        })?{self.base=latest;}
        Ok(())
    }
    fn changing(&mut self,alive:&impl Fn()->io::Result<()>)->io::Result<()>{
        self.writable(alive)?;self.refresh()?;
        if self.draft.flush_op.is_some(){return Err(io::Error::new(io::ErrorKind::WouldBlock,"repeat the outstanding original FLUSH before new WRITE"));}
        Ok(())
    }
    pub(super) fn write(&mut self,offset:u64,data:&[u8],alive:&impl Fn()->io::Result<()>)->io::Result<()>{
        self.changing(alive)?;
        let writer=&self.writer;
        self.draft.write(offset,data,||{alive()?;writer.require_work()},||writer.staging_free())
    }
    pub(super) fn resize(&mut self,eof:u64,alive:&impl Fn()->io::Result<()>)->io::Result<()>{
        self.changing(alive)?;
        let writer=&self.writer;
        self.draft.resize(eof,||{alive()?;writer.require_work()})
    }
    /// An owner can read its own staged bytes; ordinary read-only FSPs still
    /// own their fixed selection and never go through this staging object.
    pub(super) fn read_window<T>(&mut self,offset:u64,count:u32,alive:&impl Fn()->io::Result<()>,consume:impl FnOnce(&[u8])->io::Result<T>)->io::Result<T>{
        self.writable(alive)?;self.refresh()?;
        let (bytes,origins)=self.draft.read_buffer(offset,count,|at,len,out|self.base.range_evidence(at,len,out))?;
        self.base.finish_sources(&origins)?;self.writable(alive)?;
        let value=consume(&bytes)?;
        self.base.finish_sources(&origins)?;self.writable(alive)?;Ok(value)
    }
    pub(super) fn flush(&mut self,alive:&impl Fn()->io::Result<()>)->io::Result<()>{
        self.writable(alive)?;
        if self.draft.refresh_op.is_some(){let _=self.refresh();return Ok(());} // already confirmed
        let same=&self.writer;
        let baseline=self.base.working_original();
        let saved=self.draft.flush(self.base.size(),same.work_directory(),||same.staging_free(),|batch|
            same.save_packed_guarded(batch.op,baseline,batch.retained,batch.eof,batch.spans,batch.input,alive).map(drop))?;
        if !saved{return Ok(());}
        let _=self.refresh();alive()
    }
}

// Exactly one unlinked spool, one effective interval map and one sticky
// original FLUSH. This object owns bytes and evidence, not authority.
// The actual native wrapper checks its original FILE before and after.
struct DraftStage{
    spool:Spool,
    eof:u64,retained:u64,parts:Vec<Part>,
    live_bytes:u64, // exact selected byte count; holes in the logical file do not count
    flush_op:Option<OperationId>,refresh_op:Option<OperationId>,
}
struct FlushInput<'a>{op:OperationId,retained:u64,eof:u64,spans:&'a[Span],input:&'a File}
impl DraftStage{
    fn new(Scratch(spool):Scratch,eof:u64)->Self{
        Self{spool,eof,retained:eof,parts:Vec::new(),live_bytes:0,flush_op:None,refresh_op:None}
    }
    fn refresh_with<T>(&mut self,select:impl FnOnce()->io::Result<(T,Option<OperationId>,u64)>)->io::Result<Option<T>>{
        if let Some(original)=self.refresh_op{
            let (latest,op,eof)=select()?;
            if op!=Some(original)||eof!=self.eof{return Err(invalid("confirmed work no longer matches its original FLUSH"));}
            self.refresh_op=None;
            Ok(Some(latest))
        }else{Ok(None)}
    }
    fn transfer_ready(&self,base_size:u64)->io::Result<()>{
        if self.eof!=base_size||self.retained!=self.eof||!self.parts.is_empty()||
           self.flush_op.is_some()||self.spool.poisoned{
            return Err(io::Error::new(io::ErrorKind::WouldBlock,"writer still holds unconfirmed work"));
        }
        Ok(())
    }
    fn write(&mut self,offset:u64,data:&[u8],check:impl Fn()->io::Result<()>,free:impl Fn()->io::Result<u64>)->io::Result<()>{
        let hi=offset.checked_add(data.len() as u64).filter(|v|*v<=i64::MAX as u64).ok_or_else(||invalid("SMB WRITE range overflow"))?;
        if data.is_empty(){return check();}
        let old=self.spool.len;
        let stored=physically_unused(&self.parts,old,self.live_bytes,data.len() as u64)?.unwrap_or(old);
        if stored==old && !self.spool.ram && free()?<data.len() as u64{return Err(full());}
        let p=Part{lo:offset,hi,stored};
        if self.parts.last().is_none_or(|last|last.hi<=offset){
            let live=self.live_bytes.checked_add(p.len()).ok_or_else(full)?;
            let merge=self.parts.last().is_some_and(|last|last.hi==offset&&last.stored+last.len()==p.stored);
            if !merge{
                if self.parts.len()>=SPANS{return Err(invalid("too many effective staged intervals"));}
                self.parts.try_reserve(1).map_err(|_|full())?;
            }
            self.spool.place(stored,data)?;
            if let Err(e)=check(){self.spool.discard_tail(old);return Err(e);}
            if merge{self.parts.last_mut().expect("last staged interval").hi=hi;}
            else{self.parts.push(p);}
            self.live_bytes=live;
        }else{
            let next=overlay(&self.parts,p)?;
            let live=selected_bytes(&next);
            self.spool.place(stored,data)?;
            if let Err(e)=check(){self.spool.discard_tail(old);return Err(e);}
            self.parts=next;
            self.live_bytes=live;
        }
        self.eof=self.eof.max(hi);Ok(())
    }
    fn resize(&mut self,eof:u64,check:impl FnOnce()->io::Result<()>)->io::Result<()>{
        if eof>i64::MAX as u64{return Err(invalid("SMB EOF overflow"));}
        check()?;
        let keep=self.parts.partition_point(|p|p.lo<eof);
        let removed=selected_bytes(&self.parts[keep..])+
            self.parts[..keep].last().map_or(0,|last|last.hi.saturating_sub(eof));
        let live=self.live_bytes.checked_sub(removed).ok_or_else(||invalid("selected bytes do not cover the discarded draft intervals"))?;
        self.parts.truncate(keep);
        if let Some(last)=self.parts.last_mut(){last.hi=last.hi.min(eof);}
        self.live_bytes=live;
        self.retained=self.retained.min(eof);self.eof=eof;Ok(())
    }
    fn gap(&self,out:&mut Vec<u8>,used:&mut Used,lo:u64,hi:u64,
        base:&mut impl FnMut(u64,u64,&mut Vec<u8>)->io::Result<Used>)->io::Result<()>{
        let limit=hi.min(self.retained);
        if lo<limit{used.merge(base(lo,limit-lo,out)?)?;}
        let mut cursor=lo.max(self.retained);let zeros=[0u8;BLOCK as usize];
        while cursor<hi{let n=(hi-cursor).min(BLOCK);out.write_all(&zeros[..n as usize])?;cursor+=n;}
        Ok(())
    }
    fn read_buffer(&self,offset:u64,count:u32,mut base:impl FnMut(u64,u64,&mut Vec<u8>)->io::Result<Used>)
        ->io::Result<(Vec<u8>,Used)>{
        if count>0xffff00||offset.checked_add(u64::from(count)).is_none_or(|v|v>i64::MAX as u64){
            return Err(invalid("SMB staged READ overflow"));
        }
        let start=offset.min(self.eof);let end=self.eof.min(start+u64::from(count));
        let mut out=Vec::<u8>::new();
        out.try_reserve_exact((end-start) as usize).map_err(|_|full())?;
        let mut cursor=start;let mut origins=Used::default();
        for p in &self.parts[self.parts.partition_point(|p|p.hi<=start)..]{
            let at=p.lo.max(start);if at>=end{break;}
            if cursor<at{self.gap(&mut out,&mut origins,cursor,at,&mut base)?;}
            let stop=p.hi.min(end);let len=(stop-at) as usize;
            let i=out.len();out.resize(i+len,0);self.spool.read_at(p.stored+(at-p.lo),&mut out[i..])?;
            cursor=stop;
        }
        if cursor<end{self.gap(&mut out,&mut origins,cursor,end,&mut base)?;}
        Ok((out,origins))
    }
    fn flush(&mut self,base_size:u64,original:&HeldDir,free:impl Fn()->io::Result<u64>,
        save:impl FnOnce(FlushInput<'_>)->io::Result<()>)->io::Result<bool>{
        if self.parts.is_empty()&&self.eof==base_size&&self.retained==self.eof{return Ok(false);}
        let mut spans=Vec::new();spans.try_reserve_exact(self.parts.len()).map_err(|_|full())?;
        let mut length=0u64;let mut contig=true;
        for p in &self.parts{
            spans.push(Span{at:p.lo,len:p.len()});
            contig&=p.stored==length;length=length.checked_add(p.len()).ok_or_else(full)?;
        }
        let mut packed=None;
        if self.spool.poisoned{return Err(invalid("cannot flush failed private SMB staging"));}
        if length!=self.live_bytes{return Err(invalid("selected byte count does not match the staged input map"));}
        if contig{
            // Reuse the same anonymous spool, including a strict prefix
            // after SET EOF. Never charge RAM for a redundant packed copy.
            self.spool.trim_unselected_tail(length)?;
        }else{
            // Copy only effective changed ranges if needed, never base
            // file contents or an unused staging tail.
            let mut fresh=Spool::new(original,self.spool.charge.writer.clone())?;
            if !fresh.ram&&free()?<length{return Err(full());}
            let mut window=[0u8;BLOCK as usize];
            for p in &self.parts{
                let mut left=p.len();let mut off=p.stored;
                while left>0{
                    let count=left.min(BLOCK) as usize;
                    self.spool.read_at(off,&mut window[..count])?;fresh.append(&window[..count])?;
                    left-=count as u64;off+=count as u64;
                }
            }
            packed=Some(fresh);
        }
        // No fresh descriptor must be allocated after actual confirmation.
        let empty=Spool::new(original,self.spool.charge.writer.clone())?;
        let op=match self.flush_op{Some(op)=>op,None=>OperationId::new()?};
        self.flush_op=Some(op);
        let input=&packed.as_ref().unwrap_or(&self.spool).file;
        save(FlushInput{op,retained:self.retained,eof:self.eof,spans:&spans,input})?;
        // A known committed result cannot become a failed SAVE because a
        // fresh read view is unavailable. Block subsequent data work until
        // the committed Op is positively rebound; never read the old draft.
        drop(packed);self.spool=empty;self.retained=self.eof;
        self.parts.clear();self.live_bytes=0;self.flush_op=None;self.refresh_op=Some(op);
        Ok(true)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/attached/system/smb_stage/tests.rs"]
mod tests;

#[path="smb_rust_staged.rs"]
mod rust;
pub(crate) use rust::{RustStagedName,RustStagedReadProof,RustStagedStandard,RustStagedState,RustStagedWriter};
