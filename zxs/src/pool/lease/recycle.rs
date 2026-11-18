//! The two ends of a dedicated lifecycle transfer. Register evidence can be
//! consulted offline. Only a subsequent physical observation opens roots, and
//! neither object exposes the recycle directory as a general FILE volume.
use super::{Access,FileVolume,LocalPool};
use crate::{Entity,Role,VolumeId,fs::{self,Birth,HeldDir,invalid},pool::{format::Record,require_system}};
use std::io;

#[derive(Debug,Clone,Copy,Eq,PartialEq)]
pub(crate) enum Direction{ToRecycle,FromRecycle}

/// Always ordered source→destination. Private fields: knowing two VolumeIds
/// alone is not evidence. This does not represent a particular FILE or a
/// decision that the FILE was deleted, was restored, or can be retired.
pub(crate) struct Route<'r>{record:&'r Record,direction:Direction,ordinary:usize,recycle:usize}
impl Record{
    pub(in crate::pool) fn dedicated_route(&self,uid:u32,direction:Direction,from:VolumeId,to:VolumeId)->io::Result<Route<'_>>{
        if uid!=self.actor_uid||from==to{return Err(invalid("invalid principal or repeated lifecycle volume"));}
        let(f,r)=match direction{Direction::ToRecycle=>(from,to),Direction::FromRecycle=>(to,from)};
        let ordinary=self.info.volumes.iter().position(|v|v.id==f && v.volume.role==Role::File)
            .ok_or_else(||invalid("lifecycle transfer requires this registered ordinary root"))?;
        // Record::prefix verifies exactly one recycle root; check its own ID,
        // physical device AND filesystem UUID against every original role.
        let recycle=1;
        let bin=self.info.volumes.get(recycle).filter(|v|v.id==r&&v.volume.role==Role::Recycle)
            .ok_or_else(||invalid("not the dedicated recycle volume of this pool"))?;
        let(_,rb)=self.original_root(recycle);
        if self.info.volumes.iter().enumerate().any(|(i,v)|i!=recycle&&
            (v.volume.role==Role::Recycle||v.volume.entity.device==bin.volume.entity.device||self.original_root(i).1.filesystem()==rb.filesystem())){
            return Err(invalid("dedicated recycle requires a distinct registered filesystem"));
        }
        Ok(Route{record:self,direction,ordinary,recycle})
    }
}
impl Route<'_>{
    fn ends(&self)->(usize,usize){
        match self.direction{Direction::ToRecycle=>(self.ordinary,self.recycle),Direction::FromRecycle=>(self.recycle,self.ordinary)}
    }
    fn at(&self,i:usize)->(VolumeId,Entity,&Birth){
        let(e,b)=self.record.original_root(i);(self.record.info.volumes[i].id,e,b)
    }
    /// Original raw registration/physical evidence, not an online health check.
    pub(crate) fn originals(&self)->((VolumeId,Entity,&Birth),(VolumeId,Entity,&Birth)){
        let(f,t)=self.ends();(self.at(f),self.at(t))
    }
    pub(crate) fn matches(&self,from:(VolumeId,Entity,&Birth),to:(VolumeId,Entity,&Birth))->bool{
        self.originals()==(from,to)
    }
}

/// Only the bounded pre-decision physical observation of both roots. No path,
/// directory FD or write operation is lent for recycle. Later installation or
/// cleanup must use its own original decision, including offline-source cases.
pub(crate) struct Observed<'p>{pool:&'p LocalPool,route:Route<'p>,ordinary:FileVolume<'p>,recycle:HeldDir}
impl LocalPool{
    pub(crate) fn observe_dedicated(&self,direction:Direction,from:VolumeId,to:VolumeId)->io::Result<Observed<'_>>{
        let route=self.registration.dedicated_route(fs::local_uid(),direction,from,to)?;
        let ordinary=self.ordinary(self.registration.info.volumes[route.ordinary].id)?;
        let recycle=self.open_root(route.recycle,Role::Recycle)?;
        let held=Observed{pool:self,route,ordinary,recycle};
        held.require_current()?;
        Ok(held)
    }
}
impl Observed<'_>{
    pub(crate) fn registration(&self)->&Route<'_>{&self.route}
    pub(crate) fn require_current(&self)->io::Result<()>{
        // Check the original SYSTEM record, not just the same displayed path
        // or an earlier diagnostic PoolInfo. Do not open unrelated FILE roots.
        self.pool.snapshot()?;
        require_system(&self.pool.system,&self.pool.registration)?;
        self.ordinary.require_current()?;
        self.pool.check_root(self.route.recycle,Role::Recycle,&self.recycle)?;
        if self.ordinary.root.same_mount(&self.recycle)?||self.pool.system.same_mount(&self.recycle)?{
            return Err(invalid("recycle cannot be a same-mount rename"));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path="../../../tests/unit/recycle_roles.rs"]
mod tests;
