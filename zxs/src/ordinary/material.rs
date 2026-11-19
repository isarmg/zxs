//! Keep logical FILE birth inside the container and physical custody in SYSTEM.
//! The exclusive volume lease remains held until source, target and all names
//! have been verified. Unattributed private staging is never adopted or swept.
use super::allocation::{Allocation,Stage,SLOTS};
use super::wire::{Creation,NativeObject,empty_material,human_name};
use crate::{OperationId,fs::{self,Birth,HeldDir,invalid},pool::lease::FileVolume};
use std::{ffi::{CStr,CString},io};

const LOCAL:&CStr=c"FILE";
const MAX_LOCAL:usize=512;
pub(super) fn staging(op:OperationId,slot:u8)->CString{
    CString::new(format!(".zxs-new-{op}-{slot}")).expect("fixed ASCII stage")
}
fn destination(name:&str)->io::Result<CString>{
    human_name(name.as_bytes())?;
    CString::new(name.as_bytes()).map_err(|_|invalid("invalid FILE destination"))
}
/// Recognizing a distinct reserved *spelling* is not permission to look
/// inside, adopt or clean up any such physical entry.
pub(super) fn private_slot(raw:&[u8])->Option<(OperationId,u8)>{
    let tail=raw.strip_prefix(b".zxs-new-")?;
    if tail.len()!=34 || tail[32]!=b'-'{return None;}
    let slot=tail[33].checked_sub(b'0')?;
    if slot>=SLOTS{return None;}
    Some((std::str::from_utf8(&tail[..32]).ok()?.parse().ok()?,slot))
}
pub(in crate::ordinary) fn check_siblings(names:&[Vec<u8>],name:&str,allow_target:bool)->io::Result<()>{
    human_name(name.as_bytes())?;
    for raw in names{
        if raw.eq_ignore_ascii_case(name.as_bytes()) && !(allow_target&&raw.as_slice()==name.as_bytes()){
            return Err(io::Error::new(io::ErrorKind::AlreadyExists,"FILE destination or equivalent name occupied"));
        }
        // Unsupported Unicode, trailing spaces/dots, noncanonical foreign
        // spellings etc. cannot silently pass this partial equivalence policy.
        if private_slot(raw).is_none()&&human_name(raw).is_err(){
            return Err(invalid("a physical sibling is outside the reviewed filename comparison domain"));
        }
    }
    Ok(())
}
fn check_name(volume:&FileVolume<'_>,name:&str,allow_target:bool)->io::Result<()>{
    check_siblings(&volume.root.names()?,name,allow_target)
}
pub(in crate::ordinary) fn vacant_for_move(v:&FileVolume<'_>,name:&str)->io::Result<()>{
    v.require_current()?;crate::fs::check_path(&v.root.path.join(name))?;check_name(v,name,false)
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
fn ticket_paths<T>(name:&str,ticket:OperationId,mut check:impl FnMut(&std::ffi::CStr)->io::Result<T>)->io::Result<T>{
    let final_name=check(&destination(name)?)?;
    for slot in 0..SLOTS{check(&staging(ticket,slot))?;}
    Ok(final_name)
}
pub(in crate::ordinary) fn check_ticket_paths(parent:&HeldDir,name:&str,ticket:OperationId)->io::Result<()>{
    ticket_paths(name,ticket,|n|parent.checked_child_path(n)).map(|_|())
}
/// The same issued names, checked from the authenticated parent's cumulative
/// extent rather than a filesystem listing. Cache only the business child.
pub(in crate::ordinary) fn ticket_budget(parent:crate::fs::PathBudget,name:&str,ticket:OperationId)->io::Result<crate::fs::PathBudget>{
    ticket_paths(name,ticket,|n|parent.child(n))
}

/// New residence in a Child must remain addressable for a future source
/// isolation. Only the canonical name width is used: this neither reserves a
/// future MOVE nor opens R. Ordinary root birth keeps its original rules.
fn child_residence<T>(name:&str,ticket:OperationId,mut check:impl FnMut(&CStr)->io::Result<T>)->io::Result<T>{
    let human=ticket_paths(name,ticket,&mut check)?;
    check(&super::custody::transfer::isolation_name(ticket))?;Ok(human)
}
pub(in crate::ordinary) fn check_child_residence(parent:&HeldDir,name:&str,ticket:OperationId)->io::Result<()>{
    child_residence(name,ticket,|n|parent.checked_child_path(n)).map(|_|())
}
pub(in crate::ordinary) fn child_residence_budget(parent:crate::fs::PathBudget,name:&str,ticket:OperationId)->io::Result<crate::fs::PathBudget>{
    child_residence(name,ticket,|n|parent.child(n))
}

/// No named write here. The actual root, FileId and this one bounded ticket
/// must be durably reserved in SYSTEM first.
pub(super) fn select_ticket(volume:&FileVolume<'_>,name:&str)->io::Result<(OperationId,NativeObject)>{
    volume.require_current()?;
    let names=volume.root.names()?;check_siblings(&names,name,false)?;
    let attempt=OperationId::new()?;
    check_ticket_paths(&volume.root,name,attempt)?;
    if names.iter().any(|raw|private_slot(raw).is_some_and(|(ticket,_)|ticket==attempt)){
        return Err(invalid("new creation ticket collides with an existing private name"));
    }
    room_for_ticket(&names,name,&staging(attempt,0))?;
    Ok((attempt,NativeObject{entity:volume.root.entity,birth:Birth::read(&volume.root.file)?}))
}

pub(super) fn bind_fresh_stage(volume:&FileVolume<'_>,a:&Allocation,after_mkdir:impl FnOnce()->io::Result<()>)->io::Result<()>{
    a.require_root_writer(volume)?;check_name(volume,&a.name,false)?;
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
    a.require_root_writer(volume)?;check_name(volume,&a.name,false)?;
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
    volume.require_current()?;check_name(volume,&c.name,false)?;
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
    let dir=volume.root.child_directory(&destination(&c.name)?)?.ok_or_else(||invalid("decided FILE not installed or unavailable"))?;
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
    check_name(volume,&c.name,true)?;let dst=destination(&c.name)?;
    if volume.root.child_directory(&dst)?.is_none(){
        require_staged(volume,c)?;
        volume.root.install_child(&staging(c.attempt,c.slot),&dst,&c.directory.birth)?;
    }
    // A prior rename can be visible before its parent sync. Reconfirm the
    // exact installed material and then complete that original durability cut.
    installed(volume,c)?;volume.root.file.sync_all()?;installed(volume,c)
}
