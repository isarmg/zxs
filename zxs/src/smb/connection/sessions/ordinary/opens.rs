//! The server-wide Persistent half and one authoritative per-session OPEN
//! table. Only the latter owns the held object. Neither half is authority.
use super::{RustSessionWitness,VerifiedMember};
use crate::{smb::{backend::{CloseCompletion,DirectoryCloseError,DirectoryOpen,FileCloseError,FileCloseFault,FileOpen,
    PreparedDisposition,PreparedOrdinaryRename,PreparedRootFsAttributes,PreparedRootFsSize,PreparedRetainedRestore,
    RetainedOpen,RetiredOrdinaryRename,RetiredRetainedRestore,RetiredRootFs,RetiredRootFsSize,RootCloseError,RootOpen,WritableCloseFault,WritableFileOpen},connection::{Fault,Result},
    wire::io_fields::WireOpenId},system::reader::smb::runtime::RustRoot};
use std::{num::{NonZeroU64,NonZeroUsize},sync::{Arc,Mutex,Weak,atomic::{AtomicBool,Ordering}}};

const PER_SESSION:usize=32;
pub(in crate::smb::connection::sessions) const PER_RUN:NonZeroUsize=NonZeroUsize::new(256).unwrap();

struct GlobalIndex {next:u64, slots:Vec<Weak<Persistent>>}
struct Persistent {id:NonZeroU64,published:AtomicBool}

pub(in crate::smb::connection::sessions) struct GlobalOpens(Mutex<GlobalIndex>);
impl GlobalOpens {
    pub(in crate::smb::connection::sessions) fn new()->Arc<Self>{
        Arc::new(Self(Mutex::new(GlobalIndex{next:1,slots:Vec::new()})))
    }
    fn reserve(self:&Arc<Self>)->Result<Ticket>{
        let mut index=self.0.lock().map_err(|_|Fault::SessionState)?;
        index.slots.retain(|w|w.strong_count()!=0);
        if index.slots.len()>=PER_RUN.get(){return Err(Fault::Resources);}
        let id=next_id(index.next)?;
        index.slots.try_reserve(1).map_err(|_|Fault::Resources)?;
        let slot=Arc::new(Persistent{id,published:AtomicBool::new(false)});
        index.slots.push(Arc::downgrade(&slot));
        index.next+=1;
        Ok(Ticket{owner:Arc::downgrade(self),slot})
    }
}
fn next_id(n:u64)->Result<NonZeroU64>{
    NonZeroU64::new(n).filter(|v|v.get()!=u64::MAX).ok_or(Fault::Exhausted)
}
/// Reservation is already counted but invisible to ordinary OPEN lookup.
/// A drop retires that exact number and entry, even if never advertised.
struct Ticket {owner:Weak<GlobalOpens>,slot:Arc<Persistent>}
impl Drop for Ticket {
    fn drop(&mut self){
        self.slot.published.store(false,Ordering::Release);
        if let Some(owner)=self.owner.upgrade(){
            if let Ok(mut index)=owner.0.lock(){
                let here=Arc::downgrade(&self.slot);
                index.slots.retain(|v|!v.ptr_eq(&here));
            }
        }
    }
}

#[must_use]
pub(super) enum Opened {Root(RootOpen),Directory(DirectoryOpen),File(FileOpen),Writer(WritableFileOpen),Retained(RetainedOpen)}
pub(super) enum OpenCloseError {Root(RootCloseError),Directory(DirectoryCloseError),File(FileCloseError),Writer(FileCloseError)}
impl Opened {
    pub(super) fn require_current(&self,tree:&RustRoot,scope:&RustSessionWitness)->std::io::Result<()>{
        match self{
            Self::Root(v)=>v.require_current(tree,scope),
            Self::Directory(v)=>v.require_current(tree,scope),
            Self::File(v)=>v.require_current(tree,scope),
            Self::Writer(v)=>v.require_current(tree,scope),
            Self::Retained(v)=>v.require_current(tree,scope),
        }
    }
    fn close(self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)->std::result::Result<CloseCompletion,(Option<Self>,OpenCloseError)>{
        match self {
            Self::Root(v)=>v.close(tree,member).map_err(|(v,e)|(Some(Self::Root(v)),OpenCloseError::Root(e))),
            Self::Directory(v)=>v.close(tree,member).map_err(|(v,e)|(Some(Self::Directory(v)),OpenCloseError::Directory(e))),
            Self::File(v)=>v.close(tree,member).map_err(|e|match e{
                FileCloseFault::NotStarted(open,e)=>(Some(Self::File(open)),OpenCloseError::File(e)),
                FileCloseFault::Retired(e)=>(None,OpenCloseError::File(e)),
            }),
            Self::Retained(v)=>v.close(tree,member).map_err(|(v,e)|(Some(Self::Retained(v)),OpenCloseError::File(e))),
            Self::Writer(v)=>v.close(tree,member).map_err(|e|match e {
                WritableCloseFault::NotStarted(open,e)=>(Some(Self::Writer(open)),OpenCloseError::Writer(e)),
                WritableCloseFault::Retired(e)=>(None,OpenCloseError::Writer(e)),
            }),
        }
    }
}

