//! A private transfer has one verified original and one independently selected
//! target. No entry, reservation or decision exists merely because the entire
//! physical payload was allocatable. An original unbound reservation can
//! reprepare anonymous bytes; a missing linked stage is never recreated.
use super::{admission::{LifecycleGrant,Side},command::Request,source::{Leaf,Verified},staging::Existing,transfer};
use crate::{OperationId,PoolId,Role,fs::{self,invalid},ordinary::{allocation::SLOTS,material,wire::NativeObject},
    pool::lease::{Access,LocalPool,recycle::{CustodyEnd,incoming::PreparedIncoming}}};
use std::{collections::{BTreeMap,BTreeSet},ffi::{CStr,CString},io,path::Path};

pub(crate) struct IncomingSelection<'v>{source:&'v Verified,request:Request,prior:[u8;32],pool:PoolId,system:NativeObject,
    register:(crate::Entity,fs::Birth),target:NativeObject,ticket:OperationId,original:Option<([u8;32],NativeObject)>}
impl<'v> IncomingSelection<'v>{
    #[cfg(test)]
    pub(super) fn prepare(system:&Path,source:&'v Verified,r:Request)->io::Result<PreparedIncoming<'v>>{
        Self::prepare_checked(system,source,r,|_|Ok(()))
    }
    pub(super) fn prepare_checked(system:&Path,source:&'v Verified,r:Request,check:impl FnOnce(&LocalPool)->io::Result<()>)->io::Result<PreparedIncoming<'v>>{
        let incoming={
            let(p,_,_)=LocalPool::with_snapshot(system,Access::Incoming)?;check(&p)?;let g=LifecycleGrant::capture(&p,r)?;
            let src=p.custody_end(&g,Side::Source)?;let dst=p.custody_end(&g,Side::Target)?;
            let s=g.predecision(&p)?;
            let(ticket,original)=match s.moving.as_ref(){
                Some(a)=>(a.original.ticket,Some((a.digest,NativeObject::stamp(&a.stamp)))),
                None=>(OperationId::new()?,None)
            };
            let select=Self{source,request:r,pool:p.id(),prior:g.prior(),target:g.root(Side::Target).clone(),
                system:NativeObject{entity:p.system.entity,birth:fs::Birth::read(&p.system.file)?},register:p.lifecycle_register(),ticket,original};
            dst.claim_anonymous(&src,select)?
        }; // Release all SYSTEM and root locks before reservation of bytes or long transfer.
        let copy=incoming.copy()?;
        copy.recheck(system)?;Ok(copy)
    }
    pub(crate) fn grant<'p>(&self,p:&'p LocalPool)->io::Result<LifecycleGrant<'p>>{LifecycleGrant::capture(p,self.request)}
    pub(crate) fn with_ends<T>(&self,system:&Path,access:Access,
        apply:impl FnOnce(&LocalPool,&CustodyEnd<'_,'_>,&CustodyEnd<'_,'_>)->io::Result<T>)->io::Result<T>{
        let(p,_,_)=LocalPool::with_snapshot(system,access)?;let g=LifecycleGrant::capture(&p,self.request)?;
        let src=p.custody_end(&g,Side::Source)?;let dst=p.custody_end(&g,Side::Target)?;
        self.require(&p,&g,&src,&dst)?;apply(&p,&src,&dst)
    }
    pub(crate) fn require(&self,p:&LocalPool,g:&LifecycleGrant<'_>,src:&CustodyEnd<'_,'_>,dst:&CustodyEnd<'_,'_>)->io::Result<()>{
        if !matches!(p.access,Access::Incoming|Access::Write)||!src.belongs(g,Side::Source)||!dst.belongs(g,Side::Target)||
            self.request!=g.request()||self.pool!=p.id()||self.register!=p.lifecycle_register()||self.system.entity!=p.system.entity||self.system.birth!=fs::Birth::read(&p.system.file)?||
            self.prior!=g.prior()||&self.target!=g.root(Side::Target)||self.source.source_root()!=g.root(Side::Source)||
            self.ticket==self.request.op||self.source.volume()!=self.request.from||self.source.file()!=self.request.id{
            return Err(invalid("another source, target, subject or placement at anonymous preparation"));}
        self.source.require_lifecycle(p,g,src)?;
        let s=g.predecision(p)?;p.require_business_op(self.ticket)?;
        if s.ticket_used(self.ticket){return Err(invalid("ticket was already decided for another original"));}
        self.same_original(p,&s,g)
    }
    fn same_original(&self,p:&LocalPool,s:&crate::ordinary::journal::Journal,g:&LifecycleGrant<'_>)->io::Result<()>{
        let Some(a)=s.moving.as_ref()else{
            if self.original.is_some(){return Err(invalid("selected original reservation disappeared"));}
            return Ok(())
        };
        if a.opened()||s.pending.as_ref().is_some_and(|i|i.frame.moved().map_or(true,|m|!a.matches(m))){
            return Err(invalid("not the original predecision lifecycle stage"));}
        let o=&a.original;let r=self.request;
        let(c,f,d)=self.source.originals();
        if a.request!=r.digest(p)?||o.ticket!=self.ticket||o.op!=r.op||o.purpose.call()!=r.call||
            o.src_root!=*self.source.source_root()||o.dst_root!=self.target||o.source_bytes!=self.source.physical_bytes()||
            o.source_material!=self.source.digest()?||self.original.as_ref().is_some_and(|(hash,node)|*hash!=a.digest||*node!=NativeObject::stamp(&a.stamp)){
            return Err(invalid("not the same original validated reservation or complete predecision material"));}
        s.placement(r.id)?.require_next(o)?;o.require_history(c,f,d)?;
        if g.prior()!=self.prior||o.mode!=super::Method::Transfer{return Err(invalid("different original storage transfer"));}
        Ok(())
    }
    pub(crate) fn stage_state(&self,p:&LocalPool,g:&LifecycleGrant<'_>)->io::Result<(bool,Option<(u8,NativeObject)>)>{
        if self.request!=g.request(){return Err(invalid("wrong private ticket owner"));}
        let s=g.predecision(p)?;self.same_original(p,&s,g)?;
        Ok((s.moving.is_some(),s.moving.as_ref().and_then(|a|a.stage()).map(|st|(st.slot,st.directory.clone()))))
    }
    pub(crate) fn capture_known(&self,p:&LocalPool,g:&LifecycleGrant<'_>,dir:Option<fs::HeldDir>)->io::Result<Option<KnownStage>>{
        let s=g.predecision(p)?;self.same_original(p,&s,g)?;
        match(s.moving.as_ref().and_then(|a|a.stage().map(|st|(a,st))),dir){
            (None,None)=>Ok(None),
            (Some((a,st)),Some(dir))=>{
                let seal=s.pending.as_ref().map(|i|i.frame.moved()).transpose()?;
                let fixed=Existing::capture_roles(self.source,a,seal,st,dir)?;
                Ok(Some(KnownStage{fixed,order:self.source.objects().iter().map(|row|row.kind).collect()}))
            }
            _=>Err(invalid("bound lifecycle target missing or unissued native child")),
        }
    }
    pub(crate) fn require_known(&self,p:&LocalPool,g:&LifecycleGrant<'_>,seen:Option<&KnownStage>,actual:Option<&fs::HeldDir>)->io::Result<()>{
        let s=g.predecision(p)?;self.same_original(p,&s,g)?;
        match(seen,actual,s.moving.as_ref().and_then(|a|a.stage().map(|st|(a,st)))){
            (Some(k),Some(dir),Some((a,_)))=>k.fixed.require_directory(a,dir),
            (None,None,None)=>Ok(()),
            // Only a new, genuinely empty, originally bound stage preserves a
            // still-entirely-anonymous preparation captured before the bind.
            (None,Some(dir),Some((_,st))) if dir.entity==st.directory.entity&&fs::Birth::read(&dir.file)?==st.directory.birth=>
                dir.only(None),
            _=>Err(invalid("private target prefix changed since the checked copy")),
        }
    }
    pub(crate) fn confirm_issued(&self,p:&LocalPool,g:&LifecycleGrant<'_>)->io::Result<()>{
        let s=g.predecision(p)?;self.same_original(p,&s,g)?;s.moving.as_ref().ok_or_else(||invalid("no original private reservation"))?.confirm_phase(p)
    }
    pub(crate) fn bind_created(&self,actual:&crate::pool::lease::recycle::incoming::FreshStageTurn<'_,'_>)->io::Result<()>{
        let(p,who,_,_,_)=actual.require()?;if !std::ptr::eq(self,who){return Err(invalid("unrelated prepared source"));}
        let g=LifecycleGrant::capture(p,self.request)?;let s=g.predecision(p)?;self.same_original(p,&s,&g)?;
        let a=s.moving.as_ref().ok_or_else(||invalid("unissued stage"))?;a.bind_created(actual)
    }
    pub(crate) fn new_role(&self,p:&LocalPool,g:&LifecycleGrant<'_>,i:usize,stamp:&fs::OriginalStamp)->io::Result<(CString,[u8;32])>{
        let s=g.predecision(p)?;self.same_original(p,&s,g)?;
        let a=s.moving.as_ref().filter(|a|a.stage().is_some()&&!a.opened()).ok_or_else(||invalid("no bound original role issuer"))?;
        if s.pending.is_some(){return Err(invalid("never replace or recreate a sealed original role"));}
        let part=self.source.objects().get(i).ok_or_else(||invalid("unknown original transfer role"))?;
        let n=NativeObject::stamp(stamp);
        if stamp.stat.size()!=part.len||n.entity.device!=self.target.entity.device||n.birth.filesystem()!=self.target.birth.filesystem()||
            self.source.objects().iter().any(|x|x.original.entity==n.entity||x.original.birth==n.birth){
            return Err(invalid("not the source role's independently allocated target"));}
        Ok((part.name.clone(),super::placed::token(a.digest,part.kind,&n)?))
    }
    pub(crate) fn goal(&self,p:&LocalPool,g:&LifecycleGrant<'_>,dir:&fs::HeldDir,complete:&[fs::OriginalStamp])->io::Result<super::target::Goal>{
        let s=g.predecision(p)?;self.same_original(p,&s,g)?;
        let a=s.moving.as_ref().ok_or_else(||invalid("no original private reservation"))?;
        let st=a.stage().ok_or_else(||invalid("unbound private stage cannot supply exact roles"))?;
        if dir.entity!=st.directory.entity||fs::Birth::read(&dir.file)?!=st.directory.birth||complete.len()!=self.count(){
            return Err(invalid("incorrect bound directory or incomplete target material"));}
        let mut selected=BTreeMap::new();
        for (row,stamp) in self.source.objects().iter().zip(complete){
            let actual=dir.pin_stream(&row.name,row.len)?.ok_or_else(||invalid("original private role no longer named"))?;
            if actual.stamp!=*stamp{return Err(invalid("named role changed after its one original link or cold verification"));}
            let n=NativeObject::stamp(&actual.stamp);fs::require_material_token(&actual.file,&super::placed::token(a.digest,row.kind,&n)?)?;
            actual.named(dir,&row.name)?;if selected.insert(row.kind,n).is_some(){return Err(invalid("duplicate role"));}
        }
        let sites=super::staging::sites(dir,&selected,&a.original)?;
        let m=transfer::Moved::seal(p.id(),a.seq,a.previous,a.original.clone(),a.digest,Some(st.slot),sites)?;
        let(_,f,d)=self.source.originals();m.require_at(s.placement(self.request.id)?,f,d)?;
        if !a.matches(&m)||s.pending.as_ref().is_some_and(|i|i.frame.bytes()!=m.bytes){return Err(invalid("new target bytes differ from the original sealed decision"));}
        Ok(super::target::Goal{frame:m,site:super::target::Site::Private})
    }
    pub(crate) fn allocation_count(&self,g:&LifecycleGrant<'_>)->io::Result<usize>{
        if self.request!=g.request()||self.prior!=g.prior(){return Err(invalid("not this original lifecycle selection"));}
        self.source.allocation_count(g.sites())
    }
    pub(crate) fn count(&self)->usize{self.source.objects().len()}
    pub(crate) fn claim(&self,i:usize)->io::Result<(u64,[u8;32])>{
        let x=self.source.objects().get(i).ok_or_else(||invalid("no such original source role"))?;Ok((x.len,x.digest))
    }
    pub(crate) fn stream(&self,i:usize,output:impl FnMut(u64,&[u8])->io::Result<()>)->io::Result<()>{
        let x=self.source.objects().get(i).ok_or_else(||invalid("no such original source role"))?;self.source.stream(x.kind,output)
    }
    pub(crate) fn ticket(&self)->OperationId{self.ticket}
    pub(crate) fn source_isolation(&self)->CString{transfer::isolation_name(self.ticket)}
    pub(crate) fn stage_names(&self)->impl Iterator<Item=CString>+'_ {(0..SLOTS).map(|i|material::staging(self.ticket,i))}
    pub(crate) fn check_occupancy(&self,p:&LocalPool,g:&LifecycleGrant<'_>,role:Role,leaf:&CStr,names:&[Vec<u8>])->io::Result<()>{
        let s=g.predecision(p)?;
        self.same_original(p,&s,g)?;
        if g.request()!=self.request||g.name(Side::Target)!=leaf||self.request.call==super::purpose::Call::Move{
            return Err(invalid("not the original, fresh lifecycle target"));}
        let spelling=leaf.to_str().map_err(|_|invalid("noncanonical custody target name"))?;
        let selected=|n:&[u8]|material::private_slot(n).is_some_and(|(op,_)|op==self.ticket);
        if names.iter().any(|n|n.as_slice()==leaf.to_bytes()||(s.moving.is_none()&&selected(n))){
            return Err(io::Error::new(io::ErrorKind::AlreadyExists,"exact original target or fresh private ticket already occupied"));}
        match role{
            Role::File=>{if !matches!(self.request.call,super::purpose::Call::Restore{..})||crate::ordinary::storage_name::file_leaf(self.source.name(),self.request.id)?.as_c_str()!=leaf{
                return Err(invalid("not an authorized ordinary restore target"));}
}
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
                for n in names{if !(s.moving.is_some()&&selected(n))&&!retained.contains(n)&&!material::private_slot(n).is_some_and(|(op,i)|skips.get(&op).is_some_and(|limit|i<*limit)){
                    return Err(invalid("unattributed occupant on the original recycle target"));}}
            }
            _=>return Err(invalid("SYSTEM is not a target for a FILE")),
        }
        // The slots already actually issued by this same ticket are not eight
        // new names on each retry. Physical checks still verify every occupant.
        let other=names.iter().filter(|n|!selected(n)).cloned().collect::<Vec<_>>();
        material::room_for_ticket(&other,spelling,&material::staging(self.ticket,0))
    }
}

