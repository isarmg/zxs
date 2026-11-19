//! The material that will, or already does, implement one specific lifecycle
//! decision. A bounded frame, alone, is not proof the target bytes exist.
use super::{admission::{LifecycleGrant,Side},placed::{self,Reopened,TargetInput},purpose::Call,transfer::{ChildRoles,Moved}};
use crate::{fs::{HeldDir,invalid},ordinary::{delta::types::Delta,formal::types::Commit,wire::{self,Frame}},
    pool::lease::{LocalPool,recycle::CustodyEnd}};
use std::{ffi::CString,io};

#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) enum Site{Private,Installed}
#[derive(Clone)]
pub(crate) struct Goal{pub(super) frame:Moved,pub(super) site:Site}
pub(crate) struct Scan{goal:Goal,input:TargetInput}
pub(crate) struct Checked{pub(super) goal:Goal,pub(super) bytes:Reopened}
impl Goal{
    // Also invoked by the only R reader before it looks up a name. An old
    // receipt alone is no longer a read capability after its operand is gone.
    fn parts(&self,p:&LocalPool,g:&LifecycleGrant<'_>)->io::Result<(CString,Frame,Commit,ChildRoles<Delta>)>{
        let s=g.live_cut(p)?;let r=g.request();let m=&self.frame;
        let a=s.moving.as_ref().ok_or_else(||invalid("no active original target operand"))?;
        if r.call==Call::Move||m.original.op!=r.op||m.original.purpose.call()!=r.call||r.digest(p)?!=m.request||!a.matches(m)||
            s.pending.as_ref().is_some_and(|i|i.frame.bytes()!=m.bytes)||m.original.dst_root!=*g.root(Side::Target)||
            Moved::read(p.id(),&m.bytes,wire::envelope(p.id(),&m.bytes)?)?!=*m{return Err(invalid("target does not match the original reservation and exact frame"));}
        let(c,f,d)=if s.decided(r.op).is_some(){
            let(old,_)=s.confirmed_handoff(r.op)?;if old!=m{return Err(invalid("different confirmed target"));}
            super::finish::frozen(&s,m)?
        }else{
            if self.site!=Site::Private||a.opened(){return Err(invalid("an undecided target has no public install address"));}
            let c=s.file(r.id).ok_or_else(||invalid("missing original FILE"))?;
            let f=s.head(r.id).ok_or_else(||invalid("missing original FULL"))?;let d=s.actual_graph(r.id)?.children();
            m.require_at(s.placement(r.id)?,f,d)?;(c,f,d)
        };
        let name=match self.site{
            Site::Private=>{if a.opened(){return Err(invalid("opened target can never fall back to its stage"));}
                let st=a.stage().ok_or_else(||invalid("no signed target stage"))?;crate::ordinary::material::staging(m.original.ticket,st.slot)}
            Site::Installed=>g.name(Side::Target).to_owned()
        };Ok((name,c.clone(),f.clone(),d.map(Clone::clone)))
    }
    pub(crate) fn addresses(&self,p:&LocalPool,g:&LifecycleGrant<'_>)->io::Result<(CString,CString)>{
        let selected=self.name(p,g)?;let other=match self.site{
            Site::Private=>g.name(Side::Target).to_owned(),
            Site::Installed=>crate::ordinary::material::staging(self.frame.original.ticket,self.frame.stage.ok_or_else(||invalid("no exact private transfer address"))?),
        };Ok((selected,other))
    }
    pub(crate) fn namespace(&self,p:&LocalPool,g:&LifecycleGrant<'_>,role:crate::Role,names:&[Vec<u8>])->io::Result<()>{
        let s=g.live_cut(p)?;let(selected,other)=self.addresses(p,g)?;
        if names.iter().any(|n|n.as_slice()==other.to_bytes()){
            return Err(invalid("competing name at the original lifecycle destination"));}
        let o=&self.frame.original;
        match role{
            crate::Role::File=>{},
            crate::Role::Recycle=>{
                let mut accepted=std::collections::BTreeSet::new();let mut gaps=std::collections::BTreeMap::new();
                for old in s.moves().filter(|m|m.original.to==o.to){
                    if old.original.purpose.call()!=Call::Delete{return Err(invalid("unexpected non-deletion on dedicated target"));}
                    if let Some(slot)=old.stage{gaps.insert(old.original.ticket,slot);}
                    if s.placed(old.original.file).map(|m|m.digest)==Some(old.digest){
                        accepted.insert(old.original.purpose.leaves(old.original.file,old.original.op,&old.original.name)?.1.into_bytes());}
                }
                gaps.insert(o.ticket,self.frame.stage.ok_or_else(||invalid("no bound transfer slot"))?);
                for n in names{if n!=selected.to_bytes()&&!accepted.contains(n)&&!crate::ordinary::material::private_slot(n).is_some_and(|(t,i)|gaps.get(&t).is_some_and(|&b|i<b)){
                    return Err(invalid("unattributed entry on the original dedicated destination"));}}
            },
            _=>return Err(invalid("SYSTEM cannot hold a FILE lifecycle target"))
        }Ok(())
    }
    pub(crate) fn name(&self,p:&LocalPool,g:&LifecycleGrant<'_>)->io::Result<CString>{self.parts(p,g).map(|x|x.0)}
    pub(crate) fn start(&self,p:&LocalPool,g:&LifecycleGrant<'_>,end:&CustodyEnd<'_,'_>,dir:HeldDir)->io::Result<Scan>{
        if !end.belongs(g,Side::Target){return Err(invalid("target belongs to a different lifecycle decision"));}
        let(name,c,f,d)=self.parts(p,g)?;end.check_target_child(self,&dir)?;
        let m=&self.frame;
        crate::ordinary::material::verify_header_at(&dir,&c.create,&m.target.directory,&m.target.local)?;
        let rows=placed::roles(&c,&f,d.as_refs(),&m.target)?;
        for r in &rows{let _=placed::held_role(&dir,r,Some(m.allocation))?;}
        dir.share()?;end.check_target_child(self,&dir)?;
        let input=Reopened::input_lifecycle(p,end.id(),m,name,dir,&c,&f,d.as_refs())?;
        Ok(Scan{goal:self.clone(),input})
    }
}
impl Scan{
    pub(crate) fn verify(self)->io::Result<Checked>{Ok(Checked{goal:self.goal,bytes:Reopened::verify(self.input)?})}
}
impl Checked{
    pub(crate) fn installed(&self)->bool{self.goal.site==Site::Installed}
    pub(crate) fn install_names(&self,p:&LocalPool,g:&LifecycleGrant<'_>)->io::Result<(CString,CString,crate::Entity,crate::fs::Birth)>{
        let m=&self.goal.frame;let s=g.live_cut(p)?;let(now,a)=s.confirmed_handoff(g.request().op)?;
        if !matches!(p.access,crate::pool::lease::Access::Write)||self.installed()||a.opened()||m!=now{
            return Err(invalid("cannot install before the exact original decision or after opening"));}
        let(old,new)=self.goal.addresses(p,g)?;Ok((old,new,m.target.directory.entity,m.target.directory.birth.clone()))
    }
    pub(crate) fn goal(&self)->&Goal{&self.goal}
    pub(crate) fn require(&self,p:&LocalPool,g:&LifecycleGrant<'_>,end:&CustodyEnd<'_,'_>)->io::Result<()>{
        let name=self.goal.name(p,g)?;
        if !end.belongs(g,Side::Target){return Err(invalid("not the original target endpoint"));}
        self.bytes.require_lifecycle(end,&self.goal,&name)
    }
}
