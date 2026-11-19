//! Keep logical FILE birth inside the container and physical custody in SYSTEM.
//! The exclusive volume lease remains held until source, target and all names
//! have been verified. Unattributed private staging is never adopted or swept.
use super::allocation::{Allocation,Stage,SLOTS};
use super::wire::{Creation,NativeObject,empty_material};
use super::storage_name::{DirectoryName,FileName,ResidentNames,TicketLeaf,file_leaf};
use crate::{FileId,OperationId,fs::{self,Birth,HeldDir,invalid},pool::lease::FileVolume};
use std::{ffi::{CStr,CString},io};

const LOCAL:&CStr=c"FILE";
const MAX_LOCAL:usize=512;
pub(super) fn staging(op:OperationId,slot:u8)->CString{
    TicketLeaf::stage(op,slot).expect("validated original operation and slot").name()
}
/// Recognizing a distinct reserved spelling is not authority to inspect or
/// remove an occupant of that name.
pub(super) fn private_slot(raw:&[u8])->Option<(OperationId,u8)>{TicketLeaf::parse(raw)?.as_stage()}
/// Independent of the display-name index: only an exact *physical* leaf
/// can be compared here. Its presence at a permitted installation still
/// requires subsequent positive verification of that actual native object.
pub(in crate::ordinary) fn check_leaf(parent:&HeldDir,raw:&[Vec<u8>],leaf:&CStr,allow:bool)->io::Result<()>{
    if parent.exact_presence(raw,leaf)?&&!allow{
        return Err(io::Error::new(io::ErrorKind::AlreadyExists,"exact physical destination occupied"));}
    Ok(())
}
fn check_name(volume:&FileVolume<'_>,display:&str,file:FileId,allow:bool)->io::Result<()>{
    let n=file_leaf(display,file)?;let raw=volume.root.names()?;let(_,cut)=volume.pool.snapshot()?;
    super::ordinary_target_domain(volume.pool,&cut,volume.id(),&volume.root,display,file,&raw)?;
    check_leaf(&volume.root,&raw,&n,allow)
}
pub(in crate::ordinary) fn vacant_for_move(v:&FileVolume<'_>,display:&str,file:FileId)->io::Result<()>{
    v.require_current()?;let n=file_leaf(display,file)?;fs::NameBudget::held(&v.root)?.child(&n)?;check_name(v,display,file,false)
}
pub(in crate::ordinary) fn room_for_ticket(names:&[Vec<u8>],name:&str,stage:&CStr)->io::Result<()>{
    let bytes:usize=names.iter().map(|n|1+n.len()).sum();let n=usize::from(SLOTS);
    if names.len()+n>8192 || bytes+n*(1+name.len().max(stage.to_bytes().len()))>512*1024 {
        return Err(invalid("insufficient bounded inventory for the original temporary names"));
    }
    Ok(())
}

