//! Original logical command vs its later storage observation. The timestamp
//! is chosen once for a new DELETE reservation, not supplied by the caller,
//! recomputed on replay, or substituted for the exact delete operation.
use super::destination::Method;
use crate::{FileId,OperationId,PoolId,VolumeId,fs::invalid,ordinary::{journal::credits::{Deletion,Effect},wire::{self,In}}};
use std::{ffi::CString,io,time::{SystemTime,UNIX_EPOCH}};

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) enum Call{Move,Delete,Restore{deleted_by:OperationId,decision:[u8;32]}}
impl Call{
    pub(in crate::ordinary) fn request(self,pool:PoolId,uid:u32,op:OperationId,file:FileId,from:VolumeId,to:VolumeId,epoch:u64)->io::Result<[u8;32]>{
        if from==to||epoch==u64::MAX||matches!(self,Self::Move) && epoch!=0 ||
            matches!(self,Self::Restore{deleted_by,..} if deleted_by==op||epoch==0){
            return Err(invalid("invalid original FILE custody request, endpoint or position"));
        }
        let domain:&[u8]=match self{Self::Move=>b"ZXS/FILE/MOVE/REQUEST\0",Self::Delete=>b"ZXS/FILE/DELETE/REQUEST\0",
            Self::Restore{..}=>b"ZXS/FILE/RESTORE/REQUEST\0"};
        let mut b=domain.to_vec();for id in [&pool.0,&op.0,&file.0,&from.0,&to.0]{b.extend_from_slice(id);}
        b.extend_from_slice(&uid.to_be_bytes());b.extend_from_slice(&epoch.to_be_bytes());
        if let Self::Restore{deleted_by,decision}=self{b.extend_from_slice(&deleted_by.0);b.extend_from_slice(&decision);}
        crate::hash::digest(&b)
    }
    pub(super) fn leaves(self,file:FileId,op:OperationId,human:&str)->io::Result<(CString,CString)>{
        wire::human_name(human.as_bytes())?;
        let h=CString::new(human.as_bytes()).map_err(|_|invalid("invalid original FILE leaf"))?;
        let kept=|by:OperationId|CString::new(format!(".zxs-retained-{file}-{by}")).expect("two fixed hex IDs, no NUL");
        Ok(match self{Self::Move=>(h.clone(),h),Self::Delete=>(h,kept(op)),Self::Restore{deleted_by,..}=>(kept(deleted_by),h)})
    }
    pub(in crate::ordinary) fn effect(self,file:FileId,op:OperationId)->Effect{
        match self{Self::Move=>Effect::Ordinary,Self::Delete=>Effect::Retain{file,operation:op},
            Self::Restore{deleted_by,decision}=>Effect::Restore(Deletion{file,operation:deleted_by,decision})}
    }
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct DeletedAt{seconds:u64,nanos:u32}
impl DeletedAt{
    pub(super) fn parts(self)->(u64,u32){(self.seconds,self.nanos)}
    pub(super) fn now()->io::Result<Self>{
        let d=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_|invalid("cannot record a canonical UTC deletion timestamp"))?;
        Self::new(d.as_secs(),d.subsec_nanos())
    }
    pub(super) fn new(seconds:u64,nanos:u32)->io::Result<Self>{
        if nanos>=1_000_000_000{return Err(invalid("noncanonical deletion timestamp"));}
        Ok(Self{seconds,nanos})
    }
}
/// The only permitted difference between a logical DELETE and the same
/// caller's source request is extra *original* physical/clock evidence.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) enum Purpose{
    Ordinary,
    Delete{previous:[u8;32],at:DeletedAt},
    Restore{deleted_by:OperationId,previous:[u8;32]},
}
impl Purpose{
    pub(in crate::ordinary) fn call(self)->Call{
        match self{Self::Ordinary=>Call::Move,Self::Delete{..}=>Call::Delete,
            Self::Restore{deleted_by,previous}=>Call::Restore{deleted_by,decision:previous}}
    }
    pub(super) fn tag(self,method:Method)->io::Result<u8>{
        match(self,method){(Self::Ordinary,Method::Rename)=>Ok(0),(Self::Ordinary,Method::Transfer)=>Ok(1),
            (Self::Delete{..},Method::Transfer)=>Ok(2),(Self::Restore{..},Method::Transfer)=>Ok(3),
            _=>Err(invalid("dedicated recycle transactions are cross-filesystem transfers only"))}
    }
    pub(super) fn method(tag:u8)->io::Result<Method>{
        match tag{0=>Ok(Method::Rename),1..=3=>Ok(Method::Transfer),_=>Err(invalid("unknown custody method/purpose"))}
    }
    pub(super) fn put(self,b:&mut Vec<u8>){
        match self{Self::Ordinary=>{},Self::Delete{previous,at}=>{
            b.extend_from_slice(&previous);b.extend_from_slice(&at.seconds.to_be_bytes());b.extend_from_slice(&at.nanos.to_be_bytes());
        },Self::Restore{deleted_by,previous}=>{b.extend_from_slice(&deleted_by.0);b.extend_from_slice(&previous);}}
    }
    pub(super) fn get(tag:u8,r:&mut In<'_>)->io::Result<Self>{
        match tag{0|1=>Ok(Self::Ordinary),2=>Ok(Self::Delete{previous:r.fixed()?,at:DeletedAt::new(r.u64()?,r.u32()?)?}),
            3=>Ok(Self::Restore{deleted_by:OperationId(r.id()?),previous:r.fixed()?}),_=>Err(invalid("unknown custody purpose"))}
    }
    pub(in crate::ordinary) fn check_transfer(self,method:Method,from:&wire::NativeObject,to:&wire::NativeObject)->io::Result<()>{
        self.tag(method)?;
        let same_device=from.entity.device==to.entity.device;let same_fs=from.birth.filesystem()==to.birth.filesystem();
        if method==Method::Rename&&!(same_device&&same_fs) || self!=Self::Ordinary&&(same_device||same_fs){
            return Err(invalid("custody method does not match the two original filesystems"));
        }Ok(())
    }
    pub(super) fn leaves(self,file:FileId,op:OperationId,human:&str)->io::Result<(CString,CString)>{self.call().leaves(file,op,human)}
}

pub(super) const MAX_EXTRA:usize=16+32; // max(previous + u64 seconds + u32 nanos, op + previous)
