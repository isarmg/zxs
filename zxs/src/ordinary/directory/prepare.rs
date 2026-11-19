//! The first named write is the exact existing SYSTEM operand. At most one
//! newly-created private child is ever bound to it. No detached Origin or
//! existing filesystem occupant can issue or rebind a ticket.
use super::{inventory::{self,same_child},operand::Operand,path::HeldParent,types::Origin};
use crate::{EntryId,OperationId,identity::DirectoryId,fs::{self,invalid},
    ordinary::{Cut,allocation::{self,Stage,SLOTS},material,wire::NativeObject},pool::lease::Access};
use std::io;

pub(super) fn issue(parent:&HeldParent<'_,'_>,op:OperationId,name:&str)->io::Result<()>{
    let p=parent.volume().pool;if !matches!(p.access,Access::Write){return Err(invalid("mkdir requires the original exclusive SYSTEM"));}
    p.require_business_op(op)?;let(_,s)=p.snapshot()?;parent.require()?;
    let o=Origin{op,uid:fs::local_uid(),parent:parent.basis().clone(),name:name.into(),entry:EntryId::new()?,id:DirectoryId::new()?,ticket:OperationId::new()?};
    s.new_directory_slot(&o,&p.journal_registration())?;
    let actual=inventory::census(parent,&s)?;let all:Vec<_>=actual.into_iter().collect();
    material::check_leaf(parent.directory(),&all,&inventory::human(name)?,false)?;material::check_ticket_paths(parent.directory(),name,o.ticket)?;
    for i in 0..SLOTS{material::check_leaf(parent.directory(),&all,&material::staging(o.ticket,i),false)?;}
    material::room_for_ticket(&all,name,&material::staging(o.ticket,0))?;
    let(n,previous)=s.frontier();let(inode,entity)=p.system.anonymous_record()?;
    let original=NativeObject{entity,birth:fs::Birth::read(&inode)?};let(bytes,digest)=Operand::image(p.id(),&original,&o,n+1,previous)?;
    fs::initial_attempt_binding(&inode,&allocation::binding(digest,None)?)?;
    parent.require()?;p.system.add_record(inode,allocation::NAME,&bytes)
}
pub(super) fn stage(parent:&HeldParent<'_,'_>,op:OperationId,request:[u8;32],mut hook:impl FnMut(Cut)->io::Result<()>)->io::Result<Operand>{
    let p=parent.volume().pool;if !matches!(p.access,Access::Write){return Err(invalid("not the original mkdir writer"));}
    let(_,s)=p.snapshot()?;let a=s.making.as_ref().ok_or_else(||invalid("no original mkdir reservation"))?;
    if a.origin.op!=op||a.request!=request||&a.origin.parent!=parent.basis(){return Err(invalid("different original mkdir request or parent"));}
    p.system.confirm_record_sync(allocation::NAME,&a.stamp,a.bytes.len())?;p.system.file.sync_all()?;
    let dir=parent.directory();let raw=inventory::census(parent,&s)?;
    let expected_stage=if let Some(st)=&a.stage{st.clone()}else{
        if s.pending.is_some()||s.decided(op).is_some(){return Err(invalid("sealed or published mkdir lost its bound child"));}
        let i=(0..SLOTS).find(|&i|!raw.contains(material::staging(a.origin.ticket,i).to_bytes())).ok_or_else(||invalid("mkdir ticket exhausted; existing occupants remain untouched"))?;
        let private=material::staging(a.origin.ticket,i);let h=dir.new_child_directory(&private)?;
        let object=NativeObject{entity:h.entity,birth:fs::Birth::read(&h.file)?};
        same_child(&h,&object)?;a.origin.check_child(i,&object)?;h.only(None)?;h.file.sync_all()?;dir.file.sync_all()?;
        hook(Cut::UnboundMkdir)?;
        let(_,now)=p.snapshot()?;let original=now.making.as_ref().ok_or_else(||invalid("mkdir reservation disappeared before binding"))?;
        if original.stamp!=a.stamp||original.bytes!=a.bytes||original.stage.is_some(){return Err(invalid("mkdir reservation changed before binding"));}
        inventory::census(parent,&now)?;let named=dir.child_directory(&private)?.ok_or_else(||invalid("newly created mkdir slot vanished"))?;
        same_child(&h,&object)?;same_child(&named,&object)?;h.only(None)?;
        let new_stage=Stage{slot:i,object};
        p.system.bind_attempt_at(allocation::NAME,&a.stamp,a.bytes.len(),&allocation::binding(a.digest,None)?,
            &allocation::binding(a.digest,Some(&new_stage))?,||hook(Cut::DirectoryBoundBeforeSync))?;
        new_stage
    };
    let(_,mut verified)=p.snapshot()?;inventory::census(parent,&verified)?;
    let held=verified.making.take().ok_or_else(||invalid("missing persistent mkdir reservation"))?;
    if held.bytes!=a.bytes||held.stage.as_ref()!=Some(&expected_stage)||!same_original_record(&a.stamp,&held.stamp){
        return Err(invalid("original mkdir reservation or stage changed"));}
    p.system.confirm_record_sync(allocation::NAME,&held.stamp,held.bytes.len())?;Ok(held)
}
fn same_original_record(a:&fs::OriginalStamp,b:&fs::OriginalStamp)->bool{
    // Setting the stage changes ctime: retain the exact native incarnation
    // and verify original immutable bytes separately.
    a.birth==b.birth&&a.stat.entity()==b.stat.entity()
}

#[cfg(test)]
#[path="../../../tests/unit/directory_prepare.rs"]
mod tests;