/// Check every name this exact ticket might need, against the same held
/// parent. Inventory capacity and vacancy are separate checks; this does not
/// allocate a slot, authorize a parent, or open/claim any existing occupant.
fn directory_paths<T>(name:&str,ticket:OperationId,mut check:impl FnMut(&CStr)->io::Result<T>)->io::Result<T>{
    let final_name=check(&DirectoryName::parse(name.as_bytes())?.leaf())?;
    for slot in 0..SLOTS{check(&staging(ticket,slot))?;}Ok(final_name)
}
pub(in crate::ordinary) fn check_ticket_paths(parent:&HeldDir,name:&str,ticket:OperationId)->io::Result<()>{
    let budget=fs::NameBudget::held(parent)?;directory_paths(name,ticket,|n|budget.child(n)).map(|_|())
}
pub(in crate::ordinary) fn directory_budget(parent:fs::NameBudget,name:&str,ticket:OperationId)->io::Result<fs::PathBudget>{
    directory_paths(name,ticket,|n|parent.child(n))
}
pub(in crate::ordinary) fn resident(display:&str,file:FileId,ticket:OperationId)->io::Result<ResidentNames>{
    ResidentNames::for_file(FileName::parse(display.as_bytes())?,file,ticket)
}
/// Never opens the recycle volume: the SYSTEM registration, which also
/// authenticated this ordinary parent, supplies its original geometry.
pub(in crate::ordinary) fn birth_geometry(reg:&crate::pool::JournalRegistration<'_>,id:crate::VolumeId,uid:u32,parent:fs::PathBudget,
    display:&str,file:FileId,ticket:OperationId)->io::Result<fs::PathBudget>{
    resident(display,file,ticket)?.new_spelling(reg.ordinary_names(id,uid,parent)?,reg.recycle_names(uid)?)
}
/// Physical source of the applicant is the held ordinary parent. New names
/// are refused at both the recorded limit and the currently observed one.
pub(in crate::ordinary) fn held_residence(v:&FileVolume<'_>,parent:&HeldDir,display:&str,file:FileId,ticket:OperationId)->io::Result<()>{
    let uid=fs::local_uid();let reg=v.pool.journal_registration();
    birth_geometry(&reg,v.id(),uid,fs::path_budget(&parent.path)?,display,file,ticket)?;
    resident(display,file,ticket)?.at(fs::NameBudget::held(parent)?)?;Ok(())
}
/// No named write. The actual root, FileId and this ticket must be reserved
/// before naming any object. Unknown private spellings are not adopted.
pub(super) fn select_ticket(volume:&FileVolume<'_>,name:&str,file:FileId)->io::Result<(OperationId,NativeObject)>{
    volume.require_current()?;let attempt=OperationId::new()?;let leaf=file_leaf(name,file)?;
    held_residence(volume,&volume.root,name,file,attempt)?;
    let names=volume.root.names()?;let(_,cut)=volume.pool.snapshot()?;
    super::ordinary_target_domain(volume.pool,&cut,volume.id(),&volume.root,name,file,&names)?;
    check_leaf(&volume.root,&names,&leaf,false)?;
    for i in 0..SLOTS{check_leaf(&volume.root,&names,&staging(attempt,i),false)?;}
    check_leaf(&volume.root,&names,&TicketLeaf::isolation(attempt)?.name(),false)?;
    room_for_ticket(&names,leaf.to_str().map_err(|_|invalid("invalid derived container"))?,&staging(attempt,0))?;
    Ok((attempt,NativeObject{entity:volume.root.entity,birth:Birth::read(&volume.root.file)?}))
}

