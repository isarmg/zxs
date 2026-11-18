//! A handoff uses the existing exclusive SYSTEM operand. Its immutable bytes
//! reserve the original FILE and both roots. Its bounded progress attribute
//! cannot establish a decision or reconstruct a missing file.
use super::{destination::{Method,Prepared},transfer::{self,Moved,Origin}};
use crate::{OperationId,PoolId,fs::{self,HeldDir,invalid},ordinary::{allocation::{NAME,SLOTS},wire::{In,NativeObject}}};
use std::io;
pub(in crate::ordinary) const MAGIC:&[u8]=b"ZXS/ORIGINAL-FILE-HANDOFF\0";
pub(in crate::ordinary) const MAX:usize=2048;

#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Stage{pub slot:u8,pub directory:NativeObject}
#[derive(Clone,Debug,Eq,PartialEq,Default)]
struct Phase{stage:Option<Stage>,step:Progress}
/// One bounded progress value on the original operand, not another decision.
/// 2 authorizes only isolation; 3..=6 authorize exactly one named source
/// role each. Only 7 proves the checked, synced original quarantine was empty.
#[derive(Clone,Copy,Debug,Default,Eq,PartialEq,Ord,PartialOrd)]
pub(super) struct Progress(u8);
impl Progress{
    const STAGING:Self=Self(0);pub(super) const OPEN:Self=Self(1);
    pub(super) const ARMED:Self=Self(2);pub(super) const EMPTY:Self=Self(7);
    pub(super) fn leaf(i:usize)->io::Result<Self>{
        if i<4{Ok(Self(3+i as u8))}else{Err(invalid("unknown original retirement role"))}
    }
    pub(super) fn authorized(self)->Option<usize>{(3..=6).contains(&self.0).then(||(self.0-3)as usize)}
    fn allows_next(self,count:usize,next:Self)->bool{
        if !(3..=4).contains(&count)||self<Progress::ARMED{return false;}
        if next==Progress::EMPTY{return self.authorized()==Some(count-1);}
        next.0==self.0+1&&next.authorized().is_some_and(|i|i<count)
    }
    fn read(n:u8)->io::Result<Self>{if n<=7{Ok(Self(n))}else{Err(invalid("unknown MOVE progress"))}}
}
impl Phase{
    fn check(&self,o:&Origin)->io::Result<()>{
        if self.step.authorized().is_some_and(|i|i>=o.role_count())||self.step>=Progress::ARMED&&(o.mode!=Method::Transfer||self.stage.is_none())||self.step>=Progress::OPEN&&o.mode==Method::Transfer&&self.stage.is_none()||self.stage.as_ref().is_some_and(|s|
            o.mode!=Method::Transfer||s.slot>=SLOTS||s.directory.entity.device!=o.dst_root.entity.device||
            s.directory.birth.filesystem()!=o.dst_root.birth.filesystem()||[&o.src_root,&o.dst_root].iter()
              .any(|r|r.entity==s.directory.entity||r.birth==s.directory.birth)){
            return Err(invalid("invalid handoff stage/incarnation or retirement progress"));
        }Ok(())
    }
    fn bytes(&self,digest:[u8;32])->io::Result<Vec<u8>>{
        let mut b=vec![self.step.0,u8::from(self.stage.is_some())];
        if let Some(s)=&self.stage{b.push(s.slot);s.directory.put(&mut b);}
        let mut h=crate::hash::Sha256::new();h.update(b"ZXS/ORIGINAL/MOVE-PHASE\0")?;
        h.update(&digest)?;h.update(&b)?;b.extend_from_slice(&h.finish());
        if b.len()>256{return Err(invalid("MOVE progress does not fit the original inode's bounded attribute"));}
        Ok(b)
    }
    fn read(o:&Origin,digest:[u8;32],b:&[u8])->io::Result<Self>{
        let mut r=In(b);let step=Progress::read(r.byte()?)?;
        let stage=match r.byte()?{0=>None,1=>Some(Stage{slot:r.byte()?,directory:NativeObject::get(&mut r)?}),
            _=>return Err(invalid("unknown MOVE directory binding"))};
        let p=Self{stage,step};p.check(o)?;
        if p.bytes(digest)?.as_slice()!=b{return Err(invalid("unknown or mismatched original MOVE progress"));}
        Ok(p)
    }
}
pub(in crate::ordinary) struct MoveAllocation{pub original:Origin,pub seq:u64,pub previous:[u8;32],pub request:[u8;32],
    pub digest:[u8;32],pub bytes:Vec<u8>,pub stamp:fs::OriginalStamp,phase:Phase}
