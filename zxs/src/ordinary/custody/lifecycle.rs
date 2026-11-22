//! The same native reservation, sealed instruction and SYSTEM lower bound
//! as an ordinary FILE operation. Preparation and publication alone do not
//! remove anything. The paired public commands complete through this one path.
use super::{admission::{LifecycleGrant,Side},allocation::MoveAllocation,command::Request,incoming::IncomingSelection,
    purpose::Call,source::Clean,target::{Checked,Goal,Site},transfer::Moved};
use crate::{fs::invalid,ordinary::{self,Cut,journal::Intent},pool::lease::{Access,LocalPool,recycle::incoming::PreparedIncoming}};
use std::{io,path::Path};

/// Fixed while resolving a public selector under its actual register. Changing
/// SYSTEM between that resolution and source selection must never authorize a
/// native operation on another register, even before its first reservation.
pub(super) struct Invocation{r:Request,digest:[u8;32],register:(crate::Entity,crate::fs::Birth)}
impl Invocation{
    pub(super) fn new(p:&LocalPool,r:Request)->io::Result<Self>{Ok(Self{r,digest:r.digest(p)?,register:p.lifecycle_register()})}
    pub(super) fn require(&self,p:&LocalPool)->io::Result<()>{
        if self.register!=p.lifecycle_register()||self.digest!=self.r.digest(p)?{return Err(invalid("different original SYSTEM registration or local custody request"));}
        Ok(())
    }
}
fn decide_named(system:&Path,r:Request,ready:&PreparedIncoming<'_>,target:&Checked,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<Moved>{
    let m=&target.goal.frame;
    let(p,_,s)=LocalPool::with_snapshot(system,Access::Write)?;
    let req=r.digest(&p)?;
    if target.installed()||m.request!=req||m.original.op!=r.op||m.original.purpose.call()!=r.call{
        return Err(invalid("a private lifecycle FILE cannot sign some other action"));}
    let a=s.moving.as_ref().ok_or_else(||invalid("original lifecycle reservation disappeared"))?;
    ready.recheck_for_seal(&p,target)?;s.admit_dedicated_frame(&p.journal_registration(),m)?;a.confirm_phase(&p)?;
    if s.pending.is_none(){hook(Cut::BeforeIntent)?;ready.recheck_for_seal(&p,target)?;Intent::seal(&p.system,&m.bytes)?;}
    hook(Cut::Intent)?;
    // The new INTENT changed the exact live stage. The final source,
    // independently read target and both endpoints are checked against a
    // newly acquired grant on the same exclusive original SYSTEM.
    ordinary::publish_sealed(&p,r.op,req,|actual|{
        if actual.moved()?!=m{return Err(invalid("not the original sealed dedicated handoff"));}
        ready.recheck_for_seal(&p,target)
    },|cut|{
        hook(cut)?;
        // A complete frame is still unpublished here. A changed source,
        // lost stage or competing final name must stop the warm path just
        // as it stops recovery of that same complete but unconfirmed frame.
        if cut==Cut::FrameSynced{ready.recheck_for_seal(&p,target)?;}Ok(())
    })?;
    hook(Cut::Decision)?;Ok(m.clone())
}

fn decide_for(system:&Path,call:&Invocation,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<Moved>{
    let r=call.r;
    if r.call==Call::Move{return Err(invalid("ordinary MOVE cannot use dedicated custody"));}
    let reserved={
        let(p,_,s)=LocalPool::with_snapshot(system,Access::Read)?;call.require(&p)?;
        if let Some(d)=s.decided(r.op){
            let m=d.moved()?;if m.request!=r.digest(&p)?||m.original.purpose.call()!=r.call{return Err(invalid("different original lifecycle request"));}
            return Ok(m.clone())
        }
        let _=LifecycleGrant::capture(&p,r)?;s.moving.is_some()
    };
    let source=Clean::capture_checked(system,r,|p|call.require(p))?.verify()?;
    let ready=IncomingSelection::prepare_checked(system,&source,r,|p|call.require(p))?;
    if !reserved{MoveAllocation::reserve_lifecycle(system,&ready)?;}
    hook(Cut::Reservation)?;
    ready.ensure_private_stage(system,||hook(Cut::UnboundMkdir))?;
    let input=ready.name_and_reopen(system,||hook(Cut::MoveLeafLinked))?;
    let exact=input.verify()?;decide_named(system,r,&ready,&exact,hook)
}

pub(super) fn installed(system:&Path,r:Request,expected:&Moved,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<Checked>{
    // No opening the old source, even if its original location is offline.
    // At most one real rename followed by a new, independent verified read.
    for _ in 0..2{
        let scan={let(p,_,s)=LocalPool::with_snapshot(system,Access::Incoming)?;
            let g=LifecycleGrant::capture(&p,r)?;let(m,a)=s.confirmed_handoff(r.op)?;
            if m!=expected{return Err(invalid("another confirmed lifecycle operation"));}
            let end=p.custody_end(&g,Side::Target)?;
            let site=if end.target_present()?{Site::Installed}else if a.opened(){
                return Err(invalid("previously confirmed target missing: do not use its source or reconstruct"));
            }else{Site::Private};
            end.read_target(&Goal{frame:m.clone(),site})?
        };
        let checked=scan.verify()?;
        {let(p,_,s)=LocalPool::with_snapshot(system,Access::Write)?;
            let g=LifecycleGrant::capture(&p,r)?;let(m,a)=s.confirmed_handoff(r.op)?;
            if m!=expected||m!=&checked.goal.frame{return Err(invalid("different installed lifecycle original"));}
            let end=p.custody_end(&g,Side::Target)?;checked.require(&p,&g,&end)?;
            if !checked.installed(){end.install_checked(&checked,||hook(Cut::MoveInstalled))?;continue;}
            if !a.opened(){a.confirm_lifecycle_target(&p,&g,&end,&checked,||hook(Cut::MoveTargetFlagBeforeSync))?;}
            else{end.sync_installed(&checked)?;a.confirm_phase(&p)?;}
        }
        hook(Cut::MoveTargetChecked)?;return Ok(checked)
    }Err(invalid("lifecycle target still pending independent installed readback"))
}

pub(super) fn perform_for(system:&Path,call:&Invocation,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<super::command::MoveReceipt>{
    use super::command::MoveStatus;
    let r=call.r;
    if r.call==Call::Move{return Err(invalid("ordinary MOVE is not a dedicated operation"));}
    {let(p,_,s)=LocalPool::with_snapshot(system,Access::Read)?;call.require(&p)?;
        if let Some(old)=r.observe(&p,&s)?{if old.status==MoveStatus::OriginalPublished{return Ok(old);}}}
    let m=decide_for(system,call,hook)?;
    if m.request!=call.digest{return Err(invalid("another SYSTEM or original custody decision"));}
    let target=installed(system,r,&m,hook)?;
    super::retire::dedicated::complete(system,r,&m,&target,hook)?;
    let(p,_,s)=LocalPool::with_snapshot(system,Access::Read)?;
    call.require(&p)?;let mut result=r.observe(&p,&s)?.ok_or_else(||invalid("original lifecycle decision missing after completion"))?;
    if result.status!=MoveStatus::OriginalPublished{return Err(invalid("original lifecycle still pending"));}
    result.status=MoveStatus::CompleteCheckedNow;Ok(result)
}

#[cfg(test)]
include!("../../../tests/unit/attached/ordinary/custody/lifecycle/items.rs");
