//! Retire the one exact source after full installed destination admission.
//! First isolate it by non-replacing rename within its original root. The
//! original allocation authorizes at most one role's unlink at a time.
//! Loss of a business name, or even both names, is not proof of retirement.
pub(super) mod dedicated;
use super::{allocation::Progress,command::Request,destination::Method,finish::history,placed::{self,Reopened},
    source::{self,Leaf},transfer::{Children,Moved,Sites}};
use crate::{fs::{self,HeldDir,invalid,stream::PinnedRegular},ordinary::{self,Cut,formal::types::Commit,wire::{Frame,NativeObject}},
    pool::lease::{Access,FileVolume,LocalPool}};
use std::{collections::BTreeSet,ffi::CString,io,path::Path};

#[derive(Clone,Copy,Eq,PartialEq)]enum Address{Business,Isolated,EmptyGone}
struct Remnant{index:usize,kind:Leaf,name:CString,pin:PinnedRegular}
struct SourceRetirement{frame:Moved,initial:Progress,addr:Address,exclusive:bool,dir:Option<HeldDir>,original:NativeObject,
    business:CString,quarantine:CString,order:Vec<CString>,remaining:Vec<Remnant>}
pub(super) struct VerifiedSource{group:SourceRetirement}
impl VerifiedSource{
    pub(super) fn whole_at_start(&self)->bool{self.group.addr==Address::Business&&self.group.remaining.len()==self.group.frame.original.role_count()}
    pub(super) fn require(&self,v:&FileVolume<'_>,m:&Moved)->io::Result<()>{self.group.require(v,m)}
    pub(super) fn require_next(&self,v:&FileVolume<'_>,m:&Moved,next:Progress)->io::Result<()>{
        let g=&self.group;
        let count=m.original.role_count();let first=if next==Progress::EMPTY{count}else{next.authorized().ok_or_else(||invalid("not an isolated source release"))?};
        if g.addr!=Address::Isolated||!g.exclusive||g.remaining.iter().map(|r|r.index).ne(first..count){
            return Err(invalid("wrong remaining original source roles for next retirement step"));}
        g.require(v,m)?;let dir=g.dir.as_ref().ok_or_else(||invalid("no exact quarantined source"))?;
        // The business -> private rename and any newly absent previous role
        // must be durable before progressing; Empty requires the real live,
        // physically empty original directory, not merely lack of a path.
        if next==Progress::EMPTY{dir.only(None)?;}
        dir.file.sync_all()?;v.root.confirm_absence(&g.business)?;v.root.file.sync_all()?;g.require(v,m)
    }
}
fn business(m:&Moved)->io::Result<CString>{m.original.purpose.leaves(m.original.file,m.original.op,&m.original.name).map(|(from,_)|from)}
impl SourceRetirement{
    fn capture(v:&FileVolume<'_>,m:&Moved,step:Progress,c:&Frame,f:&Commit,d:Children<'_>,exclusive:bool)->io::Result<Self>{
        placed::root(v,&m.original.src_root)?;
        if v.id()!=m.original.from||m.original.mode!=Method::Transfer{return Err(invalid("not this original transfer source"));}
        let business=business(m)?;let quarantine=m.original.quarantine_name();
        let(_,s)=v.pool.snapshot()?;let prior=s.before_handoff(m)?;
        let sites=m.require_at(prior,f,d)?;let previous=prior.transfer();
        let g=Self::capture_selected(m,step,c,f,d,exclusive,sites,previous,v.root.child_directory(&business)?,v.root.child_directory(&quarantine)?)?;
        g.require(v,m)?;Ok(g)
    }
    fn capture_selected(m:&Moved,step:Progress,c:&Frame,f:&Commit,d:Children<'_>,exclusive:bool,sites:Sites,previous:Option<&Moved>,
        live:Option<HeldDir>,priv_name:Option<HeldDir>)->io::Result<Self>{
        let business=business(m)?;let quarantine=m.original.quarantine_name();
        let (addr,dir)=match(live,priv_name){
            (Some(dir),None) if step.business_possible()=>(Address::Business,Some(dir)),
            (None,Some(dir)) if step.retiring()=>(Address::Isolated,Some(dir)),
            (None,None) if step==Progress::EMPTY=>(Address::EmptyGone,None),
            _=>return Err(invalid("source not at its exact authorized business/quarantine name; do not infer retirement from absence")),
        };
        let rows=placed::roles(c,f,d,&sites)?;
        let order=rows.iter().rev().map(|r|r.name.clone()).collect();
        let mut g=Self{frame:m.clone(),initial:step,addr,exclusive,dir,original:sites.directory.clone(),business,quarantine,order,remaining:Vec::new()};
        if let Some(ref dir)=g.dir{
            if dir.entity!=sites.directory.entity||fs::Birth::read(&dir.file)?!=sites.directory.birth{return Err(invalid("unrecognized source occupant; leave untouched"));}
            if exclusive{dir.exclusive()?;dir.require_named()?;sites.directory.birth.require(&fs::Birth::read(&dir.file)?)?;}
            for (i,role) in rows.into_iter().rev().enumerate(){
                let present=dir.pin_stream(&role.name,role.len)?.is_some();
                let allowed=if step==Progress::EMPTY{!present}else{match step.authorized(){
                    Some(k) if i<k=>!present,Some(k) if i==k=>true,_=>present,
                }};
                if !allowed{return Err(invalid("source role disappeared or reappeared outside its exact authorized unlink"));}
                if present{
                    // A B-native later INCR does not inherit A->B's token;
                    // an actual old transferred role must retain that token.
                    let allocation=previous.filter(|x|x.original.mode==Method::Transfer&&x.target.at(role.kind)==Some(role.original)).map(|x|x.allocation);
                    let pin=placed::held_role(dir,&role,allocation)?;if exclusive{pin.file.try_lock().map_err(io::Error::from)?;}
                    g.remaining.push(Remnant{index:i,kind:role.kind,name:role.name,pin});
                }
            }
        }
        Ok(g)
    }
    fn verify(self,dest:&Reopened)->io::Result<VerifiedSource>{
        if !self.exclusive{return Err(invalid("source inventory is not a retirement lease"));}
        for r in &self.remaining{
            let expected=dest.files().iter().find(|v|v.kind==r.kind).ok_or_else(||invalid("original target role missing"))?;
            if r.name!=expected.name||source::read_physical(&r.pin,expected.len,|_,_|Ok(()))?!=expected.digest{
                return Err(invalid("original source bytes differ from checked target; nothing new may be retired"));}
        }Ok(VerifiedSource{group:self})
    }
    fn require(&self,v:&FileVolume<'_>,m:&Moved)->io::Result<()>{
        if &self.frame!=m||v.id()!=m.original.from{return Err(invalid("different original custody retirement"));}
        placed::root(v,&m.original.src_root)?;
        let (n,other)=if self.addr==Address::Business{(&self.business,&self.quarantine)}else{(&self.quarantine,&self.business)};
        if v.root.entry_present(other)?{return Err(invalid("other source name occupied; do not alter either name"));}
        self.require_named_dir(v.root.child_directory(n)?)?;v.require_current()
    }
    fn require_named_dir(&self,named:Option<HeldDir>)->io::Result<()>{
        match(&self.dir,named){
            (Some(dir),Some(actual))=>{
                if dir.entity!=self.original.entity||actual.entity!=self.original.entity||!dir.same_mount(&actual)?||
                    fs::Birth::read(&dir.file)?!=self.original.birth||fs::Birth::read(&actual.file)?!=self.original.birth{
                    return Err(invalid("original source directory changed"));}
                let want:BTreeSet<_>=self.remaining.iter().map(|r|r.name.to_bytes().to_vec()).collect();
                let names=dir.names()?;if names.len()!=want.len()||names.iter().any(|n|!want.contains(n)){return Err(invalid("unknown or changed source contents"));}
                for r in &self.remaining{r.pin.named(dir,&r.name)?;}dir.require_named()?;
            },
            (None,None) if self.addr==Address::EmptyGone&&self.initial==Progress::EMPTY=>{},
            _=>return Err(invalid("original source address changed")),
        }Ok(())
    }
}
fn exact_progress(p:&LocalPool,m:&Moved)->io::Result<Progress>{
    let(_,s)=p.snapshot()?;let(now,a)=s.confirmed_handoff(m.original.op)?;
    if now!=m||!a.opened(){return Err(invalid("different original target at retirement"));}
    a.confirm_phase(p)?;Ok(a.progress())
}
fn advance(p:&LocalPool,old:&FileVolume<'_>,new:&FileVolume<'_>,m:&Moved,g:&VerifiedSource,dst:&Reopened,next:Progress,
    hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<()>{
    let(_,s)=p.snapshot()?;let(now,a)=s.confirmed_handoff(m.original.op)?;
    if now!=m{return Err(invalid("different original MOVE at retirement advance"));}
    a.advance_isolated(p,old,new,g,dst,next,||hook(if next==Progress::EMPTY{Cut::MoveEmptyFlagBeforeSync}else{Cut::MoveEraseFlagBeforeSync}))?;
    if exact_progress(p,m)?!=next{return Err(invalid("original retirement advance not confirmed"));}Ok(())
}
fn empty_source(p:&LocalPool,old:&FileVolume<'_>,new:&FileVolume<'_>,m:&Moved,g:&mut VerifiedSource,dst:&Reopened,
    hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<()>{
    let mut cur=exact_progress(p,m)?;if cur!=g.group.initial||!cur.retiring(){return Err(invalid("original isolated source cut changed"));}
    for i in 0..m.original.role_count(){
        let wanted=Progress::leaf(i)?;if cur.past_leaf(i){continue;}
        if cur!=wanted{advance(p,old,new,m,g,dst,wanted,hook)?;cur=wanted;}
        g.require(old,m)?;dst.verify_installed(new,m)?;
        let gr=&mut g.group;let dir=gr.dir.as_ref().ok_or_else(||invalid("quarantine missing before original empty confirmation"))?;
        if let Some(r)=gr.remaining.first().filter(|r|r.index==i){
            dir.release_verified_work(&r.name,&r.pin,||hook(Cut::MoveLeafRetired))?;gr.remaining.remove(0);
        }
        // Only this single authorized role may already be absent on retry.
        dir.confirm_absence(&gr.order[i])?;
    }
    if cur!=Progress::EMPTY{advance(p,old,new,m,g,dst,Progress::EMPTY,hook)?;}
    if exact_progress(p,m)?!=Progress::EMPTY{return Err(invalid("original empty source not confirmed"));}
    hook(Cut::MoveEmptyConfirmed)?;g.require(old,m)?;dst.verify_installed(new,m)?;
    let gr=&mut g.group;
    if let Some(dir)=gr.dir.take(){
        old.root.retire_empty_original(&gr.quarantine,&dir,&gr.original.birth,||hook(Cut::MoveDirectoryUnlinked))?;
    }
    old.root.confirm_absence(&gr.quarantine)?;old.root.confirm_absence(&gr.business)?;old.require_current()?;dst.verify_installed(new,m)
}
pub(super) fn complete(system:&Path,r:Request,m:&Moved,dst:&Reopened,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<()>{
    // The only optional second pass is after the business -> quarantine
    // rename: reopen all roles frozen in this original MOVE by the private, now exact name.
    for _ in 0..2{
        let mut cut=if m.original.mode==Method::Transfer{
            let g={let(p,_,s)=LocalPool::with_snapshot(system,Access::Incoming)?;let(now,a)=s.confirmed_handoff(r.op)?;
                if now!=m||a.request!=r.digest(&p)?||!a.opened(){return Err(invalid("no genuine applied target for original source retirement"));}
                a.confirm_phase(&p)?;let new=p.ordinary(m.original.to)?;dst.verify_installed(&new,now)?;let old=p.ordinary(m.original.from)?;
                let(c,f,d)=history(&s,now)?;SourceRetirement::capture(&old,now,a.progress(),&c,&f,d.as_refs(),true)?};
            Some(g.verify(dst)?)
        }else{None};
        let(p,_,s)=LocalPool::with_snapshot(system,Access::Write)?;let(now,a)=s.confirmed_handoff(r.op)?;
        if now!=m||a.request!=r.digest(&p)?||!a.opened(){return Err(invalid("missing original reopened target at exact cleanup cut"));}
        let old=p.ordinary(m.original.from)?;let new=p.ordinary(m.original.to)?;
        placed::root(&old,&m.original.src_root)?;dst.verify_installed(&new,now)?;
        if let Some(ref mut g)=cut{
            g.require(&old,now)?;if g.group.initial!=a.progress(){return Err(invalid("source cleanup advanced during independent verification"));}
            if g.group.addr==Address::Business{
                if !a.retiring(){a.begin_verified_retirement(&p,&old,&new,g,dst,||hook(Cut::MoveRetireFlagBeforeSync))?;}
                if exact_progress(&p,now)?!=Progress::ARMED{return Err(invalid("wrong original isolation phase"));}
                hook(Cut::MoveSourceAdmitted)?;
                g.require(&old,now)?;dst.verify_installed(&new,now)?;
                let gr=&g.group;old.root.handoff_directory(&gr.business,&old.root,&gr.quarantine,
                    gr.original.entity,&gr.original.birth,||hook(Cut::MoveSourceIsolated))?;continue;
            }
            empty_source(&p,&old,&new,now,g,dst,hook)?;
        }else{
            if !old.root.same_mount(&new.root)?{return Err(invalid("original rename mount changed; no transfer fallback"));}
            old.root.confirm_absence(&business(now)?)?;new.root.file.sync_all()?;
        }
        old.require_current()?;dst.verify_installed(&new,now)?;
        let(_,latest)=p.snapshot()?;let(cur,active)=latest.confirmed_handoff(r.op)?;
        if cur!=m||!active.opened()||(m.original.mode==Method::Transfer&&active.progress()!=Progress::EMPTY){
            return Err(invalid("different source retirement at completion"));}
        if let Some(i)=&latest.pending{
            if i.frame.moved()?!=cur{return Err(invalid("different original sealed MOVE"));}
            p.system.remove_original_record(ordinary::journal::INTENT,&i.stamp,&i.bytes)?;hook(Cut::IntentReleased)?;
        }
        p.system.remove_original_record(ordinary::allocation::NAME,&active.stamp,&active.bytes)?;return hook(Cut::Release)
    }
    Err(invalid("exact original source remains pending"))
}
/// An unresolved original source address is not a business FILE. The
/// returned spelling identifies only the physically verified occurrence on
/// this exact source root. It grants neither read nor retirement authority.
pub(super) fn observe_source(v:&FileVolume<'_>,m:&Moved,step:Progress,c:&Frame,f:&Commit,d:Children<'_>)->io::Result<Option<Vec<u8>>>{
    if m.original.mode==Method::Transfer{
        let g=SourceRetirement::capture(v,m,step,c,f,d,false)?;
        return Ok(match g.addr{Address::EmptyGone=>None,Address::Business=>Some(g.business.into_bytes()),Address::Isolated=>Some(g.quarantine.into_bytes())})
    }
    let n=business(m)?;placed::root(v,&m.original.src_root)?;
    let Some(dir)=v.root.child_directory(&n)? else{return Ok(None)};
    let sites=Sites::birthplace(c,f,d);if dir.entity!=sites.directory.entity||fs::Birth::read(&dir.file)?!=sites.directory.birth{
        return Err(invalid("unknown same-named occupant on original rename source"));}
    let mut names:BTreeSet<Vec<u8>>=dir.names()?.into_iter().collect();
    for role in placed::roles(c,f,d,&sites)?{
        if !names.remove(role.name.to_bytes()){return Err(invalid("unverified missing original rename source role"));}
        let _=placed::held_role(&dir,&role,None)?;
    }
    if !names.is_empty(){return Err(invalid("unknown contents at original rename source"));}dir.require_named()?;Ok(Some(n.into_bytes()))
}
