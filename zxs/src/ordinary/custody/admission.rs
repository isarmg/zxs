//! Only a native, original local FILE request can borrow R. Knowing its ID,
//! seeing its label or having a diagnostic Route is insufficient. The same
//! SYSTEM cut chooses the file, previous placement, complete original FILE
//! edge and both endpoints. No installation, lifecycle decision or retirement
//! is made in this module.
use super::{allocation::{Progress,Stage as DirectoryStage},command::Request,purpose::Call,transfer::{Origin,Sites}};
use crate::{VolumeId,fs::{self,invalid},ordinary::{journal::{Journal,credits::Deletion},wire::{MAX_FRAME,NativeObject}},pool::lease::{LocalPool,recycle::Direction}};
use std::{ffi::CString,io};

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) enum Side{Source,Target}
#[derive(Eq,PartialEq)]
pub(super) struct Stage{
    frontier:(u64,[u8;32]),reservation:Option<([u8;32],fs::OriginalStamp,Option<DirectoryStage>,Progress)>,
    intent:Option<([u8;32],fs::OriginalStamp)>,tail:usize,
}
impl Stage{
    pub(super) fn of(s:&Journal)->io::Result<Self>{
        Ok(Self{frontier:s.frontier(),reservation:s.moving.as_ref().map(|a|(a.digest,a.stamp.clone(),a.stage().cloned(),a.progress())),
            intent:s.pending.as_ref().map(|i|i.frame.moved().map(|m|(m.digest,i.stamp.clone()))).transpose()?,tail:s.tail})
    }
}
pub(crate) struct LifecycleGrant<'p>{
    pool:&'p LocalPool,request:Request,direction:Direction,prior:[u8;32],stage:Stage,
    names:(CString,CString),prior_stage:Option<CString>,source:NativeObject,target:NativeObject,source_sites:Sites,
}
impl LifecycleGrant<'_>{
    // Only called by the physical lease owner; a valid grant cannot be
    // detached from its exact held SYSTEM register, even for the same pool.
    pub(crate) fn roles(&self,p:&LocalPool)->io::Result<(Direction,VolumeId,VolumeId)>{
        let _=self.live_cut(p)?;Ok((self.direction,self.request.from,self.request.to))
    }
    /// The exact journal returned is the same one checked here. The caller
    /// cannot combine its older undecided snapshot with a later live grant.
    pub(super) fn live_cut(&self,p:&LocalPool)->io::Result<Journal>{
        if !std::ptr::eq(p,self.pool){return Err(invalid("lifecycle operand belongs to a different SYSTEM cut"));}
        let(_,s)=p.snapshot()?;let now=LifecycleGrant::from_same_cut(p,&s,self.request)?;
        if now.stage!=self.stage||now.prior!=self.prior||now.source!=self.source||now.target!=self.target||
            now.names!=self.names||now.prior_stage!=self.prior_stage||now.source_sites!=self.source_sites{
            return Err(invalid("lifecycle operand no longer has the same original cut"));
        }Ok(s)
    }
    pub(super) fn predecision(&self,p:&LocalPool)->io::Result<Journal>{
        let s=self.live_cut(p)?;
        if s.decided(self.request.op).is_some(){return Err(invalid("decided custody cannot prepare its old source again"));}Ok(s)
    }
    pub(crate) fn name(&self,side:Side)->&std::ffi::CStr{match side{Side::Source=>&self.names.0,Side::Target=>&self.names.1}}
    pub(super) fn request(&self)->Request{self.request}
    pub(crate) fn prior_stage(&self)->Option<&std::ffi::CStr>{self.prior_stage.as_deref()}
    pub(crate) fn source_directory(&self)->(crate::Entity,&fs::Birth){
        (self.source_sites.directory.entity,&self.source_sites.directory.birth)
    }
    /// Receives only the selected FILE child, not the registered parent.
    /// Return an opaque reader; the caller cannot take any raw descriptor.
    pub(crate) fn read_source(&self,p:&LocalPool,end:&crate::pool::lease::recycle::CustodyEnd<'_,'_>,dir:fs::HeldDir)->io::Result<super::source::Clean>{
        super::source::Clean::from_lifecycle(p,self,end,dir)
    }
    pub(super) fn root(&self,side:Side)->&NativeObject{match side{Side::Source=>&self.source,Side::Target=>&self.target}}
    pub(super) fn sites(&self)->&Sites{&self.source_sites}
    pub(super) fn prior(&self)->[u8;32]{self.prior}
}
impl<'p> LifecycleGrant<'p>{
    pub(super) fn capture(p:&'p LocalPool,r:Request)->io::Result<Self>{
        let(_,s)=p.snapshot()?;Self::from_same_cut(p,&s,r)
    }
    fn from_same_cut(p:&'p LocalPool,s:&Journal,r:Request)->io::Result<Self>{
        let direction=match r.call{Call::Delete=>Direction::ToRecycle,Call::Restore{..}=>Direction::FromRecycle,
            Call::Move=>return Err(invalid("ordinary MOVE cannot acquire recycle access"))};
        let digest=r.digest(p)?;
        let route=p.registered_dedicated(direction,r.from,r.to)?;
        let(f,t)=route.originals();
        let source=NativeObject{entity:f.1,birth:f.2.clone()};let target=NativeObject{entity:t.1,birth:t.2.clone()};
        let c=s.file(r.id).ok_or_else(||invalid("missing original FILE"))?;
        if c.create.uid!=fs::local_uid(){return Err(invalid("wrong original FILE principal"));}
        let full=s.head(r.id).ok_or_else(||invalid("lifecycle requires a fully committed original main FULL"))?;
        let (prior,child,original)=if let Some(d)=s.decided(r.op){
            // A finished receipt is history only: never mint fresh physical
            // access for it, and never reinterpret it against today's owner.
            let(m,a)=s.confirmed_handoff(r.op)?;
            if d.moved()?!=m||m.request!=digest||a.original.purpose.call()!=r.call{
                return Err(invalid("not this pending original lifecycle completion"));
            }
            let(_,_,child)=super::finish::frozen(s,m)?;
            (s.before_handoff(m)?,child,Some(&a.original))
        }else{
            let prior=s.placement(r.id)?;let child=s.actual_graph(r.id)?.children();
            if s.active_work(r.id)?.last().is_some(){return Err(invalid("dirty original revision cannot change custody"));}
            let original=if let Some(a)=&s.moving{
                if a.original.op!=r.op||a.request!=digest||a.original.purpose.call()!=r.call{
                    return Err(invalid("another original custody operation must complete"));}
                Some(&a.original)
            }else{
                if s.pending.is_some()||s.allocated.is_some()||s.saving.is_some()||s.committing.is_some()||
                    s.editing.is_some()||s.incrementing.is_some()||s.ticket_used(r.op){
                    return Err(invalid("another original operation needs the current slot"));}
                None
            };
            // In particular, do not precharge an Ordinary frame to a matching
            // RESTORE that needs its own final reserved position or bytes.
            s.room_for_effect(MAX_FRAME,r.call.effect(r.id,r.op))?;
            if matches!(r.call,Call::Restore{..})&&s.occupied_root(r.to,&c.create.name){
                return Err(invalid("a different active FILE owns the restore destination name"));}
            (prior,child,original)
        };
        if prior.volume()!=r.from||prior.epoch()!=r.epoch{return Err(invalid("lifecycle no longer selects this original FILE placement"));}
        if let Some(m)=prior.transfer(){if m.original.dst_root!=source{return Err(invalid("current owner no longer matches registered source"));}}
        let key=prior.key();
        match r.call{
            Call::Delete if prior.retained().is_none()=>{},
            Call::Restore{deleted_by,decision} if prior.retained()==Some(Deletion{file:r.id,operation:deleted_by,decision})=>{},
            _=>return Err(invalid("not the selected active FILE or exact still-retained original deletion")),
        }
        if let Some(o)=original{require_original(r,&digest,p.id(),o,&source,&target)?;prior.require_next(o)?;o.require_history(c,full,child)?;}
        let sites=prior.sites(full,child)?;sites.check(&source)?;
        let names=r.call.leaves(r.id,r.op,&c.create.name)?;
        let prior_stage=prior.transfer().and_then(|m|m.stage.map(|slot|crate::ordinary::material::staging(m.original.ticket,slot)));
        Ok(Self{pool:p,request:r,direction,prior:key,stage:Stage::of(s)?,names,prior_stage,source,target,source_sites:sites})
    }
}
fn require_original(r:Request,digest:&[u8;32],pool:crate::PoolId,o:&Origin,source:&NativeObject,target:&NativeObject)->io::Result<()>{
    if o.file!=r.id||o.uid!=fs::local_uid()||o.op!=r.op||o.from!=r.from||o.to!=r.to||o.epoch!=r.epoch||
        o.purpose.call()!=r.call||o.request_digest(pool)?!=*digest||o.src_root!=*source||o.dst_root!=*target{
        return Err(invalid("wrong original lifecycle reservation and registered endpoints"));
    }
    Ok(())
}
