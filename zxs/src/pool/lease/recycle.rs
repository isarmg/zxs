//! The two ends of a dedicated lifecycle transfer. Register evidence can be
//! consulted offline. Only a subsequent physical observation opens roots, and
//! neither object exposes the recycle directory as a general FILE volume.
use super::{Access,FileVolume,LocalPool};
use crate::ordinary::{LifecycleGrant,Side};
use crate::{Entity,Role,VolumeId,fs::{self,Birth,HeldDir,invalid},pool::{format::Record,require_system}};
use std::io;
pub(crate) mod incoming;
mod retirement;

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
    /// Read only the original register; the FILE or recycle root may be offline.
    pub(crate) fn lifecycle_register(&self)->(Entity,Birth){
        (self.registration.origin,self.registration.origin_birth.clone())
    }
    pub(crate) fn recycle_for(&self,ordinary:VolumeId)->io::Result<VolumeId>{
        let bin=self.registration.info.volumes.get(1).filter(|r|r.volume.role==Role::Recycle)
            .ok_or_else(||invalid("missing original dedicated recycle role"))?.id;
        let _=self.registered_dedicated(Direction::ToRecycle,ordinary,bin)?;Ok(bin)
    }
    pub(crate) fn registered_dedicated(&self,direction:Direction,from:VolumeId,to:VolumeId)->io::Result<Route<'_>>{
        self.registration.dedicated_route(fs::local_uid(),direction,from,to)
    }
    /// Reopening ONE authorized end must not require its offline peer. A
    /// submitted/granted role is checked against the same immutable record
    /// on every call. Ordinary FILE APIs cannot borrow this return type.
    pub(crate) fn custody_end<'g,'p>(&'g self,grant:&'g LifecycleGrant<'p>,side:Side)->io::Result<CustodyEnd<'g,'p>>{
        let(direction,from,to)=grant.roles(self)?;
        let route=self.registered_dedicated(direction,from,to)?;let(s,t)=route.ends();
        let index=match side{Side::Source=>s,Side::Target=>t};
        let role=if index==route.recycle{Role::Recycle}else{Role::File};
        let root=self.open_root(index,role)?;
        let end=CustodyEnd{pool:self,grant,side,index,role,root};end.require_current()?;Ok(end)
    }
    pub(crate) fn observe_dedicated(&self,direction:Direction,from:VolumeId,to:VolumeId)->io::Result<Observed<'_>>{
        let route=self.registration.dedicated_route(fs::local_uid(),direction,from,to)?;
        let ordinary=self.ordinary(self.registration.info.volumes[route.ordinary].id)?;
        let recycle=self.open_root(route.recycle,Role::Recycle)?;
        let held=Observed{pool:self,route,ordinary,recycle};
        held.require_current()?;
        Ok(held)
    }
}
/// No deref, root path, directory descriptor, arbitrary component, clone or
/// unscoped callback. The lease validates the same grant on every operation.
/// A long-running clean reader may outlive it, but contains only immutable
/// FILE children and is not permission to locate or modify another item.
pub(crate) struct CustodyEnd<'g,'p>{pool:&'g LocalPool,grant:&'g LifecycleGrant<'p>,side:Side,index:usize,role:Role,root:HeldDir}
impl CustodyEnd<'_, '_>{
    pub(crate) fn id(&self)->VolumeId{self.pool.registration.info.volumes[self.index].id}
    pub(crate) fn role(&self)->Role{self.role}
    pub(crate) fn belongs(&self,g:&LifecycleGrant<'_>,side:Side)->bool{self.side==side&&std::ptr::eq(self.grant,g)}
    fn require_current(&self)->io::Result<()>{
        self.grant.roles(self.pool)?;self.pool.check_root(self.index,self.role,&self.root)?;
        if self.role==Role::Recycle&&self.pool.system.same_mount(&self.root)?{return Err(invalid("dedicated recycle shares SYSTEM's mount"));}
        require_system(&self.pool.system,&self.pool.registration)
    }
    pub(crate) fn identity(&self)->io::Result<(Entity,Birth)>{
        self.require_current()?;Ok((self.root.entity,Birth::read(&self.root.file)?))
    }
    pub(crate) fn target_present(&self)->io::Result<bool>{
        if self.side!=Side::Target{return Err(invalid("not this operation's target"));}
        self.require_current()?;let v=self.root.entry_present(self.grant.name(Side::Target))?;self.require_current()?;Ok(v)
    }
    // May expose pins for the exact selected FILE only, never its recycle
    // root. Authority and exact frame are re-established for each opening.
    fn target_child(&self,goal:&crate::ordinary::LifecycleTarget)->io::Result<HeldDir>{
        if self.side!=Side::Target||!matches!(self.pool.access,Access::Incoming|Access::Write){return Err(invalid("not a bounded lifecycle target observation"));}
        self.require_current()?;let name=goal.name(self.pool,self.grant)?;
        goal.namespace(self.pool,self.grant,self.role,&self.root.names()?)?;
        let d=self.root.child_directory(&name)?.ok_or_else(||invalid("original lifecycle target is unavailable"))?;
        self.require_current()?;Ok(d)
    }
    pub(crate) fn check_target_child(&self,goal:&crate::ordinary::LifecycleTarget,held:&HeldDir)->io::Result<()>{
        let d=self.target_child(goal)?;
        if d.path!=held.path||d.entity!=held.entity||Birth::read(&d.file)?!=Birth::read(&held.file)?||!d.same_mount(held)?{
            return Err(invalid("different original dedicated target FILE or private stage"));}
        held.require_named()?;self.require_current()
    }
    pub(crate) fn read_target(&self,goal:&crate::ordinary::LifecycleTarget)->io::Result<crate::ordinary::LifecycleScan>{
        let dir=self.target_child(goal)?;goal.start(self.pool,self.grant,self,dir)
    }
    pub(crate) fn install_checked(&self,checked:&crate::ordinary::LifecycleChecked,after:impl FnOnce()->io::Result<()>)->io::Result<()>{
        checked.require(self.pool,self.grant,self)?;let(old,new,entity,birth)=checked.install_names(self.pool,self.grant)?;
        self.root.handoff_directory(&old,&self.root,&new,entity,&birth,after)
    }
    pub(crate) fn sync_installed(&self,checked:&crate::ordinary::LifecycleChecked)->io::Result<()>{
        if !matches!(self.pool.access,Access::Write)||!checked.installed(){return Err(invalid("not an installed original lifecycle target"));}
        checked.require(self.pool,self.grant,self)?;self.root.file.sync_all()?;checked.require(self.pool,self.grant,self)
    }
    fn source_child(&self)->io::Result<HeldDir>{
        if self.side!=Side::Source||!matches!(self.pool.access,Access::Incoming|Access::Write){
            return Err(invalid("requires the original incoming FILE source lease"));}
        self.require_current()?;
        if let Some(stage)=self.grant.prior_stage(){if self.root.entry_present(stage)?{return Err(invalid("previous FILE stage remains at the source"));}}
        let d=self.root.child_directory(self.grant.name(Side::Source))?.ok_or_else(||invalid("selected FILE source is unavailable"))?;
        let(e,b)=self.grant.source_directory();
        if d.entity!=e||Birth::read(&d.file)?!=*b{return Err(invalid("not the original lifecycle FILE directory"));}
        self.require_current()?;Ok(d)
    }
    pub(crate) fn check_source(&self,held:&HeldDir)->io::Result<()>{
        let d=self.source_child()?;
        if d.path!=held.path||d.entity!=held.entity||Birth::read(&d.file)?!=Birth::read(&held.file)?||
            !d.same_mount(held)?{return Err(invalid("not the selected held source FILE"));}
        held.require_named()?;self.require_current()
    }
    pub(crate) fn read_clean_source(&self)->io::Result<crate::ordinary::LifecycleSource>{
        let dir=self.source_child()?;
        let clean=self.grant.read_source(self.pool,self,dir)?;self.require_current()?;Ok(clean)
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