pub(super) fn bind_fresh_stage(volume:&FileVolume<'_>,a:&Allocation,after_mkdir:impl FnOnce()->io::Result<()>)->io::Result<()>{
    a.require_root_writer(volume)?;check_name(volume,&a.name,a.file,false)?;
    if a.stage.is_some(){return Err(invalid("original root FILE ticket is already bound"));}
    let mut next=None;
    for slot in 0..SLOTS{
        match volume.root.child_directory(&staging(a.attempt,slot))?{
            Some(unbound)=>{
                // An empty unbound directory is positively NOT a FILE. Do not
                // touch it, certify its origin, or adopt it even if it looks ours.
                // Unknown contents, an unreadable/foreign mount, or non-directory
                // occupant stop the request; no alternative material is created.
                if next.is_some(){return Err(invalid("nonconsecutive occupants of original temporary ticket"));}
                unbound.only(None)?;unbound.require_named()?;
            }
            None=>{if next.is_none(){next=Some(slot);}}
        }
    }
    let slot=next.ok_or_else(||invalid("original ticket exhausted; preserve unbound private entries"))?;
    let dir=volume.root.new_child_directory(&staging(a.attempt,slot))?;
    dir.only(None)?;dir.file.sync_all()?;volume.root.file.sync_all()?;
    after_mkdir()?;
    dir.require_named()?;volume.require_current()?;dir.only(None)?;
    let original=NativeObject{entity:dir.entity,birth:Birth::read(&dir.file)?};
    a.bind(volume,&Stage{slot,object:original})
}
fn local_token(allocation:[u8;32],local:&NativeObject)->io::Result<[u8;32]>{
    let mut b=b"ZXS/INITIAL-LOCAL-MATERIAL\0".to_vec();b.extend_from_slice(&allocation);local.put(&mut b);
    crate::hash::digest(&b)
}
pub(super) fn prepare(volume:&FileVolume<'_>,a:&super::allocation::Allocation)->io::Result<Creation>{
    a.require_root_writer(volume)?;check_name(volume,&a.name,a.file,false)?;
    let s=a.bound()?;
    let dir=volume.root.child_directory(&staging(a.attempt,s.slot))?.ok_or_else(||invalid("reserved physical FILE stage disappeared"))?;
    let directory=NativeObject{entity:dir.entity,birth:Birth::read(&dir.file)?};
    if directory!=s.object{return Err(invalid("reserved physical FILE stage was replaced"));}
    dir.only(Some(LOCAL))?;
    let bytes=empty_material(a.file,a.history,a.op,&a.name);
    // Create a logical FILE only after its exact stage and stable IDs are
    // durable in SYSTEM. All unmatched material is preserved unchanged.
    let local=if let Some((st,original))=dir.record(LOCAL,MAX_LOCAL)?{
        let obj=NativeObject::stamp(&st);
        if original!=bytes{return Err(invalid("unexpected original FILE birth metadata"));}
        dir.require_preparation_token(LOCAL,&st,&local_token(a.digest,&obj)?,MAX_LOCAL)?;
        obj
    }else{
        let (data,entity)=dir.anonymous_record()?;
        let obj=NativeObject{entity,birth:Birth::read(&data)?};
        fs::set_preparation_token(&data,&local_token(a.digest,&obj)?)?;
        dir.add_record(data,LOCAL,&bytes)?;
        obj
    };
    dir.file.sync_all()?;volume.root.file.sync_all()?;
    let c=Creation{parent:super::file_parent::BirthParent::Root,op:a.op,uid:a.uid,volume:volume.id(),entry:a.entry,file:a.file,history:a.history,
        name:a.name.clone(),attempt:a.attempt,slot:s.slot,allocation:a.digest,directory,local,local_digest:crate::hash::digest(&bytes)?};
    require_staged(volume,&c)?;Ok(c)
}
fn verify_header(dir:&HeldDir,c:&Creation)->io::Result<()>{
    verify_header_at(dir,c,&c.directory,&c.local)
}
pub(in crate::ordinary) fn verify_header_at(dir:&HeldDir,c:&Creation,container:&NativeObject,local:&NativeObject)->io::Result<()>{
    if dir.entity!=container.entity{return Err(invalid("FILE container entity changed"));}
    container.birth.require(&Birth::read(&dir.file)?)?;
    let (origin,bytes)=dir.record(LOCAL,MAX_LOCAL)?.ok_or_else(||invalid("original FILE metadata missing"))?;
    if &NativeObject::stamp(&origin)!=local || crate::hash::digest(&bytes)?!=c.local_digest ||
        bytes!=empty_material(c.file,c.history,c.op,&c.name) {
        return Err(invalid("FILE local birth, contents or manual-unborn state differs"));
    }
    dir.require_named()
}
fn verify_directory(dir:&HeldDir,c:&Creation)->io::Result<()>{
    dir.only(Some(LOCAL))?;verify_header(dir,c)
}
/// The only optional extra occupant is a working object attributable to this
/// exact FILE’s original SYSTEM command. Its shape is not permission to serve
/// it: a current reader still requires the published native inode and head.
pub(super) fn working_directory(volume:&FileVolume<'_>,s:&super::journal::Journal,f:&super::wire::Frame,working:Option<(OperationId,bool)>)->io::Result<HeldDir>{
    volume.require_current()?;let dir=working_directory_held(volume,s,f,working)?;volume.require_current()?;Ok(dir)
}
/// Caller compares the selected volume before and after a batch. Retain
/// the original accepted birth, rather than treating volume+name as a FILE.
pub(super) fn working_directory_held(volume:&FileVolume<'_>,s:&super::journal::Journal,f:&super::wire::Frame,working:Option<(OperationId,bool)>)->io::Result<HeldDir>{
    directory_all(volume,s,f,working,None,&[])
}
pub(super) fn directory_all(volume:&FileVolume<'_>,s:&super::journal::Journal,f:&super::wire::Frame,working:impl IntoIterator<Item=(OperationId,bool)>,
    formal:Option<(&super::formal::types::Original,bool)>,incr:&[(&super::delta::types::Original,bool)])->io::Result<HeldDir>{
    let dir=super::directory::file_home::open(volume,s,f)?;
    require_entries(&dir,working,formal,incr)?;dir.require_named()?;Ok(dir)
}
/// Exact FILE names from original decisions/issued operations; not a source of
/// native identities. A future extent is never part of an earlier MOVE.
pub(in crate::ordinary) fn require_entries(dir:&HeldDir,working:impl IntoIterator<Item=(OperationId,bool)>,
    formal:Option<(&super::formal::types::Original,bool)>,incr:&[(&super::delta::types::Original,bool)])->io::Result<()>{
    let listed=dir.names()?;let count=listed.len();
    let mut names:std::collections::BTreeSet<_>=listed.into_iter().collect();
    if names.len()!=count{return Err(invalid("duplicate names during FILE enumeration"));}
    let mut take=|name:&CStr|names.remove(name.to_bytes());
    if !take(LOCAL){return Err(invalid("original FILE record missing"));}
    for (op,required) in working{
        if !take(&super::draft::material::leaf(op))&&required{return Err(invalid("required FILE working body is unnamed"));}
    }
    if let Some((o,required))=formal{
        let node=take(&o.node_name());let raw=take(&o.body_name());
        if (raw&&!node)||(required&&(!node||!raw)){return Err(invalid("missing or unordered formal FILE material"));}
    }
    for &(o,required) in incr{if !take(&o.leaf())&&required{return Err(invalid("required original incremental node is missing"));}}
    if !names.is_empty(){return Err(invalid("unknown material in the selected FILE"));}
    Ok(())
}
/// One existing FILE birth registration is the read lease for an immutable
/// working cut; no new sidecar or owner. Future retirement must acquire an
/// exclusive lease on this same native inode before removing used material.
pub(in crate::ordinary) fn custody_lease_at(dir:&HeldDir,c:&Creation,local:&NativeObject,retire:bool)->io::Result<fs::stream::PinnedRegular>{
    let bytes=empty_material(c.file,c.history,c.op,&c.name);
    let pin=dir.pin_stream(LOCAL,bytes.len() as u64)?.ok_or_else(||invalid("original FILE birth unavailable for read lease"))?;
    if retire{pin.file.try_lock().map_err(io::Error::from)?;}else{pin.file.try_lock_shared().map_err(io::Error::from)?;}
    if &NativeObject::stamp(&pin.stamp)!=local{return Err(invalid("foreign FILE read lease"));}
    let mut actual=vec![0;bytes.len()];fs::stream::read_at(&pin.file,&mut actual,0)?;
    if actual!=bytes||crate::hash::digest(&actual)?!=c.local_digest{return Err(invalid("FILE lease does not cover the original local birth"));}
    pin.named(dir,LOCAL)?;Ok(pin)
}
pub(super) fn require_staged(volume:&FileVolume<'_>,c:&Creation)->io::Result<()>{
    if volume.id()!=c.volume{return Err(invalid("wrong owner volume for staged FILE"));}
    volume.require_current()?;check_name(volume,&c.name,c.file,false)?;
    let dir=volume.root.child_directory(&staging(c.attempt,c.slot))?.ok_or_else(||invalid("original prepared FILE is missing"))?;
    verify_directory(&dir,c)?;
    dir.file.sync_all()?;volume.root.file.sync_all()?;volume.require_current()
}
pub(super) fn installed(volume:&FileVolume<'_>,c:&Creation)->io::Result<()>{
    if volume.id()!=c.volume{return Err(invalid("wrong volume for current FILE"));}
    volume.require_current()?;
    verify_named(volume,c)?;volume.require_current()
}
pub(super) fn verify_named(volume:&FileVolume<'_>,c:&Creation)->io::Result<()>{
    let dir=volume.exact_directory(&file_leaf(&c.name,c.file)?)?.ok_or_else(||invalid("decided FILE not installed or unavailable"))?;
    verify_directory(&dir,c)?;
    if volume.root.child_directory(&staging(c.attempt,c.slot))?.is_some(){return Err(invalid("both FILE names remain present"));}
    Ok(())
}
pub(super) fn install_decided(volume:&FileVolume<'_>,c:&Creation)->io::Result<()>{
    if c.parent!=super::file_parent::BirthParent::Root||c.volume!=volume.id()||!matches!(volume.pool.access,crate::pool::lease::Access::Write){
        return Err(invalid("not an original FILE root installation on this writer"));}
    volume.require_current()?;
    let(_,s)=volume.pool.snapshot()?;let original=s.operation(c.op).ok_or_else(||invalid("no confirmed original root FILE creation"))?;
    if &original.create!=c||!s.allocated.as_ref().is_some_and(|a|a.matches(original)){
        return Err(invalid("no matching root creation and current installation reservation"));}
    check_name(volume,&c.name,c.file,true)?;let dst=file_leaf(&c.name,c.file)?;
    if volume.root.child_directory(&dst)?.is_none(){
        require_staged(volume,c)?;
        volume.root.install_child(&staging(c.attempt,c.slot),&dst,&c.directory.birth)?;
    }
    // A prior rename can be visible before its parent sync. Reconfirm the
    // exact installed material and then complete that original durability cut.
    installed(volume,c)?;volume.root.file.sync_all()?;installed(volume,c)
}
