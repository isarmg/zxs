//! No named material, overwrite, operation decision or source retirement at
//! this stage. Same-mount rename has no copy fallback; different mounts
//! prepare independently verified unnamed originals on the selected target.
use super::{source::{Bytes,Leaf,Verified},staging::Existing};
use crate::{OperationId,PoolId,VolumeId,fs::{self,HeldDir,invalid,stream},ordinary::{material,wire::NativeObject},
    pool::lease::{Access,FileVolume,LocalPool}};
use crate::ordinary::journal::Journal;
use std::{fs::File,io,path::{Path,PathBuf}};

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) enum Method{Rename,Transfer}
/// This identity must still match a real authenticated registration before a
/// name can be reserved. In particular the recycle role never passes ordinary().
fn vacant(s:&Journal,v:&FileVolume<'_>,name:&str)->io::Result<()>{
    if s.named(v.id(),name).is_some()||s.allocated.as_ref().is_some_and(|a|a.volume==v.id()&&a.name==name){
        return Err(invalid("the selected target name is already decided or reserved by an original CREATE"));
    }
    material::vacant_for_move(v,name)
}
pub(super) struct Destination<'s>{source:&'s Verified,system:PathBuf,system_origin:NativeObject,pub pool:PoolId,pub volume:VolumeId,pub root:NativeObject,pub mode:Method,held:HeldDir}
impl<'s> Destination<'s>{
    pub(super) fn select(system:&Path,src:&'s Verified,to:VolumeId)->io::Result<Self>{
        let(p,_,s)=LocalPool::with_snapshot(system,Access::Incoming)?;
        if p.id()!=src.pool()||to==src.volume(){return Err(invalid("MOVE requires two distinct original ordinary volume registrations"));}
        if !s.clean_initial_handoff(src.file()){return Err(invalid("initial clean formal FILE required for transfer"));}
        let old=p.ordinary(src.volume())?;src.require(&p,&s,&old)?;
        let new=p.ordinary(to)?;vacant(&s,&new,src.name())?;
        let root=NativeObject{entity:new.root.entity,birth:fs::Birth::read(&new.root.file)?};
        if &root==src.source_root(){return Err(invalid("different volume IDs cannot select the same native root"));}
        let mode=if src.same_mount(&new.root)?{
            if root.entity.device!=src.source_root().entity.device||root.birth.filesystem()!=src.source_root().birth.filesystem(){
                return Err(invalid("one mount cannot have incompatible original filesystem identities"));
            }
            Method::Rename
        }else{Method::Transfer};
        // A new open, not dup of the flocked registration FD: long copy holds
        // no SYSTEM or volume-wide lease. Its real incarnation is checked now
        // and again whenever the caller re-enters the original publication.
        let held=HeldDir::open(&new.root.path)?;
        let system_origin=NativeObject{entity:p.system.entity,birth:fs::Birth::read(&p.system.file)?};
        let result=Self{source:src,system:p.system.path.clone(),system_origin,pool:p.id(),volume:to,root,mode,held};result.require(&p,&new)?;
        src.require(&p,&s,&old)?;Ok(result)
    }
    pub(super) fn require(&self,p:&LocalPool,actual:&FileVolume<'_>)->io::Result<()>{
        if p.id()!=self.pool||p.system.entity!=self.system_origin.entity||fs::Birth::read(&p.system.file)?!=self.system_origin.birth||actual.id()!=self.volume||self.held.entity!=self.root.entity||actual.root.entity!=self.root.entity||
            self.held.path!=actual.root.path||!self.held.same_mount(&actual.root)?||
            fs::Birth::read(&self.held.file)?!=self.root.birth||fs::Birth::read(&actual.root.file)?!=self.root.birth{
            return Err(invalid("not the same registered handoff target"));
        }
        self.held.require_named()?;actual.require_current()
    }
    fn at_vacant<T>(&self,access:Access,use_cut:impl FnOnce(&LocalPool,&Journal,&FileVolume<'_>,&FileVolume<'_>)->io::Result<T>)->io::Result<T>{
        let(p,_,s)=LocalPool::with_snapshot(&self.system,access)?;let src=self.source;
        let old=p.ordinary(src.volume())?;let new=p.ordinary(self.volume)?;
        src.require(&p,&s,&old)?;self.require(&p,&new)?;vacant(&s,&new,src.name())?;
        src.require(&p,&s,&old)?;self.require(&p,&new)?;use_cut(&p,&s,&old,&new)
    }
    pub(super) fn prepare(self)->io::Result<Prepared<'s>>{self.prepare_after(||Ok(()))}
    pub(super) fn for_operation(self,op:OperationId)->io::Result<Prepared<'s>>{self.prepare_original(Some(op),||Ok(()))}
    fn prepare_after(self,begin:impl FnOnce()->io::Result<()>)->io::Result<Prepared<'s>>{self.prepare_original(None,begin)}
    fn prepare_original(self,op:Option<OperationId>,begin:impl FnOnce()->io::Result<()>)->io::Result<Prepared<'s>>{
        // Selection may have been held across a slow call. Check the exact
        // current source, target vacancy/role and SYSTEM again before allocating.
        let existing=self.at_vacant(Access::Incoming,|p,s,old,new|{
            if let Some(a)=&s.moving{
                if Some(a.original.op)!=op||a.original.uid!=fs::local_uid()||s.decided(a.original.op).is_some(){return Err(invalid("another original movement already occupies this source or frontier"));}
                if a.original.mode==Method::Transfer{
                    super::transfer::check_private_paths(&old.root,&new.root,a.original.ticket)?;
                    if old.root.entry_present(&a.original.quarantine_name())?{return Err(invalid("source isolation name occupied before transfer"));}
                }
                let sealed=s.pending.as_ref().map(|i|i.frame.moved()).transpose()?;
                Existing::capture(self.source,&self,new,a,sealed)
            }else{
                if let Some(op)=op{s.admit_new_handoff(p,op,self.source.file(),self.source.volume(),new.id())?;}
                Ok(None)
            }
        })?;
        if let Some(ref x)=existing{x.verify()?;}begin()?;
        let src=self.source;
        if src.pool()!=self.pool||src.volume()==self.volume{return Err(invalid("foreign MOVE source"));}
        self.held.require_named()?;
        if self.mode==Method::Rename{
            if !src.same_mount(&self.held)?{return Err(invalid("original same-mount rename boundary changed"));}
            return Ok(Prepared{dest:self,files:Vec::new(),existing});
        }
        if src.same_mount(&self.held)?{return Err(invalid("original cross-mount transfer boundary changed"));}
        // Allocate all physical regular-file sizes before transferring any
        // payload. Zero-length roles still get an actual empty native inode.
        let mut allocated=Vec::with_capacity(src.objects().len());
        for i in src.objects(){
            if existing.as_ref().is_some_and(|known|known.contains(i.kind)){continue;}
            let (file,_)=self.held.anonymous_record()?;
            if i.len!=0{stream::allocate_stream(&file,i.len)?;}
            let stamp=stream::anonymous_stamp(&file)?;
            if stamp.stat.size()!=i.len||stamp.birth.filesystem()!=self.root.birth.filesystem(){
                return Err(invalid("target does not hold the actual reserved physical size on its original filesystem"));
            }
            allocated.push((i.clone(),file));
        }
        let mut files=Vec::with_capacity(allocated.len());
        for (part,f) in allocated{
            src.stream(part.kind,|at,b|stream::write_at(&f,b,at))?;
            f.sync_all()?;let stamp=stream::anonymous_stamp(&f)?;
            if hash_unnamed(&f,&stamp)?!=part.digest{return Err(invalid("target physical data differs from checked source"));}
            files.push(Anonymous{part,file:f,stamp});
        }
        self.held.require_named()?;
        Ok(Prepared{dest:self,files,existing})
    }
}
fn hash_unnamed(f:&File,original:&fs::OriginalStamp)->io::Result<[u8;32]>{
    if stream::anonymous_stamp(f)?!=*original{return Err(invalid("unnamed target changed at transfer boundary"));}
    let len=original.stat.size();let mut h=crate::hash::Sha256::new();let mut at=0;let mut buf=vec![0;len.min(1024*1024) as usize];
    while at<len{let n=(len-at).min(buf.len() as u64)as usize;stream::read_at(f,&mut buf[..n],at)?;
        h.update(&buf[..n])?;at+=n as u64;}
    if stream::anonymous_stamp(f)?!=*original{return Err(invalid("unnamed target changed during complete readback"));}Ok(h.finish())
}
pub(super) struct Anonymous{pub part:Bytes,pub file:File,pub stamp:fs::OriginalStamp}
pub(super) struct Prepared<'s>{dest:Destination<'s>,files:Vec<Anonymous>,existing:Option<Existing>}
impl Prepared<'_>{
    pub(super) fn dest_pool(&self)->PoolId{self.dest.pool}
    pub(super) fn source(&self)->&Verified{self.dest.source}
    pub(super) fn volume(&self)->VolumeId{self.dest.volume}
    pub(super) fn root(&self)->&NativeObject{&self.dest.root}
    pub(super) fn method(&self)->Method{self.dest.mode}
    pub(super) fn under_original_writer<T>(&self,use_cut:impl FnOnce(&LocalPool,&Journal,&FileVolume<'_>,&FileVolume<'_>)->io::Result<T>)->io::Result<T>{
        self.at_vacant_cut(Access::Write,use_cut)
    }
    pub(super) fn part(&self,which:Leaf)->io::Result<&Anonymous>{
        self.files.iter().find(|p|p.part.kind==which).ok_or_else(||invalid("no authentic unnamed transfer operand for requested role"))
    }
    /// Reacquire SYSTEM and BOTH concrete roots from a single fresh snapshot.
    /// The closure cannot supply another FILE or mix older captured catalogs.
    pub(super) fn at_vacant_cut<T>(&self,access:Access,use_cut:impl FnOnce(&LocalPool,&Journal,&FileVolume<'_>,&FileVolume<'_>)->io::Result<T>)->io::Result<T>{
        self.dest.at_vacant(access,|p,s,old,new|{
            self.require_material(new)?;
            if let Some(existing)=&self.existing{existing.require(new,s.moving.as_ref().ok_or_else(||invalid("original staged MOVE reservation disappeared"))?)?;}
            use_cut(p,s,old,new)
        })
    }
    /// Changes only the private operand of this *same* reserved original.
    /// link_stream never replaces; each first name follows its durable issued
    /// directory, each existing role is authenticated and not reallocated.
    pub(super) fn staged_decision(&self,op:OperationId,hook:&mut impl FnMut(crate::ordinary::Cut)->io::Result<()>)
        ->io::Result<(super::transfer::Moved,Option<(super::placed::TargetInput,std::ffi::CString)>)>{
        use super::{placed::{self,Reopened},transfer::{Moved,Sites},staging};
        self.under_original_writer(|p,s,old,new|{
            let a=s.moving.as_ref().ok_or_else(||invalid("original MOVE reservation absent"))?;
            if a.original.op!=op||a.opened()||s.decided(op).is_some(){return Err(invalid("different MOVE or already decided"));}
            a.original.require(self.source(),self)?;
            if a.original.mode==Method::Transfer&&old.root.entry_present(&a.original.quarantine_name())?{
                return Err(invalid("source private name now occupied; no further target staging"));}
            p.system.confirm_record_sync(crate::ordinary::allocation::NAME,&a.stamp,a.bytes.len())?;p.system.file.sync_all()?;
            let(c,f,d)=self.source().originals();
            if a.original.mode==Method::Rename{
                let m=Moved::seal(p.id(),a.seq,a.previous,a.original.clone(),a.digest,None,Sites::birthplace(c,f,d))?;
                m.require_history(c,f,d)?;if !a.matches(&m){return Err(invalid("different same-mount reservation"));}return Ok((m,None));
            }
            if a.stage().is_none(){
                let fresh=staging::Fresh::under_writer(p,new,a,hook)?;a.bind_fresh(p,new,fresh)?;
            }
            let(_,now)=p.snapshot()?;let a=now.moving.as_ref().ok_or_else(||invalid("original MOVE reservation lost during stage bind"))?;
            a.original.require(self.source(),self)?;
            let st=a.stage().ok_or_else(||invalid("stage not bound"))?;let dir=staging::bound_dir(new,a,st)?;
            let mut selected=std::collections::BTreeMap::new();
            // The pre-copy ALL named-prefix proof is for this actual stage.
            if let Some(x)=&self.existing{x.require(new,a)?;}else{dir.only(None)?;}
            for role in self.source().objects(){
                let actual=if let Some(pin)=self.existing.as_ref().and_then(|e|e.pin(role.kind)){
                    pin.named(&dir,&role.name)?;NativeObject::stamp(&pin.stamp)
                }else{
                    if now.pending.is_some(){return Err(invalid("original sealed role must already be present"));}
                    let a0=self.part(role.kind)?;
                    if stream::anonymous_stamp(&a0.file)?!=a0.stamp{return Err(invalid("anonymous target changed before name"));}
                    let n=NativeObject::stamp(&a0.stamp);fs::set_preparation_token(&a0.file,&placed::token(a.digest,role.kind,&n)?)?;
                    dir.link_stream(&role.name,&a0.file,&a0.stamp,role.len)?;
                    hook(crate::ordinary::Cut::MoveLeafLinked)?;n
                };
                if selected.insert(role.kind,actual).is_some(){return Err(invalid("repeated MOVE role"));}
            }
            dir.file.sync_all()?;new.root.file.sync_all()?;
            let sites=staging::sites(&dir,&selected,&a.original)?;
            let m=Moved::seal(p.id(),a.seq,a.previous,a.original.clone(),a.digest,Some(st.slot),sites)?;
            m.require_history(c,f,d)?;if !a.matches(&m)||now.pending.as_ref().is_some_and(|i|i.frame.bytes()!=m.bytes){
                return Err(invalid("named targets differ from the original MOVE seal"));}
            let name=material::staging(a.original.ticket,st.slot);
            let input=Reopened::input(new,&name,&m,c,f,d)?;Ok((m,Some((input,name))))
        })
    }
    /// Naming consumes each previously anonymous stamp. On the later decision
    /// pass use the independently read-back *new named* holder and all live
    /// source pins, not the pre-link anonymous ctime or untrusted path.
    pub(super) fn original_writer_after_naming<T>(&self,apply:impl FnOnce(&LocalPool,&Journal,&FileVolume<'_>,&FileVolume<'_>)->io::Result<T>)->io::Result<T>{
        self.dest.at_vacant(Access::Write,apply)
    }
    fn require_material(&self,v:&FileVolume<'_>)->io::Result<()>{
        let src=self.source();let p=v.pool;
        if src.pool()!=p.id()||src.volume()==v.id()||src.same_mount(&v.root)?!=(self.dest.mode==Method::Rename){
            return Err(invalid("original transfer method or source changed"));
        }
        if self.dest.mode==Method::Rename{
            if !self.files.is_empty(){return Err(invalid("a rename cannot claim staged content"));}
        }else{
            if self.files.len()+self.existing.as_ref().map_or(0,Existing::len)!=src.objects().len(){return Err(invalid("incomplete original target transfer set"));}
            for s in src.objects(){
                if let Some(p)=self.existing.as_ref().and_then(|e|e.pin(s.kind)){p.unchanged()?;continue;}
                let n=self.part(s.kind)?;if &n.part!=s||stream::anonymous_stamp(&n.file)?!=n.stamp{
                return Err(invalid("prepared target file not the frozen source's verified corresponding role"));}}
        }
        Ok(())
    }
}
#[cfg(test)]
#[path="../../../tests/unit/custody_destination.rs"]
mod tests;