#[must_use]
pub(super) struct OpenSlot {object:Opened,ticket:Ticket,volatile:NonZeroU64,tree:u32}
/// Both already-reserved halves and capacity, but no storage authority or
/// published routing entry. Can survive a conditional Core birth attempt;
/// only an actual held object lets the same original owner consume it.
#[must_use]
pub(super) struct OpenSeat {ticket:Ticket,volatile:NonZeroU64,tree:u32}
impl OpenSlot {
    pub(super) fn wire(&self)->WireOpenId{
        WireOpenId{persistent:self.ticket.slot.id.get(),volatile:self.volatile.get()}
    }
    pub(super) fn tree(&self)->u32{self.tree}
    pub(super) fn object(&self)->&Opened{&self.object}
    pub(super) fn object_mut(&mut self)->&mut Opened{&mut self.object}
}
pub(in crate::smb::connection::sessions) struct OpenTable {
    next:u64, pending:Option<Weak<Persistent>>, live:Vec<OpenSlot>,
}
impl OpenTable {
    pub(in crate::smb::connection::sessions) fn new()->Self{
        Self{next:1,pending:None,live:Vec::new()}
    }
    pub(super) fn reserve(&mut self,global:&Arc<GlobalOpens>,tree:u32,object:Opened)->Result<OpenSlot>{
        let seat=self.seat(global,tree)?;
        self.materialize(seat,object)
    }
    pub(super) fn seat(&mut self,global:&Arc<GlobalOpens>,tree:u32)->Result<OpenSeat>{
        if self.has_pending()||tree==0{return Err(Fault::SessionState);}
        if self.live.len()>=PER_SESSION{return Err(Fault::Resources);}
        let volatile=next_id(self.next)?;
        self.live.try_reserve(1).map_err(|_|Fault::Resources)?;
        let seat=OpenSeat{ticket:global.reserve()?,volatile,tree};
        self.pending=Some(Arc::downgrade(&seat.ticket.slot));self.next+=1;
        Ok(seat)
    }
    pub(super) fn materialize(&self,seat:OpenSeat,object:Opened)->Result<OpenSlot>{
        if !self.waiting_ticket(&seat.ticket){return Err(Fault::SessionState);}
        let OpenSeat{ticket,volatile,tree}=seat;
        Ok(OpenSlot{object,ticket,volatile,tree})
    }
    fn has_pending(&self)->bool{
        // A dropped seat or prepared object left only an expired weak
        // mark. It cannot block a subsequent operation or reuse its IDs.
        self.pending.as_ref().is_some_and(|p|p.strong_count()!=0)
    }
    fn waiting_ticket(&self,ticket:&Ticket)->bool{
        self.pending.as_ref().is_some_and(|p|p.ptr_eq(&Arc::downgrade(&ticket.slot)))&&
            !ticket.slot.published.load(Ordering::Acquire)
    }
    pub(super) fn waiting(&self,slot:&OpenSlot)->bool{
        self.waiting_ticket(&slot.ticket)
    }
    pub(super) fn publish(&mut self,slot:OpenSlot)->Result<()>{
        if !self.waiting(&slot){return Err(Fault::SessionState);}
        self.pending=None;
        self.live.push(slot); // exact capacity was reserved on this owner
        self.live.last().ok_or(Fault::SessionState)?.ticket.slot.published.store(true,Ordering::Release);
        Ok(())
    }
    pub(super) fn retire_root_probe(&mut self,slot:OpenSlot,tree:&RustRoot,prepared:PreparedRootFsAttributes,
        close:&VerifiedMember<'_,'_>)->Result<(CloseCompletion,RetiredRootFs)>{
        self.retire_probe(slot,|open|prepared.finish_related_close(open,tree,close))
    }
    pub(super) fn retire_root_size_probe(&mut self,slot:OpenSlot,tree:&RustRoot,prepared:PreparedRootFsSize,
        close:&VerifiedMember<'_,'_>)->Result<(CloseCompletion,RetiredRootFsSize)>{
        self.retire_probe(slot,|open|prepared.finish_related_close(open,tree,close))
    }
    pub(super) fn retire_root_pair(&mut self,slot:OpenSlot,tree:&RustRoot,close:&VerifiedMember<'_,'_>)
        ->Result<CloseCompletion>{
        self.retire_probe(slot,|open|open.close(tree,close).map_err(|_|std::io::ErrorKind::PermissionDenied.into()))
    }
    fn retire_probe<T>(&mut self,slot:OpenSlot,finish:impl FnOnce(RootOpen)->std::io::Result<T>)->Result<T>{
        if !self.waiting(&slot){return Err(Fault::SessionState);}
        self.pending=None;
        let OpenSlot{object,ticket,..}=slot;
        let Opened::Root(open)=object else{return Err(Fault::SessionState);};
        let result=finish(open).map_err(|_|Fault::SessionState);
        // Actual hold/close first; then retire the never-published number.
        drop(ticket);result
    }
    pub(super) fn retire_retained_restore(&mut self,slot:OpenSlot,tree:&RustRoot,receipt:PreparedRetainedRestore,
        close:&VerifiedMember<'_,'_>)->Result<(CloseCompletion,RetiredRetainedRestore)>{
        if !self.waiting(&slot){return Err(Fault::SessionState);}
        self.pending=None;
        let OpenSlot{object,ticket,..}=slot;
        let Opened::Retained(open)=object else{return Err(Fault::SessionState);};
        let result=receipt.finish_related_close(open,tree,close).map_err(|_|Fault::SessionState);
        drop(ticket);result
    }
    pub(super) fn retire_ordinary_rename(&mut self,slot:OpenSlot,tree:&RustRoot,receipt:PreparedOrdinaryRename,
        close:&VerifiedMember<'_,'_>)->Result<(CloseCompletion,RetiredOrdinaryRename)>{
        if !self.waiting(&slot){return Err(Fault::SessionState);}
        self.pending=None;
        let OpenSlot{object,ticket,..}=slot;
        let Opened::File(open)=object else{return Err(Fault::SessionState);};
        // Rename already confirmed. Failed eligibility/retirement is
        // terminal; it cannot turn into a definite no-rename response.
        let result=receipt.finish_related_close(open,tree,close).map_err(|_|Fault::WriteOutcomeUnknown);
        drop(ticket);result
    }
    pub(super) fn close_marked_file(&mut self,slot:OpenSlot,tree:&RustRoot,mark:PreparedDisposition,close:&VerifiedMember<'_,'_>)
        ->Result<std::result::Result<CloseCompletion,FileCloseError>>{
        if !self.waiting(&slot){return Err(Fault::SessionState);}
        self.pending=None;
        let OpenSlot{object,ticket,..}=slot;
        let Opened::File(file)=object else{return Err(Fault::SessionState);};
        mark.require_file(&file,tree).map_err(|_|Fault::SessionState)?;
        let result=match file.close(tree,close){
            Ok(actual)=>Ok(Ok(actual)),
            Err(FileCloseFault::Retired(e))=>Ok(Err(e)),
            // This reply cannot advertise a staged mark on an OPEN whose
            // next paid member lost its authority. Abort that original
            // OPEN; dropping alone cannot arm a mark or move its FILE.
            Err(FileCloseFault::NotStarted(open,_))=>{drop(open);Err(Fault::SessionState)},
        };
        drop(ticket);result
    }
    fn position(&self,wire:WireOpenId)->Option<usize>{
        let i=self.live.iter().position(|v|v.volatile.get()==wire.volatile)?;
        let slot=&self.live[i];
        (slot.wire()==wire&&slot.ticket.slot.published.load(Ordering::Acquire)).then_some(i)
    }
    pub(super) fn find(&self,wire:WireOpenId)->Option<&OpenSlot>{
        self.live.get(self.position(wire)?)
    }
    pub(super) fn find_mut(&mut self,wire:WireOpenId)->Option<&mut OpenSlot>{
        let i=self.position(wire)?;self.live.get_mut(i)
    }
    pub(super) fn retire_tree(&mut self,tree:u32)->Result<()>{
        if self.has_pending(){return Err(Fault::SessionState);}
        // Discard only this tree's real holds/votes. No implicit CLOSE
        // command, FLUSH or delete-to-recycle transaction is authorized.
        self.live.retain(|open|open.tree!=tree);Ok(())
    }
    pub(super) fn close(&mut self,wire:WireOpenId,tid:u32,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Option<std::result::Result<CloseCompletion,OpenCloseError>>{
        let i=self.position(wire)?;
        if self.live[i].tree!=tid{return None;}
        let OpenSlot{object,ticket,volatile,tree:tid}=self.live.swap_remove(i);
        Some(match object.close(tree,member){
            Ok(done)=>{drop(ticket);Ok(done)},
            Err((Some(object),why))=>{
                // Only a refusal before the original close/flush starts.
                // Same original object/number; no allocation or lookup.
                self.live.push(OpenSlot{object,ticket,volatile,tree:tid});Err(why)
            },
            Err((None,why))=>{drop(ticket);Err(why)}, // started CLOSE never recreates an OPEN
        })
    }
}

#[cfg(test)]
#[path="../../../../../tests/smb/open_ids.rs"]
mod tests;