/// Holds verified native private children only. No private target directory or
/// descriptor can be extracted; missing entries must be recreated anonymously.
pub(crate) struct KnownStage{fixed:Existing,order:Vec<Leaf>}
impl KnownStage{
    pub(crate) fn has(&self,i:usize)->bool{self.order.get(i).is_some_and(|k|self.fixed.contains(*k))}
    pub(crate) fn recognized(&self,i:usize,dir:&fs::HeldDir,selection:&IncomingSelection<'_>)->io::Result<Option<fs::OriginalStamp>>{
        let row=selection.source.objects().get(i).ok_or_else(||invalid("no source role at original ordinal"))?;
        if self.order.get(i)!=Some(&row.kind){return Err(invalid("another original role order"));}
        let Some(pin)=self.fixed.pin(row.kind)else{return Ok(None)};pin.named(dir,&row.name)?;Ok(Some(pin.stamp.clone()))
    }
    pub(crate) fn verify(&self)->io::Result<()>{self.fixed.verify()}
}

/// This is still a proposal, not a durable allocation or a permission to
/// commit a caller-supplied Origin. A later native issuer must consume the
/// live writer turn here at the actual write, never a cloned preview.
pub(super) fn proposed(turn:&crate::pool::lease::recycle::incoming::ReservationTurn<'_,'_>)->io::Result<super::transfer::Origin>{
    use super::{purpose::{Call,DeletedAt,Purpose},transfer::Origin,Method};
    let(p,selection)=turn.require()?;let r=selection.request;let g=LifecycleGrant::capture(p,r)?;
    let s=g.predecision(p)?;let(c,full,d)=selection.source.originals();
    if let Some(a)=s.moving.as_ref(){selection.same_original(p,&s,&g)?;turn.require()?;return Ok(a.original.clone());}
    let purpose=match r.call{Call::Delete=>Purpose::Delete{previous:g.prior(),at:DeletedAt::now()?},
        Call::Restore{deleted_by,decision}=>Purpose::Restore{deleted_by,previous:decision},
        Call::Move=>return Err(invalid("ordinary MOVE cannot use a dedicated original writer"))};
    let o=Origin{op:r.op,uid:fs::local_uid(),file:r.id,entry:c.create.entry,history:c.create.history,from:r.from,to:r.to,epoch:r.epoch,
        name:c.create.name.clone(),ticket:selection.ticket,mode:Method::Transfer,purpose,src_root:selection.source.source_root().clone(),
        dst_root:selection.target.clone(),creation:c.digest,parent:full.digest,children:d.map(|d|d.digest),
        source_bytes:selection.source.physical_bytes(),source_material:selection.source.digest()?};
    if r.digest(p)?!=o.request_digest(p.id())?{return Err(invalid("different original request at the reservation writer"));}
    s.check_new_lifecycle_origin(&p.journal_registration(),&o)?;turn.require()?;Ok(o)
}
