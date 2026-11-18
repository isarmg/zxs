//! A private transfer has one verified original and one independently selected
//! target. No entry, reservation or decision exists merely because the entire
//! physical payload was allocatable. This first constructor is for a fresh
//! operation only; a durable existing stage must never be replaced by it.
use super::{admission::{LifecycleGrant,Side},command::Request,source::Verified,transfer};
use crate::{OperationId,PoolId,Role,fs::{self,invalid},ordinary::{allocation::SLOTS,material,wire::NativeObject},
    pool::lease::{Access,LocalPool,recycle::{CustodyEnd,incoming::PreparedIncoming}}};
use std::{collections::{BTreeMap,BTreeSet},ffi::{CStr,CString},io,path::Path};

pub(crate) struct IncomingSelection<'v>{source:&'v Verified,request:Request,prior:[u8;32],pool:PoolId,system:NativeObject,
    target:NativeObject,ticket:OperationId}
impl<'v> IncomingSelection<'v>{
    pub(super) fn prepare(system:&Path,source:&'v Verified,r:Request)->io::Result<PreparedIncoming<'v>>{
        let incoming={
            let(p,_,_)=LocalPool::with_snapshot(system,Access::Incoming)?;let g=LifecycleGrant::capture(&p,r)?;
            let src=p.custody_end(&g,Side::Source)?;let dst=p.custody_end(&g,Side::Target)?;
            let ticket=OperationId::new()?;
            let select=Self{source,request:r,pool:p.id(),prior:g.prior(),target:g.root(Side::Target).clone(),
                system:NativeObject{entity:p.system.entity,birth:fs::Birth::read(&p.system.file)?},ticket};
            dst.claim_anonymous(&src,select)?
        }; // Release all SYSTEM and root locks before reservation of bytes or long transfer.
        let copy=incoming.copy()?;
        copy.recheck(system)?;Ok(copy)
    }
    pub(crate) fn with_ends<T>(&self,system:&Path,access:Access,
        apply:impl FnOnce(&LocalPool,&CustodyEnd<'_,'_>,&CustodyEnd<'_,'_>)->io::Result<T>)->io::Result<T>{
        let(p,_,_)=LocalPool::with_snapshot(system,access)?;let g=LifecycleGrant::capture(&p,self.request)?;
        let src=p.custody_end(&g,Side::Source)?;let dst=p.custody_end(&g,Side::Target)?;
        self.require(&p,&g,&src,&dst)?;apply(&p,&src,&dst)
    }
    pub(crate) fn require(&self,p:&LocalPool,g:&LifecycleGrant<'_>,src:&CustodyEnd<'_,'_>,dst:&CustodyEnd<'_,'_>)->io::Result<()>{
        if !matches!(p.access,Access::Incoming|Access::Write)||!src.belongs(g,Side::Source)||!dst.belongs(g,Side::Target)||
            self.request!=g.request()||self.pool!=p.id()||self.system.entity!=p.system.entity||self.system.birth!=fs::Birth::read(&p.system.file)?||
            self.prior!=g.prior()||&self.target!=g.root(Side::Target)||self.source.source_root()!=g.root(Side::Source)||
            self.ticket==self.request.op||self.source.volume()!=self.request.from||self.source.file()!=self.request.id{
            return Err(invalid("another source, target, subject or placement at anonymous preparation"));}
        self.source.require_lifecycle(p,g,src)?;
        let s=g.predecision(p)?;p.require_business_op(self.ticket)?;
        if s.moving.is_some()||s.ticket_used(self.ticket){return Err(invalid("new anonymous target cannot replace an existing original reservation"));}
        Ok(())
    }
    pub(crate) fn count(&self)->usize{self.source.objects().len()}
    pub(crate) fn claim(&self,i:usize)->io::Result<(u64,[u8;32])>{
        let x=self.source.objects().get(i).ok_or_else(||invalid("no such original source role"))?;Ok((x.len,x.digest))
    }
    pub(crate) fn stream(&self,i:usize,output:impl FnMut(u64,&[u8])->io::Result<()>)->io::Result<()>{
        let x=self.source.objects().get(i).ok_or_else(||invalid("no such original source role"))?;self.source.stream(x.kind,output)
    }
    pub(crate) fn source_isolation(&self)->CString{transfer::isolation_name(self.ticket)}
    pub(crate) fn stage_names(&self)->impl Iterator<Item=CString>+'_ {(0..SLOTS).map(|i|material::staging(self.ticket,i))}
    pub(crate) fn check_occupancy(&self,p:&LocalPool,g:&LifecycleGrant<'_>,role:Role,leaf:&CStr,names:&[Vec<u8>])->io::Result<()>{
        let s=g.predecision(p)?;
        if g.request()!=self.request||g.name(Side::Target)!=leaf||s.moving.is_some()||self.request.call==super::purpose::Call::Move{
            return Err(invalid("not the original, fresh lifecycle target"));}
        let spelling=leaf.to_str().map_err(|_|invalid("noncanonical custody target name"))?;
        if names.iter().any(|n|n.eq_ignore_ascii_case(leaf.to_bytes())||material::private_slot(n).is_some_and(|(op,_)|op==self.ticket)){
            return Err(io::Error::new(io::ErrorKind::AlreadyExists,"original target, equivalent name or issued stage slot already occupied"));}
        match role{
            Role::File=>{if !matches!(self.request.call,super::purpose::Call::Restore{..})||self.source.name()!=spelling{
                return Err(invalid("not an authorized ordinary restore target"));}
                material::check_siblings(names,spelling,false)?;}
            Role::Recycle=>{if self.request.call!=super::purpose::Call::Delete{return Err(invalid("not this original dedicated recycle destination"));}
                // No content scan, no borrowing a directory listing as current
                // custody. The only allowed occupants are current confirmed R
                // names or earlier slots issued to precisely this R.
                let mut retained=BTreeSet::new();let mut skips=BTreeMap::new();
                for m in s.moves().filter(|m|m.original.to==self.request.to){
                    if !matches!(m.original.purpose,super::purpose::Purpose::Delete{..}){return Err(invalid("non-delete original at dedicated destination"));}
                    if let Some(slot)=m.stage{skips.insert(m.original.ticket,slot);}
                    if s.placed(m.original.file).map(|x|x.digest)==Some(m.digest){
                        retained.insert(m.original.purpose.leaves(m.original.file,m.original.op,&m.original.name)?.1.into_bytes());
                    }
                }
                for n in names{if !retained.contains(n)&&!material::private_slot(n).is_some_and(|(op,i)|skips.get(&op).is_some_and(|limit|i<*limit)){
                    return Err(invalid("unattributed occupant on the original recycle target"));}}
            }
            _=>return Err(invalid("SYSTEM is not a target for a FILE")),
        }
        material::room_for_ticket(names,spelling,&material::staging(self.ticket,0))
    }
}
