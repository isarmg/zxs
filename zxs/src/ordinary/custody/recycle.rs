//! The local owner's dedicated lifecycle commands. The caller selects an
//! original FILE, never an arbitrary recycle root, path or physical inode.
//! Looking up an original outcome only opens SYSTEM, even after a later cycle.
use crate::ordinary::custody::transfer::ParentChoice;
use super::{command::{MoveReceipt,MoveStatus,Request},lifecycle,purpose::{Call,Purpose},transfer::Moved};
use crate::{DirectoryParent as Parent,EntryId,FileId,HistoryId,OperationId,VolumeId,fs::{self,invalid},ordinary::journal::Journal,
    pool::lease::{Access,LocalPool}};
use std::{io,path::Path};
mod page;
pub use page::{list_retained,RecycleCursor,RecycleFilter,RecyclePage,RetainedEntry};

/// Precisely one published deletion. A name, an operation ID alone, or a
/// later deletion of the same FILE cannot authorize a restoration.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub struct DeletionKey{pub file:FileId,pub operation:OperationId,pub decision:[u8;32]}
fn read_digest(s:&str)->io::Result<[u8;32]>{
    if s.len()!=64{return Err(invalid("expected a 64-digit original digest"));}
    let digit=|b|match b{b'0'..=b'9'=>Ok(b-b'0'),b'a'..=b'f'=>Ok(b-b'a'+10),_=>Err(invalid("noncanonical digest"))};
    let mut d=[0;32];for(x,b)in d.iter_mut().zip(s.as_bytes().as_chunks::<2>().0){*x=(digit(b[0])?<<4)|digit(b[1])?;}Ok(d)
}
impl DeletionKey{
    pub fn from_hex(file:FileId,operation:OperationId,digest:&str)->io::Result<Self>{
        Ok(Self{file,operation,decision:read_digest(digest)?})
    }
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum RestoreTarget{
    /// The established root-only commands never alias an original child address.
    Original,Volume(VolumeId),
    /// Resolve an address from the exact original deletion, or ask for a
    /// specific ordinary parent. Equal complete addresses have one request.
    OriginalParent,At(Parent),
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum RecycleAction{Delete{file:FileId,from:VolumeId},DeleteAt{file:FileId,source:Parent},Restore{deleted:DeletionKey,target:RestoreTarget}}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum RecycleKind{Delete,Restore}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub struct DeletionTime{pub unix_seconds:u64,pub nanos:u32}
#[derive(Clone,Debug,Eq,PartialEq)]
pub struct RecycleReceipt{
    pub kind:RecycleKind,pub operation:OperationId,pub file:FileId,pub entry:EntryId,pub history:HistoryId,pub name:String,
    pub from:VolumeId,pub to:VolumeId,pub source_parent:Option<Parent>,pub destination_parent:Option<Parent>,pub deleted_from:Parent,pub source_epoch:u64,pub destination_epoch:u64,
    pub delete_operation:OperationId,pub deleted_at:DeletionTime,pub deletion:Option<DeletionKey>,
    pub decision_sequence:Option<u64>,pub decision:Option<[u8;32]>,pub status:MoveStatus,
}
pub(super) fn deleted<'a>(s:&'a Journal,key:DeletionKey)->io::Result<&'a Moved>{
    let m=s.decided(key.operation).ok_or_else(||invalid("no such published original deletion"))?.moved()?;
    if m.original.file!=key.file||m.original.op!=key.operation||m.digest!=key.decision||
        m.original.uid!=fs::local_uid()||!matches!(m.original.purpose,Purpose::Delete{..}){
        return Err(invalid("wrong deletion original, FILE, decision digest or subject"));
    }Ok(m)
}
impl RecycleAction{
    fn resolve(self,p:&LocalPool,s:&Journal,op:OperationId,epoch:u64)->io::Result<Request>{
        p.require_business_op(op)?;
        let r=match self{
            Self::Delete{file,from}|Self::DeleteAt{file,source:Parent::Root(from)}=>
                Request{parents:ParentChoice::Roots,op,id:file,from,to:p.recycle_for(from)?,epoch,call:Call::Delete},
            Self::DeleteAt{file,source}=>{let from=source.volume();Request{parents:ParentChoice::Delete{source},
                op,id:file,from,to:p.recycle_for(from)?,epoch,call:Call::Delete}}
            Self::Restore{deleted:key,target}=>{
                let m=deleted(s,key)?;
                let deleted_from=m.original.ends()?.0.ordinary().ok_or_else(||invalid("deletion has no original ordinary source"))?;
                let current=match target{
                    RestoreTarget::Original|RestoreTarget::Volume(_) if !matches!(deleted_from,Parent::Root(_))=>
                        return Err(invalid("a root-only restore does not identify this child deletion")),
                    RestoreTarget::Original|RestoreTarget::OriginalParent=>deleted_from,
                    RestoreTarget::Volume(v)=>Parent::Root(v),RestoreTarget::At(p)=>p,
                };
                let to=current.volume();let from=p.recycle_for(to)?;
                if m.original.to!=from{return Err(invalid("original deletion belongs to another recycle registration"));}
                let parents=match(deleted_from,current){(Parent::Root(_),Parent::Root(_))=>ParentChoice::Roots,
                    _=>ParentChoice::Restore{deleted_from,target:current}};
                Request{parents,op,id:key.file,from,to,epoch,call:Call::Restore{deleted_by:key.operation,decision:key.decision}}
            }

        };let _=r.digest(p)?;Ok(r)
    }
}
fn absolute_time(purpose:Purpose)->io::Result<DeletionTime>{
    if let Purpose::Delete{at,..}=purpose{let(unix_seconds,nanos)=at.parts();Ok(DeletionTime{unix_seconds,nanos})}
    else{Err(invalid("not an original deletion timestamp"))}
}
pub(super) fn receipt(p:&LocalPool,s:&Journal,r:Request)->io::Result<Option<RecycleReceipt>>{
    if r.call==Call::Move{return Err(invalid("ordinary MOVE is not a recycle transaction"));}
    let Some(base)=r.observe(p,s)? else{
        if s.allocated.as_ref().is_some_and(|a|a.op==r.op)||s.saving.as_ref().is_some_and(|a|a.original.op==r.op)||
            s.committing.as_ref().is_some_and(|a|a.original.op==r.op)||s.editing.as_ref().is_some_and(|a|a.original.op==r.op)||
            s.incrementing.as_ref().is_some_and(|a|a.original.op==r.op){return Err(invalid("operation is reserved for a different FILE command"));}
        return Ok(None)
    };
    let done=s.decided(r.op).map(|d|d.moved()).transpose()?;
    let o=done.map(|m|&m.original).or_else(||s.moving.as_ref().filter(|a|a.original.op==r.op).map(|a|&a.original))
        .ok_or_else(||invalid("missing original custody receipt"))?;
    if o.uid!=fs::local_uid()||o.purpose.call()!=r.call||o.request_digest(p.id())?!=r.digest(p)?{
        return Err(invalid("different original recycle command"));}
    let(kind,delete_op,delete_row,deletion)=match r.call{
        Call::Delete=>(RecycleKind::Delete,r.op,o,done.map(|m|DeletionKey{file:r.id,operation:r.op,decision:m.digest})),
        Call::Restore{deleted_by,decision}=>{let key=DeletionKey{file:r.id,operation:deleted_by,decision};
            (RecycleKind::Restore,deleted_by,&deleted(s,key)?.original,Some(key))},
        Call::Move=>return Err(invalid("not a dedicated lifecycle receipt")),
    };
    let deleted_at=absolute_time(delete_row.purpose)?;
    let deleted_from=delete_row.ends()?.0.ordinary().ok_or_else(||invalid("original deletion does not name an ordinary parent"))?;
    let MoveReceipt{operation,file,entry,history,name,from,to,source_parent,destination_parent,source_epoch,destination_epoch,decision_sequence,status}=base;
    Ok(Some(RecycleReceipt{kind,operation,file,entry,history,name,from,to,source_parent,destination_parent,deleted_from,source_epoch,destination_epoch,delete_operation:delete_op,
        deleted_at,deletion,decision_sequence,decision:done.map(|m|m.digest),status}))
}
/// Report the exact command and original phase. Never inspect online data or
/// claim that an old, now superseded deletion is still restorable.
pub fn recycle_receipt(system:&Path,op:OperationId,epoch:u64,action:RecycleAction)->io::Result<Option<RecycleReceipt>>{
    let(p,_,s)=LocalPool::with_snapshot(system,Access::Read)?;receipt(&p,&s,action.resolve(&p,&s,op,epoch)?)
}
pub fn apply_recycle(system:&Path,op:OperationId,epoch:u64,action:RecycleAction)->io::Result<RecycleReceipt>{
    apply(system,op,epoch,action,&mut |_|Ok(()))
}
pub(super) fn apply(system:&Path,op:OperationId,epoch:u64,action:RecycleAction,hook:&mut impl FnMut(crate::ordinary::Cut)->io::Result<()>)->io::Result<RecycleReceipt>{
    let(r,invocation)={let(p,_,s)=LocalPool::with_snapshot(system,Access::Read)?;
        let r=action.resolve(&p,&s,op,epoch)?;
        if let Some(old)=receipt(&p,&s,r)?{if old.status==MoveStatus::OriginalPublished{return Ok(old);}}
        (r,lifecycle::Invocation::new(&p,r)?)
    };
    let done=lifecycle::perform_for(system,&invocation,hook)?;
    let(p,_,s)=LocalPool::with_snapshot(system,Access::Read)?;
    invocation.require(&p)?;if action.resolve(&p,&s,op,epoch)?!=r{return Err(invalid("different original SYSTEM or command after custody"));}
    let mut original=receipt(&p,&s,r)?.ok_or_else(||invalid("missing original lifecycle outcome"))?;
    if original.status!=MoveStatus::OriginalPublished||original.decision_sequence!=done.decision_sequence||
        !matches!(done.status,MoveStatus::CompleteCheckedNow|MoveStatus::OriginalPublished){
        return Err(invalid("lifecycle completed at a different original phase"));}
    original.status=done.status;Ok(original)
}
