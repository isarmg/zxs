//! A handoff uses the existing exclusive SYSTEM operand. Its immutable bytes
//! reserve the original FILE and both roots. Its bounded progress attribute
//! cannot establish a decision or reconstruct a missing file.
use super::{destination::{Method,Prepared},transfer::{self,Moved,Origin}};
use crate::{OperationId,PoolId,fs::{self,HeldDir,invalid},ordinary::{allocation::{NAME,SLOTS},wire::{In,NativeObject}}};
use std::io;
pub(in crate::ordinary) const MAGIC:&[u8]=b"ZXS/ORIGINAL-FILE-HANDOFF\0";
pub(in crate::ordinary) const MAX:usize=2048;
const _:()=assert!(MAGIC.len()+16+1+fs::MAX_BIRTH+16+transfer::MAX_ORIGIN+8+32+32+32<=MAX);

#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Stage{pub slot:u8,pub directory:NativeObject}
#[derive(Clone,Debug,Eq,PartialEq,Default)]
struct Phase{stage:Option<Stage>,step:Progress}
/// One bounded progress value on the original operand, not another decision.
/// 2 authorizes only isolation; 3..=6 authorize exactly one named source
/// role each, 8 alone is the fifth. Only 7 proves the checked, synced original quarantine was empty.
#[derive(Clone,Copy,Debug,Default,Eq,PartialEq)]
pub(super) struct Progress(u8);
impl Progress{
    const STAGING:Self=Self(0);pub(super) const OPEN:Self=Self(1);
    pub(super) const ARMED:Self=Self(2);pub(super) const EMPTY:Self=Self(7);
    pub(super) fn leaf(i:usize)->io::Result<Self>{
        match i{0..=3=>Ok(Self(3+i as u8)),4=>Ok(Self(8)),_=>Err(invalid("unknown original retirement role"))}
    }
    pub(super) fn authorized(self)->Option<usize>{match self.0{3..=6=>Some((self.0-3) as usize),8=>Some(4),_=>None}}
    pub(super) fn opened(self)->bool{self!=Self::STAGING}
    pub(super) fn retiring(self)->bool{self==Self::ARMED||self==Self::EMPTY||self.authorized().is_some()}
    pub(super) fn business_possible(self)->bool{!self.retiring()||self==Self::ARMED}
    pub(super) fn past_leaf(self,i:usize)->bool{self==Self::EMPTY||self.authorized().is_some_and(|j|j>i)}
    pub(super) fn allows_next(self,count:usize,next:Self)->bool{
        if !(3..=5).contains(&count){return false;}
        let i=if self==Self::ARMED{0}else if let Some(i)=self.authorized(){i+1}else{return false;};
        if i==count{return next==Self::EMPTY;}i<count&&next.authorized()==Some(i)
    }
    fn read(n:u8)->io::Result<Self>{if n<=8{Ok(Self(n))}else{Err(invalid("unknown MOVE progress"))}}
}
impl Phase{
    fn check(&self,o:&Origin)->io::Result<()>{
        if self.step.authorized().is_some_and(|i|i>=o.role_count())||self.step.retiring()&&(o.mode!=Method::Transfer||self.stage.is_none())||self.step.opened()&&o.mode==Method::Transfer&&self.stage.is_none()||self.stage.as_ref().is_some_and(|s|
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
    pub(in crate::ordinary) fn opened(&self)->bool{self.phase.step.opened()}
    pub(in crate::ordinary) fn retiring(&self)->bool{self.phase.step.retiring()}
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
    pub(super) fn reserve(prepared:&Prepared<'_>,requested:super::command::Request)->io::Result<Origin>{
        let op=requested.op;if prepared.move_request(op)!=requested{return Err(invalid("prepared FILE or ordinary parent differs from this MOVE request"));}
        prepared.under_original_writer(|p,s,old,new|{
            let src=prepared.source();s.admit_handoff_at(p,op,src.file(),old.address(),new.address())?;
            let(ticket,root)=crate::ordinary::material::select_at(new,src.name(),src.file())?;
            if root!=*prepared.root()||ticket==op||s.ticket_used(ticket){return Err(invalid("MOVE ticket or original target conflict"));}
            p.require_business_op(ticket)?;
            let original=Origin::select(op,prepared,ticket)?;
            if original.request_digest(p.id())?!=requested.digest(p)?{return Err(invalid("different original parent-bound MOVE before reservation"));}
            s.custody_parent_projection(p,&original)?;
            if original.mode==Method::Transfer{
                transfer::check_private_paths(old.directory(),new.directory(),ticket)?;
                if old.directory().entry_present(&original.quarantine_name())?{return Err(invalid("original source-private isolation name already occupied"));}
            }
            let(seq,previous)=s.frontier();
            Self::write_original(&p.system,p.id(),&original,seq+1,previous)?;Ok(original)
        })
    }
    /// No incoming Origin argument. Issue from these very anonymous held
    /// bytes inside both native writer leases; or return the identical
    /// already issued original (including the one stored deletion time).
    /// Private to the paired lifecycle implementation, never a public action.
    pub(super) fn reserve_lifecycle(system:&std::path::Path,ready:&crate::pool::lease::recycle::incoming::PreparedIncoming<'_>)->io::Result<Origin>{
        ready.with_writer(system,|turn|{
            let o=super::incoming::proposed(&turn)?;let(p,_)=turn.require()?;let(_,s)=p.snapshot()?;
            let mut prior=None;let seq=if let Some(a)=s.moving.as_ref(){
                if a.original!=o||a.request!=o.request_digest(p.id())?||a.opened()||s.pending.is_some(){
                    return Err(invalid("not the same original reservation"));}
                prior=a.stage().cloned();a.seq
            }else{
                s.check_new_lifecycle_origin(&p.journal_registration(),&o)?;let(count,previous)=s.frontier();let seq=count+1;
                turn.require()?;Self::write_original(&p.system,p.id(),&o,seq,previous)?;seq
            };
            // The old grant is stale after write_original: read the actual
            // native envelope/digest, original inode, phase, and frontier.
            let(_,now)=p.snapshot()?;let a=now.moving.as_ref().ok_or_else(||invalid("missing original lifecycle reservation"))?;
            let(count,previous)=now.frontier();
            if a.original!=o||a.request!=o.request_digest(p.id())?||a.seq!=seq||seq!=count+1||a.previous!=previous||
                a.opened()||a.stage()!=prior.as_ref()||now.pending.is_some()||now.decided(o.op).is_some(){
                return Err(invalid("wrong original reservation readback"));}a.confirm_phase(p)?;Ok(o)
        })
    }
    fn write_original(sys:&HeldDir,pool:PoolId,o:&Origin,seq:u64,previous:[u8;32])->io::Result<()>{
        if seq==0{return Err(invalid("invalid initial MOVE sequence"));}
        let req=o.request_digest(pool)?;
        let(f,entity)=sys.anonymous_record()?;let mut b=MAGIC.to_vec();NativeObject{entity,birth:fs::Birth::read(&f)?}.put(&mut b);
        b.extend_from_slice(&pool.0);o.put(&mut b)?;b.extend_from_slice(&seq.to_be_bytes());b.extend_from_slice(&previous);b.extend_from_slice(&req);
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
        if seq==0||original.request_digest(pool)?!=request{
            return Err(invalid("MOVE allocation request is not the original FILE, caller and two roots"));
        }
        let phase=Phase::read(&original,digest,&sys.read_attempt_binding(NAME,&stamp,MAX)?)?;
        Ok(Self{original,seq,previous,request,digest,bytes,stamp,phase})
    }
    pub(super) fn bind_fresh(&self,p:&crate::pool::lease::LocalPool,v:&crate::ordinary::OrdinaryParentLease<'_,'_>,fresh:super::staging::Fresh)->io::Result<()>{
        if !matches!(p.access,crate::pool::lease::Access::Write)||!std::ptr::eq(p,v.volume().pool)||self.phase!=Phase::default(){return Err(invalid("not an original unbound handoff writer"));}
        let(_,s)=p.snapshot()?;
        if s.decided(self.original.op).is_some()||s.pending.is_some()||!s.moving.as_ref().is_some_and(|a|a.digest==self.digest&&a.stamp==self.stamp){
            return Err(invalid("cannot bind a different or already decided handoff"));}
        let next=Phase{stage:Some(fresh.witness(v,self)?),step:Progress::STAGING};next.check(&self.original)?;
        p.system.bind_attempt(NAME,&self.stamp,MAX,&self.phase.bytes(self.digest)?,&next.bytes(self.digest)?)
    }
    pub(super) fn bind_created(&self,t:&crate::pool::lease::recycle::incoming::FreshStageTurn<'_,'_>)->io::Result<()>{
        let(p,_,slot,entity,birth)=t.require()?;let(_,s)=p.snapshot()?;
        if !matches!(p.access,crate::pool::lease::Access::Write)||self.phase!=Phase::default()||
            self.original.purpose==super::purpose::Purpose::Ordinary||s.pending.is_some()||s.decided(self.original.op).is_some()||
            !s.moving.as_ref().is_some_and(|a|a.digest==self.digest&&a.stamp==self.stamp&&a.original==self.original){
            return Err(invalid("not the live unbound dedicated reservation"));}
        let st=Stage{slot,directory:NativeObject{entity,birth}};
        let next=Phase{stage:Some(st.clone()),step:Progress::STAGING};next.check(&self.original)?;
        t.require()?;p.system.bind_attempt(NAME,&self.stamp,MAX,&self.phase.bytes(self.digest)?,&next.bytes(self.digest)?)?;
        let(_,now)=p.snapshot()?;let a=now.moving.as_ref().filter(|a|a.digest==self.digest&&a.original==self.original&&
            a.stamp.birth==self.stamp.birth&&a.stamp.stat.entity()==self.stamp.stat.entity()&&a.phase==next)
            .ok_or_else(||invalid("private directory binding does not reopen on the original reservation"))?;
        if now.decided(self.original.op).is_some()||now.pending.is_some(){return Err(invalid("unexpected decision during target binding"));}a.confirm_phase(p)
    }
    pub(super) fn confirm_opened(&self,p:&crate::pool::lease::LocalPool,v:&crate::ordinary::OrdinaryParentLease<'_,'_>,verified:&super::placed::Reopened,
        after_set:impl FnOnce()->io::Result<()>)->io::Result<()>{
        if !matches!(p.access,crate::pool::lease::Access::Write)||!std::ptr::eq(v.volume().pool,p){return Err(invalid("not the selected target writer"));}
        let(_,s)=p.snapshot()?;let(m,a)=s.confirmed_handoff(self.original.op)?;
        if a.digest!=self.digest||a.stamp!=self.stamp||a.opened(){return Err(invalid("different or already applied original handoff"));}
        verified.verify_installed(v,m)?;v.directory().file.sync_all()?;v.require_at(&s)?;
        // Cross-parent RENAME can be visible after a failure before either
        // parent sync. A transferred independently copied target needs no
        // source to establish its availability; a rename needs BOTH parents.
        if m.original.mode==Method::Rename{
            let old=m.original.ordinary_at(p,&s,true)?;
            if !old.directory().same_mount(v.directory())?{return Err(invalid("original rename mount differs at completion"));}
            let name=crate::ordinary::storage_name::file_leaf(&m.original.name,m.original.file)?;
            old.directory().confirm_absence(&name)?;v.directory().file.sync_all()?;old.require()?;verified.verify_installed(v,m)?;
        }
        let next=Phase{stage:a.phase.stage.clone(),step:Progress::OPEN};next.check(&a.original)?;
        p.system.bind_attempt_at(NAME,&a.stamp,MAX,&a.phase.bytes(a.digest)?,&next.bytes(a.digest)?,after_set)
    }
    pub(super) fn confirm_lifecycle_target(&self,p:&crate::pool::lease::LocalPool,g:&super::admission::LifecycleGrant<'_>,end:&crate::pool::lease::recycle::CustodyEnd<'_,'_>,checked:&super::target::Checked,after:impl FnOnce()->io::Result<()>)->io::Result<()>{
        let s=g.live_cut(p)?;let(m,a)=s.confirmed_handoff(g.request().op)?;
        if !matches!(p.access,crate::pool::lease::Access::Write)||!end.belongs(g,super::admission::Side::Target)||!checked.installed()||
            m!=&checked.goal.frame||a.stamp!=self.stamp||a.digest!=self.digest||a.opened(){return Err(invalid("not the complete installed original target writer"));}
        end.sync_installed(checked)?;
        let next=Phase{stage:a.phase.stage.clone(),step:Progress::OPEN};next.check(&a.original)?;
        p.system.bind_attempt_at(NAME,&a.stamp,MAX,&a.phase.bytes(a.digest)?,&next.bytes(a.digest)?,after)
    }
    pub(super) fn step_lifecycle(&self,p:&crate::pool::lease::LocalPool,g:&super::admission::LifecycleGrant<'_>,
        old:&crate::pool::lease::recycle::CustodyEnd<'_,'_>,new:&crate::pool::lease::recycle::CustodyEnd<'_,'_>,
        source:&super::LifecycleRetiring,dst:&super::target::Checked,wanted:Progress,after:impl FnOnce()->io::Result<()>)->io::Result<()>{
        let s=g.live_cut(p)?;let(m,a)=s.confirmed_handoff(g.request().op)?;
        if a.digest!=self.digest||a.stamp!=self.stamp||m!=&dst.goal.frame||!a.opened()||
            !(wanted==Progress::ARMED&&a.progress()==Progress::OPEN||a.progress().allows_next(m.original.role_count(),wanted)){
            return Err(invalid("wrong exact dedicated retirement transition"));}
        source.can_advance(p,g,old,new,dst,wanted)?;
        let next=Phase{stage:a.phase.stage.clone(),step:wanted};next.check(&a.original)?;
        p.system.bind_attempt_at(NAME,&a.stamp,MAX,&a.phase.bytes(a.digest)?,&next.bytes(a.digest)?,after)
    }
    pub(super) fn begin_verified_retirement(&self,p:&crate::pool::lease::LocalPool,old:&crate::ordinary::OrdinaryParentLease<'_,'_>,
        new:&crate::ordinary::OrdinaryParentLease<'_,'_>,source:&super::retire::VerifiedSource,dest:&super::placed::Reopened,
        after_set:impl FnOnce()->io::Result<()>)->io::Result<()>{
        if !matches!(p.access,crate::pool::lease::Access::Write)||!std::ptr::eq(p,old.volume().pool)||!std::ptr::eq(p,new.volume().pool){
            return Err(invalid("not one actual original retirement writer"));}
        let(_,s)=p.snapshot()?;let(m,a)=s.confirmed_handoff(self.original.op)?;
        if a.stamp!=self.stamp||a.digest!=self.digest||!a.opened()||a.retiring()||m.original.mode!=Method::Transfer||!source.whole_at_start(){
            return Err(invalid("no complete verified, exclusively held original to start retirement"));}
        source.require(old,m)?;dest.verify_installed(new,m)?;
        let next=Phase{stage:a.phase.stage.clone(),step:Progress::ARMED};next.check(&a.original)?;
        p.system.bind_attempt_at(NAME,&a.stamp,MAX,&a.phase.bytes(a.digest)?,&next.bytes(a.digest)?,after_set)
    }
    pub(super) fn advance_isolated(&self,p:&crate::pool::lease::LocalPool,old:&crate::ordinary::OrdinaryParentLease<'_,'_>,
        new:&crate::ordinary::OrdinaryParentLease<'_,'_>,g:&super::retire::VerifiedSource,dst:&super::placed::Reopened,
        step:Progress,after_set:impl FnOnce()->io::Result<()>)->io::Result<()>{
        if !matches!(p.access,crate::pool::lease::Access::Write)||!std::ptr::eq(p,old.volume().pool)||!std::ptr::eq(p,new.volume().pool){
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
#[path = "../../../tests/unit/attached/ordinary/custody/allocation/progress_tests.rs"]
mod progress_tests;

#[cfg(test)]
#[path = "../../../tests/unit/attached/ordinary/custody/allocation/five_role_progress_tests.rs"]
mod five_role_progress_tests;
