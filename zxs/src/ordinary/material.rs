//! Keep logical FILE birth inside the container and physical custody in SYSTEM.
//! The exclusive volume lease remains held until source, target and all names
//! have been verified. Unattributed private staging is never adopted or swept.
use super::{allocation::{Allocation,SLOTS},directory::path::HeldParent};
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
pub(in crate::ordinary) fn vacant_for_move(p:&HeldParent<'_,'_>,display:&str,file:FileId)->io::Result<()>{
    let n=file_leaf(display,file)?;fs::NameBudget::held(p.directory())?.child(&n)?;
    check_birth_name(p,display,file,false).map(|_|())
}
pub(in crate::ordinary) fn room_for_ticket(names:&[Vec<u8>],name:&str,stage:&CStr)->io::Result<()>{
    let bytes:usize=names.iter().map(|n|1+n.len()).sum();let n=usize::from(SLOTS);
    if names.len()+n>fs::DIRECTORY_NAMES || bytes+n*(1+name.len().max(stage.to_bytes().len()))>fs::DIRECTORY_BYTES {
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
    let(_,cut)=volume.pool.snapshot()?;let h=HeldParent::at_cut(volume,crate::DirectoryParent::Root(volume.id()),&cut)?;
    select_at(&h,name,file)
}
pub(in crate::ordinary) fn select_at(h:&HeldParent<'_,'_>,name:&str,file:FileId)->io::Result<(OperationId,NativeObject)>{
    let volume=h.volume();h.require()?;let attempt=OperationId::new()?;let leaf=file_leaf(name,file)?;
    held_residence(volume,h.directory(),name,file,attempt)?;
    let names=check_birth_name(h,name,file,false)?;
    for i in 0..SLOTS{check_leaf(h.directory(),&names,&staging(attempt,i),false)?;}
    check_leaf(h.directory(),&names,&TicketLeaf::isolation(attempt)?.name(),false)?;
    room_for_ticket(&names,leaf.to_str().map_err(|_|invalid("invalid derived container"))?,&staging(attempt,0))?;
    Ok((attempt,NativeObject{entity:volume.root.entity,birth:Birth::read(&volume.root.file)?}))
}

pub(super) fn check_birth_name(parent:&HeldParent<'_,'_>,display:&str,file:FileId,allow:bool)->io::Result<Vec<Vec<u8>>>{
    let raw=parent.directory().names()?;let(_,s)=parent.volume().pool.snapshot()?;
    super::target_domain::ordinary_parent_target(parent,&s,display,file,&raw)?;
    check_leaf(parent.directory(),&raw,&file_leaf(display,file)?,allow)?;Ok(raw)
}
fn local_token(allocation:[u8;32],local:&NativeObject)->io::Result<[u8;32]>{
    let mut b=b"ZXS/INITIAL-LOCAL-MATERIAL\0".to_vec();b.extend_from_slice(&allocation);local.put(&mut b);
    crate::hash::digest(&b)
}
pub(super) fn prepare(parent:&HeldParent<'_,'_>,a:&Allocation)->io::Result<Creation>{
    a.require_writer(parent)?;check_birth_name(parent,&a.name,a.file,false)?;let root=parent.directory();
    let s=a.bound()?;
    let dir=root.exact_directory(&staging(a.attempt,s.slot))?.ok_or_else(||invalid("reserved physical FILE stage disappeared"))?;
    super::directory::inventory::same_child(&dir,&s.object)?;let directory=s.object.clone();
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
    dir.file.sync_all()?;root.file.sync_all()?;
    let c=Creation{parent:a.parent.clone(),op:a.op,uid:a.uid,volume:parent.volume().id(),entry:a.entry,file:a.file,history:a.history,
        name:a.name.clone(),attempt:a.attempt,slot:s.slot,allocation:a.digest,directory,local,local_digest:crate::hash::digest(&bytes)?};
    require_staged(parent,&c)?;Ok(c)
}
fn verify_header(dir:&HeldDir,c:&Creation)->io::Result<()>{
    verify_header_at(dir,c,&c.directory,&c.local)
}
pub(in crate::ordinary) fn verify_header_at(dir:&HeldDir,c:&Creation,container:&NativeObject,local:&NativeObject)->io::Result<()>{
    super::directory::inventory::same_child(dir,container)?;
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
pub(super) fn require_staged(parent:&HeldParent<'_,'_>,c:&Creation)->io::Result<()>{
    if c.volume!=parent.volume().id()||!c.parent.identifies(c.volume,parent.basis()){return Err(invalid("wrong original FILE birth parent"));}
    check_birth_name(parent,&c.name,c.file,false)?;
    let root=parent.directory();let dir=root.exact_directory(&staging(c.attempt,c.slot))?.ok_or_else(||invalid("original prepared FILE is missing"))?;
    verify_directory(&dir,c)?;dir.file.sync_all()?;root.file.sync_all()?;parent.require()
}
fn installed(parent:&HeldParent<'_,'_>,c:&Creation)->io::Result<()>{
    if c.volume!=parent.volume().id()||!c.parent.identifies(c.volume,parent.basis()){return Err(invalid("wrong original installation parent"));}
    let root=parent.directory();let dir=root.exact_directory(&file_leaf(&c.name,c.file)?)?.ok_or_else(||invalid("decided FILE not installed or unavailable"))?;
    verify_directory(&dir,c)?;
    if root.exact_directory(&staging(c.attempt,c.slot))?.is_some(){return Err(invalid("both FILE names remain present"));}
    parent.require()
}
pub(super) fn install_decided(parent:&HeldParent<'_,'_>,c:&Creation)->io::Result<()>{
    let v=parent.volume();if !matches!(v.pool.access,crate::pool::lease::Access::Write)||c.volume!=v.id()||!c.parent.identifies(v.id(),parent.basis()){
        return Err(invalid("not an original FILE installation on this issuing parent writer"));}
    let(_,s)=v.pool.snapshot()?;parent.require_at(&s)?;
    let original=s.operation(c.op).ok_or_else(||invalid("no confirmed original FILE creation"))?;
    if &original.create!=c||!s.allocated.as_ref().is_some_and(|a|a.matches(original)){
        return Err(invalid("no matching creation and current parent installation reservation"));}
    check_birth_name(parent,&c.name,c.file,true)?;let dst=file_leaf(&c.name,c.file)?;let root=parent.directory();
    if root.exact_directory(&dst)?.is_none(){
        require_staged(parent,c)?;root.install_child(&staging(c.attempt,c.slot),&dst,&c.directory.birth)?;
    }
    // A cold process can observe the rename before this *parent's* sync.
    installed(parent,c)?;root.file.sync_all()?;installed(parent,c)
}