impl MoveAllocation{
    pub(in crate::ordinary) fn stage(&self)->Option<&Stage>{self.phase.stage.as_ref()}
    pub(in crate::ordinary) fn opened(&self)->bool{self.phase.step>=Progress::OPEN}
    pub(in crate::ordinary) fn retiring(&self)->bool{self.phase.step>=Progress::ARMED}
    pub(super) fn progress(&self)->Progress{self.phase.step}
    /// A previous process may have stopped just after SETXATTR. A visible
    /// flag alone does not license serving the target or unlinking the source.
    pub(in crate::ordinary) fn confirm_phase(&self,p:&crate::pool::lease::LocalPool)->io::Result<()>{
        p.system.confirm_record_sync(NAME,&self.stamp,self.bytes.len())?;
        p.system.file.sync_all()
    }
    /// No caller supplies a frontier, allocated ticket, origin, root or syscall
    /// handle. Mint and persist under the *same* real SYSTEM + both writer
    /// leases after the independently checked anonymous transfer.
    pub(super) fn reserve(prepared:&Prepared<'_>,op:OperationId)->io::Result<Origin>{
        prepared.under_original_writer(|p,s,old,new|{
            let src=prepared.source();s.admit_new_handoff(p,op,src.file(),src.volume(),new.id())?;
            let(ticket,root)=crate::ordinary::material::select_ticket(new,src.name())?;
            if root!=*prepared.root()||ticket==op||s.ticket_used(ticket){return Err(invalid("MOVE ticket or original target conflict"));}
            p.require_business_op(ticket)?;
            let original=Origin::select(op,prepared,ticket)?;
            if original.mode==Method::Transfer{
                transfer::check_private_paths(&old.root,&new.root,ticket)?;
                if old.root.entry_present(&original.quarantine_name())?{return Err(invalid("original source-private isolation name already occupied"));}
            }
            let(seq,previous)=s.frontier();
            Self::write_original(&p.system,p.id(),&original,seq+1,previous)?;Ok(original)
        })
    }
    fn write_original(sys:&HeldDir,pool:PoolId,o:&Origin,seq:u64,previous:[u8;32])->io::Result<()>{
        if seq==0{return Err(invalid("invalid initial MOVE sequence"));}
        let req=transfer::request(pool,o.uid,o.op,o.file,o.from,o.to,o.epoch)?;
        let(f,entity)=sys.anonymous_record()?;let mut b=MAGIC.to_vec();NativeObject{entity,birth:fs::Birth::read(&f)?}.put(&mut b);
        b.extend_from_slice(&pool.0);o.put(&mut b);b.extend_from_slice(&seq.to_be_bytes());b.extend_from_slice(&previous);b.extend_from_slice(&req);
        let digest=crate::hash::digest(&b)?;b.extend_from_slice(&digest);
        if b.len()>MAX{return Err(invalid("original MOVE exceeds bounded SYSTEM reservation"));}
        fs::initial_attempt_binding(&f,&Phase::default().bytes(digest)?)?;sys.add_record(f,NAME,&b)
    }
    pub(in crate::ordinary) fn from_bytes(sys:&HeldDir,pool:PoolId,stamp:fs::OriginalStamp,bytes:Vec<u8>)->io::Result<Self>{
        if !(MAGIC.len()+32..=MAX).contains(&bytes.len()){return Err(invalid("wrong MOVE reservation length"));}
        let (body,checksum)=bytes.split_at(bytes.len()-32);let digest=crate::hash::digest(body)?;
        if digest.as_slice()!=checksum{return Err(invalid("damaged native MOVE reservation"));}
        let mut r=In(body.strip_prefix(MAGIC).ok_or_else(||invalid("unknown native MOVE reservation"))?);
        if NativeObject::get(&mut r)?!=NativeObject::stamp(&stamp)||PoolId(r.id()?)!=pool{return Err(invalid("substituted MOVE reservation or foreign pool"));}
        let original=Origin::get(&mut r)?;let seq=r.u64()?;let previous=r.fixed()?;let request=r.fixed()?;r.end()?;
        if seq==0||transfer::request(pool,original.uid,original.op,original.file,original.from,original.to,original.epoch)?!=request{
            return Err(invalid("MOVE allocation request is not the original FILE, caller and two roots"));
        }
        let phase=Phase::read(&original,digest,&sys.read_attempt_binding(NAME,&stamp,MAX)?)?;
        Ok(Self{original,seq,previous,request,digest,bytes,stamp,phase})
    }
    pub(super) fn bind_fresh(&self,p:&crate::pool::lease::LocalPool,v:&crate::pool::lease::FileVolume<'_>,fresh:super::staging::Fresh)->io::Result<()>{
        if !matches!(p.access,crate::pool::lease::Access::Write)||!std::ptr::eq(p,v.pool)||self.phase!=Phase::default(){return Err(invalid("not an original unbound handoff writer"));}
        let(_,s)=p.snapshot()?;
        if s.decided(self.original.op).is_some()||s.pending.is_some()||!s.moving.as_ref().is_some_and(|a|a.digest==self.digest&&a.stamp==self.stamp){
            return Err(invalid("cannot bind a different or already decided handoff"));}
        let next=Phase{stage:Some(fresh.witness(v,self)?),step:Progress::STAGING};next.check(&self.original)?;
        p.system.bind_attempt(NAME,&self.stamp,MAX,&self.phase.bytes(self.digest)?,&next.bytes(self.digest)?)
    }
    pub(super) fn confirm_opened(&self,p:&crate::pool::lease::LocalPool,v:&crate::pool::lease::FileVolume<'_>,verified:&super::placed::Reopened,
        after_set:impl FnOnce()->io::Result<()>)->io::Result<()>{
        if !matches!(p.access,crate::pool::lease::Access::Write)||!std::ptr::eq(v.pool,p){return Err(invalid("not the selected target writer"));}
        let(_,s)=p.snapshot()?;let(m,a)=s.confirmed_handoff(self.original.op)?;
        if a.digest!=self.digest||a.stamp!=self.stamp||a.opened(){return Err(invalid("different or already applied original handoff"));}
        verified.verify_installed(v,m)?;v.root.file.sync_all()?;v.require_current()?;
        // Cross-parent RENAME can be visible after a failure before either
        // parent sync. A transferred independently copied target needs no
        // source to establish its availability; a rename needs BOTH parents.
        if m.original.mode==Method::Rename{
            let old=p.ordinary(m.original.from)?;super::placed::root(&old,&m.original.src_root)?;
            if !old.root.same_mount(&v.root)?{return Err(invalid("original rename mount differs at completion"));}
            let name=std::ffi::CString::new(m.original.name.as_bytes()).map_err(|_|invalid("invalid original name"))?;
            old.root.confirm_absence(&name)?;v.root.file.sync_all()?;old.require_current()?;verified.verify_installed(v,m)?;
        }
        let next=Phase{stage:a.phase.stage.clone(),step:Progress::OPEN};next.check(&a.original)?;
        p.system.bind_attempt_at(NAME,&a.stamp,MAX,&a.phase.bytes(a.digest)?,&next.bytes(a.digest)?,after_set)
    }
    pub(super) fn begin_verified_retirement(&self,p:&crate::pool::lease::LocalPool,old:&crate::pool::lease::FileVolume<'_>,
        new:&crate::pool::lease::FileVolume<'_>,source:&super::retire::VerifiedSource,dest:&super::placed::Reopened,
        after_set:impl FnOnce()->io::Result<()>)->io::Result<()>{
        if !matches!(p.access,crate::pool::lease::Access::Write)||!std::ptr::eq(p,old.pool)||!std::ptr::eq(p,new.pool){
            return Err(invalid("not one actual original retirement writer"));}
        let(_,s)=p.snapshot()?;let(m,a)=s.confirmed_handoff(self.original.op)?;
        if a.stamp!=self.stamp||a.digest!=self.digest||!a.opened()||a.retiring()||m.original.mode!=Method::Transfer||!source.whole_at_start(){
            return Err(invalid("no complete verified, exclusively held original to start retirement"));}
        source.require(old,m)?;dest.verify_installed(new,m)?;
        let next=Phase{stage:a.phase.stage.clone(),step:Progress::ARMED};next.check(&a.original)?;
        p.system.bind_attempt_at(NAME,&a.stamp,MAX,&a.phase.bytes(a.digest)?,&next.bytes(a.digest)?,after_set)
    }
    pub(super) fn advance_isolated(&self,p:&crate::pool::lease::LocalPool,old:&crate::pool::lease::FileVolume<'_>,
        new:&crate::pool::lease::FileVolume<'_>,g:&super::retire::VerifiedSource,dst:&super::placed::Reopened,
        step:Progress,after_set:impl FnOnce()->io::Result<()>)->io::Result<()>{
        if !matches!(p.access,crate::pool::lease::Access::Write)||!std::ptr::eq(p,old.pool)||!std::ptr::eq(p,new.pool){
            return Err(invalid("not one original isolated retirement writer"));}
        let(_,s)=p.snapshot()?;let(m,a)=s.confirmed_handoff(self.original.op)?;
        if a.digest!=self.digest||a.stamp!=self.stamp||m.original.mode!=Method::Transfer||
            !a.progress().allows_next(m.original.role_count(),step){
            return Err(invalid("source retirement progress is not the next original role"));}
        g.require_next(old,m,step)?;dst.verify_installed(new,m)?;
        let next=Phase{stage:a.phase.stage.clone(),step};next.check(&a.original)?;
        p.system.bind_attempt_at(NAME,&a.stamp,MAX,&a.phase.bytes(a.digest)?,&next.bytes(a.digest)?,after_set)
    }
    pub(in crate::ordinary) fn matches(&self,m:&Moved)->bool{
        self.seq==m.seq&&self.request==m.request&&self.previous==m.previous&&self.digest==m.allocation&&self.original==m.original&&
        match (&self.phase.stage,m.stage){(None,None)=>self.original.mode==Method::Rename,
            (Some(s),Some(n))=>self.original.mode==Method::Transfer&&s.slot==n&&s.directory==m.target.directory,_=>false}
    }
}

#[cfg(test)]
mod progress_tests{
    use super::Progress as P;
    #[test]
    fn only_the_original_final_leaf_can_precede_the_checked_empty_quarantine(){
        let (raw3,incr4)=(P::leaf(2).unwrap(),P::leaf(3).unwrap());
        assert!(raw3.allows_next(3,P::EMPTY));assert!(!raw3.allows_next(3,incr4));
        assert!(raw3.allows_next(4,incr4));assert!(!raw3.allows_next(4,P::EMPTY));
        assert!(!incr4.allows_next(3,P::EMPTY));assert!(incr4.allows_next(4,P::EMPTY));
        assert!(!P::ARMED.allows_next(3,P::EMPTY));assert!(!P::EMPTY.allows_next(3,P::EMPTY));
    }
}
