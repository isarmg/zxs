//! Exclusive retirement of precisely the previous native FILE, never a
//! directory found by name or contents. No unscoped recycle FD is exported.
use super::{Address,SourceRetirement,VerifiedSource};
use super::super::{admission::{LifecycleGrant,Side},allocation::Progress,command::Request,finish::frozen,
    purpose::Call,target::Checked,transfer::Moved};
use crate::{fs::{self,HeldDir,invalid},ordinary::{self,Cut},pool::lease::{Access,LocalPool,recycle::CustodyEnd}};
use std::{ffi::CString,io,path::Path};
#[derive(Clone)]pub(crate) struct Previous{m:Moved}
pub(crate) struct Originals{key:Previous,source:SourceRetirement}
pub(crate) struct Retiring{key:Previous,source:VerifiedSource}
impl Previous{
    fn parts(&self,p:&LocalPool,g:&LifecycleGrant<'_>,end:&CustodyEnd<'_,'_>)->io::Result<(Progress,ordinary::journal::Journal)>{
        if g.request().call==Call::Move||!end.belongs(g,Side::Source)||!matches!(p.access,Access::Incoming|Access::Write){
            return Err(invalid("not the original dedicated retirement scope"));}
        let s=g.live_cut(p)?;let(m,a)=s.confirmed_handoff(g.request().op)?;
        if m!=&self.m||!a.opened()||m.original.src_root!=*g.root(Side::Source)||end.id()!=m.original.from{
            return Err(invalid("different original or unconfirmed installed target at cleanup"));}
        a.confirm_phase(p)?;Ok((a.progress(),s))
    }
    pub(crate) fn names(&self,p:&LocalPool,g:&LifecycleGrant<'_>,end:&CustodyEnd<'_,'_>)->io::Result<(CString,CString)>{
        let _=self.parts(p,g,end)?;Ok((g.name(Side::Source).to_owned(),self.m.original.quarantine_name()))
    }
    pub(crate) fn pin(&self,p:&LocalPool,g:&LifecycleGrant<'_>,end:&CustodyEnd<'_,'_>,live:Option<HeldDir>,private:Option<HeldDir>)->io::Result<Originals>{
        let(step,s)=self.parts(p,g,end)?;let m=&self.m;let(c,f,d)=frozen(&s,m)?;let prior=s.before_handoff(m)?;
        let sites=m.require_at(prior,f,d)?;
        let source=SourceRetirement::capture_selected(m,step,c,f,d,true,sites,prior.transfer(),live,private)?;
        Ok(Originals{key:self.clone(),source})
    }
}
fn verify_address(group:&SourceRetirement,business:Option<HeldDir>,isolated:Option<HeldDir>)->io::Result<()>{
    match (group.addr,business,isolated){
        (Address::Business,b,None)=>group.require_named_dir(b),
        (Address::Isolated,None,q)=>group.require_named_dir(q),
        (Address::EmptyGone,None,None)=>group.require_named_dir(None),
        _=>Err(invalid("unattributed replacement, alias or moved original at a retirement address"))
    }
}
impl Originals{
    pub(crate) fn matches(&self,live:Option<HeldDir>,private:Option<HeldDir>)->io::Result<()>{verify_address(&self.source,live,private)}
    fn verify(self,checked:&Checked)->io::Result<Retiring>{
        if !checked.installed()||checked.goal.frame!=self.key.m{return Err(invalid("retirement requires the completely reopened installed target"));}
        Ok(Retiring{key:self.key,source:self.source.verify(&checked.bytes)?})
    }
}
impl Retiring{
    pub(crate) fn previous(&self)->&Previous{&self.key}
    pub(crate) fn matches(&self,live:Option<HeldDir>,private:Option<HeldDir>)->io::Result<()>{verify_address(&self.source.group,live,private)}
    fn at(&self,p:&LocalPool,g:&LifecycleGrant<'_>,old:&CustodyEnd<'_,'_>,new:&CustodyEnd<'_,'_>,checked:&Checked)->io::Result<Progress>{
        if !matches!(p.access,Access::Write)||!new.belongs(g,Side::Target)||!old.belongs(g,Side::Source)||
            self.key.m!=checked.goal.frame||!checked.installed(){return Err(invalid("not the original paired retirement writer"));}
        let(step,_)=self.key.parts(p,g,old)?;old.check_retiring(self)?;checked.require(p,g,new)?;Ok(step)
    }
    pub(in crate::ordinary::custody) fn can_advance(&self,p:&LocalPool,g:&LifecycleGrant<'_>,old:&CustodyEnd<'_,'_>,new:&CustodyEnd<'_,'_>,checked:&Checked,next:Progress)->io::Result<()>{
        let step=self.at(p,g,old,new,checked)?;let group=&self.source.group;let count=self.key.m.original.role_count();
        if next==Progress::ARMED{
            if step!=Progress::OPEN||group.initial!=Progress::OPEN||!self.source.whole_at_start(){return Err(invalid("cannot arm without lease on the complete original source"));}
            return Ok(())
        }
        if !step.allows_next(count,next)||group.addr!=Address::Isolated||!group.exclusive{return Err(invalid("invalid step for this exact isolated FILE"));}
        let first=if next==Progress::EMPTY{count}else{next.authorized().ok_or_else(||invalid("not a source role"))?};
        if group.remaining.iter().map(|r|r.index).ne(first..count){return Err(invalid("original retirement role order differs"));}
        let dir=group.dir.as_ref().ok_or_else(||invalid("actual original quarantine needed for advancement"))?;
        if next==Progress::EMPTY{dir.only(None)?;}dir.file.sync_all()?;old.sync_absent_business(self)?;
        self.at(p,g,old,new,checked)?;Ok(())
    }
    pub(crate) fn can_observe_gone(&self,p:&LocalPool,g:&LifecycleGrant<'_>,old:&CustodyEnd<'_,'_>)->io::Result<()>{
        let(step,_)=self.key.parts(p,g,old)?;let gr=&self.source.group;
        if step!=Progress::EMPTY||gr.addr!=Address::EmptyGone||gr.initial!=Progress::EMPTY||gr.dir.is_some()||!gr.remaining.is_empty(){
            return Err(invalid("source absence is not the proven last empty step"));}
        Ok(())
    }
    pub(crate) fn isolation(&self,p:&LocalPool,g:&LifecycleGrant<'_>,old:&CustodyEnd<'_,'_>,new:&CustodyEnd<'_,'_>,checked:&Checked)->io::Result<(crate::Entity,fs::Birth)>{
        if self.at(p,g,old,new,checked)?!=Progress::ARMED||!self.source.whole_at_start(){return Err(invalid("not armed at the complete original source"));}
        let n=&self.source.group.original;Ok((n.entity,n.birth.clone()))
    }
    fn erase_one(&mut self,p:&LocalPool,g:&LifecycleGrant<'_>,old:&CustodyEnd<'_,'_>,new:&CustodyEnd<'_,'_>,checked:&Checked,
        i:usize,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<()>{
        let step=self.at(p,g,old,new,checked)?;
        let group=&mut self.source.group;
        if group.addr!=Address::Isolated||step!=Progress::leaf(i)?{return Err(invalid("cannot release an unissued original source role"));}
        let dir=group.dir.as_ref().ok_or_else(||invalid("original quarantine missing"))?;
        if let Some(r)=group.remaining.first().filter(|r|r.index==i){
            dir.release_verified_work(&r.name,&r.pin,||hook(Cut::MoveLeafRetired))?;group.remaining.remove(0);
        }
        dir.confirm_absence(group.order.get(i).ok_or_else(||invalid("no such original role"))?)?;Ok(())
    }
    pub(crate) fn empty_witness(&self,p:&LocalPool,g:&LifecycleGrant<'_>,old:&CustodyEnd<'_,'_>,new:&CustodyEnd<'_,'_>,checked:&Checked,held:&HeldDir)->io::Result<fs::Birth>{
        if self.at(p,g,old,new,checked)?!=Progress::EMPTY{return Err(invalid("no confirmed physical empty-source boundary"));}
        let gr=&self.source.group;let actual=gr.dir.as_ref().ok_or_else(||invalid("not a live original empty quarantine"))?;
        if gr.addr!=Address::Isolated||!gr.remaining.is_empty()||!std::ptr::eq(held,actual){return Err(invalid("not the empty original directory held by this retirement"));}
        actual.only(None)?;Ok(gr.original.birth.clone())
    }
    fn finish_empty(&mut self,p:&LocalPool,g:&LifecycleGrant<'_>,old:&CustodyEnd<'_,'_>,new:&CustodyEnd<'_,'_>,checked:&Checked,
        hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<()>{
        if self.at(p,g,old,new,checked)?!=Progress::EMPTY{return Err(invalid("only the last proven empty step accepts rmdir retry"));}
        hook(Cut::MoveEmptyConfirmed)?;
        if let Some(dir)=self.source.group.dir.as_ref(){
            old.release_empty(self,dir,new,checked,||hook(Cut::MoveDirectoryUnlinked))?;
            let gr=&mut self.source.group;gr.dir=None;gr.addr=Address::EmptyGone;gr.initial=Progress::EMPTY;
        }
        old.confirm_gone(self)?;self.at(p,g,old,new,checked)?;Ok(())
    }
}
fn with_ends<T>(p:&LocalPool,r:Request,expected:&Moved,checked:&Checked,
    act:impl FnOnce(&LifecycleGrant<'_>,&CustodyEnd<'_,'_>,&CustodyEnd<'_,'_>,&super::super::allocation::MoveAllocation)->io::Result<T>)->io::Result<T>{
    let g=LifecycleGrant::capture(p,r)?;let s=g.live_cut(p)?;let(m,a)=s.confirmed_handoff(r.op)?;
    if m!=expected||m!=&checked.goal.frame||!checked.installed()||!a.opened(){return Err(invalid("different existing destination or original cleanup"));}
    let new=p.custody_end(&g,Side::Target)?;checked.require(p,&g,&new)?;let old=p.custody_end(&g,Side::Source)?;
    act(&g,&old,&new,a)
}
pub(in crate::ordinary::custody) fn complete(system:&Path,r:Request,m:&Moved,target:&Checked,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<()>{
    let key=Previous{m:m.clone()};
    for _ in 0..2{
        let captured={let(p,_,_)=LocalPool::with_snapshot(system,Access::Incoming)?;
            with_ends(&p,r,m,target,|_,old,_,_|old.capture_retirement(&key))?};
        let mut v=captured.verify(target)?; // no SYSTEM or volume locks during content scan
        let(p,_,_)=LocalPool::with_snapshot(system,Access::Write)?;
        let mut cur=with_ends(&p,r,m,target,|g,old,new,_|v.at(&p,g,old,new,target))?;
        if cur!=v.source.group.initial{return Err(invalid("retirement progressed during independent source verification"));}
        if v.source.group.addr==Address::Business{
            if cur==Progress::OPEN{
                with_ends(&p,r,m,target,|g,old,new,a|a.step_lifecycle(&p,g,old,new,&v,target,Progress::ARMED,||hook(Cut::MoveRetireFlagBeforeSync)))?;
            }
            hook(Cut::MoveSourceAdmitted)?;
            with_ends(&p,r,m,target,|_,old,new,_|old.isolate_original(&v,new,target,||hook(Cut::MoveSourceIsolated)))?;
            continue
        }
        for i in 0..m.original.role_count(){
            let next=Progress::leaf(i)?;if cur>next{continue;}
            if cur<next{with_ends(&p,r,m,target,|g,old,new,a|a.step_lifecycle(&p,g,old,new,&v,target,next,||hook(Cut::MoveEraseFlagBeforeSync)))?;cur=next;}
            with_ends(&p,r,m,target,|g,old,new,_|v.erase_one(&p,g,old,new,target,i,hook))?;
        }
        if cur<Progress::EMPTY{with_ends(&p,r,m,target,|g,old,new,a|a.step_lifecycle(&p,g,old,new,&v,target,Progress::EMPTY,||hook(Cut::MoveEmptyFlagBeforeSync)))?;}
        with_ends(&p,r,m,target,|g,old,new,_|v.finish_empty(&p,g,old,new,target,hook))?;
        if let Some(i)=p.snapshot()?.1.pending{
            if i.frame.moved()?!=m{return Err(invalid("different original seal after verified retirement"));}
            p.system.remove_original_record(ordinary::journal::INTENT,&i.stamp,&i.bytes)?;hook(Cut::IntentReleased)?;
        }
        with_ends(&p,r,m,target,|g,old,new,a|{
            if v.at(&p,g,old,new,target)?!=Progress::EMPTY{return Err(invalid("unconfirmed original cleanup"));}
            old.confirm_gone(&v)?;p.system.remove_original_record(ordinary::allocation::NAME,&a.stamp,&a.bytes)
        })?;return hook(Cut::Release)
    }Err(invalid("exact original lifecycle source is still pending retirement"))
}
