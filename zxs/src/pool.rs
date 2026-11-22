//! One native registration decision: install an immutable SYSTEM reservation,
//! durably claim only its original selected roots, then rename it to REGISTRATION.
//! A failed earlier step is either invisible or resumable under the same OpId.
use super::*;
use std::ffi::CStr;
use std::os::unix::ffi::OsStrExt;
mod format;
// Retired private decision-stream experiment. Its regression fixtures remain,
// but a production SQLite pool can never fall through to this authority.
#[cfg(test)]
mod portable_claim;
#[cfg(test)]
mod portable;
pub(crate) mod lease;
use format::{MAX_RECORD, Record};

const PENDING:&CStr=c"INITIALIZING";
pub(crate) const READY:&CStr=c"REGISTRATION";
pub(crate) const MAX_SYSTEM:usize=MAX_RECORD+crate::ordinary::journal::MAX_JOURNAL;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredVolume {pub id:VolumeId,pub volume:ObservedVolume}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PoolInfo {pub id:PoolId,pub initialized_by:OperationId,pub volumes:Vec<RegisteredVolume>}

fn canonical_files(files:&[PathBuf])->io::Result<Vec<PathBuf>> {
    if files.is_empty()||files.len()>MAX_FILE_ROOTS{return Err(invalid("invalid FILE-root count"));}
    let mut files=files.to_vec();
    files.sort_by(|l,r|l.as_os_str().as_bytes().cmp(r.as_os_str().as_bytes()));
    Ok(files)
}
fn verify_claims(selection:&Selection,record:&Record,allow_missing:bool)->io::Result<()>{
    record.check_registered_limits(selection)?;
    for (i,(_,dir)) in selection.roots.iter().enumerate(){
        dir.require_named()?;
        record.require_root(i,dir)?;
        let wanted=record.label(i);
        match dir.label()?{
            Some(actual) if actual==wanted=>{},
            None if allow_missing=>{},
            _=> return Err(invalid("selected volume does not carry its original role and pool identity")),
        }
    }
    Ok(())
}

/// The authenticated record already read from the exact original SYSTEM inode.
/// Native replay cannot mix a pool/initializer/digest from another register,
/// infer a role from a pathname, or pass a permissive per-disk predicate.
pub(crate) struct JournalRegistration<'a>(&'a Record);
impl JournalRegistration<'_>{
    pub(crate) fn basis(&self)->(PoolId,OperationId,[u8;32]){(self.0.info.id,self.0.info.initialized_by,self.0.digest())}
    pub(crate) fn file(&self,id:VolumeId,uid:u32,e:Entity,b:&fs::Birth,root:bool)->bool{self.0.allows_file(id,uid,e,b,root)}
    pub(crate) fn ordinary_path_budget(&self,id:VolumeId,uid:u32)->io::Result<fs::PathBudget>{
        if self.0.actor_uid!=uid{return Err(invalid("different original registration principal"));}
        let v=self.0.info.volumes.iter().find(|v|v.id==id&&v.volume.role==Role::File)
            .ok_or_else(||invalid("ordinary path budget requires the original registered FILE root"))?;
        fs::path_budget(&v.volume.path)
    }
    pub(crate) fn ordinary_names(&self,id:VolumeId,uid:u32,parent:fs::PathBudget)->io::Result<fs::NameBudget>{
        Ok(fs::NameBudget::from_original(parent,self.0.ordinary_limit(id,uid)?))
    }
    pub(crate) fn recycle_names(&self,uid:u32)->io::Result<fs::NameBudget>{self.0.dedicated_name_budget(uid)}
    pub(crate) fn directory_child(&self,id:VolumeId,uid:u32,e:Entity,b:&fs::Birth)->bool{self.0.directory_child(id,uid,e,b)}
    pub(crate) fn dedicated(&self,uid:u32,d:lease::recycle::Direction,f:(VolumeId,Entity,&fs::Birth),t:(VolumeId,Entity,&fs::Birth))->bool{
        self.0.dedicated_route(uid,d,f.0,t.0).is_ok_and(|route|route.matches(f,t))
    }
}

struct Published {stamp:fs::OriginalStamp,record:Record,journal:crate::ordinary::journal::Journal}
fn published(dir:&HeldDir)->io::Result<Option<Published>>{
    published_with_sync(dir,|dir,stamp|dir.confirm_record_sync(READY,stamp,MAX_SYSTEM))
}
fn published_with_sync(dir:&HeldDir,confirm:impl FnOnce(&HeldDir,&fs::OriginalStamp)->io::Result<()>)->io::Result<Option<Published>>{
    let Some((stamp,bytes))=dir.record(READY,MAX_SYSTEM)? else{return Ok(None)};
    read_published(dir,stamp,&bytes,confirm).map(Some)
}
/// One admission for known held original bytes. Only the ID-LEAF-1 business
/// decoder can accept them here; parsing a MAINT-1 empty image is not a data grant.
fn read_published(dir:&HeldDir,stamp:fs::OriginalStamp,bytes:&[u8],
    confirm:impl FnOnce(&HeldDir,&fs::OriginalStamp)->io::Result<()>)->io::Result<Published>{
    let (record,tail)=Record::prefix(bytes)?;
    if record.origin!=stamp.stat.entity()||record.origin_birth!=stamp.birth||record.actor_uid!=fs::local_uid(){
        return Err(invalid("SYSTEM journal is not its original registered inode"));
    }
    require_system(dir,&record)?;
    let frontier=dir.read_frontier(READY,&stamp,MAX_SYSTEM)?;
    let journal=crate::ordinary::journal::Journal::read(dir,JournalRegistration(&record),tail,frontier,&stamp)?;
    confirm(dir,&stamp)?;
    Ok(Published{stamp,record,journal})
}


fn current_record(dir:&HeldDir,name:&CStr)->io::Result<Option<(fs::OriginalStamp,Record)>>{
    if name==READY{return Ok(published(dir)?.map(|r|(r.stamp,r.record)));}
    dir.record(name,MAX_RECORD)?.map(|(stamp,b)|{
        let r=Record::read(b)?;
        if r.origin!=stamp.stat.entity() || r.origin_birth!=stamp.birth || r.actor_uid!=fs::local_uid() {
            return Err(invalid("registration is not its original SYSTEM inode or local owner"));
        }
        if dir.read_frontier(name,&stamp,MAX_RECORD)?!=fs::frontier(0,0,r.digest()){
            return Err(invalid("unpublished SYSTEM registration does not have its original genesis frontier"));
        }
        Ok((stamp,r))
    }).transpose()
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum InitStep { Reservation, Label(usize), BeforePublication, RenamedBeforeSync, Publication }

pub fn initialize(op:OperationId, system:&Path, recycle:&Path, files:&[PathBuf])->io::Result<PoolInfo>{
    initialize_inner(op,system,recycle,files,|_|Ok(()))
}
fn initialize_inner(op:OperationId, system:&Path, recycle:&Path, files:&[PathBuf],
    mut checkpoint:impl FnMut(InitStep)->io::Result<()>)->io::Result<PoolInfo>{
    let files=canonical_files(files)?;
    fs::check_path(recycle)?;
    for path in &files {fs::check_path(path)?;}
    let held_system=HeldDir::open(system)?;
    held_system.exclusive()?;held_system.require_named()?;
    held_system.reject_registered_ancestors(&std::collections::BTreeSet::new())?;
    let ready=current_record(&held_system,READY)?;
    let previous=current_record(&held_system,PENDING)?;
    if let Some((stamp,r))=ready {
        if previous.is_some(){return Err(invalid("both pending and published registration exist"));}
        r.matches_request(system,recycle,&files,op)?;
        require_system(&held_system,&r)?;
        // A prior rename may have succeeded before its directory sync failed.
        // Finish only that original durability boundary, never (re)claim data.
        held_system.file.sync_all()?;
        require_original(&held_system,READY,&stamp,&r)?;
        return Ok(r.info); // Exact historical registration, not current volume availability.
    }
    let selection=Selection::with_system(held_system,recycle,&files)?;
    for (_,dir) in &selection.roots[1..]{dir.exclusive()?;}
    selection.require_lineage()?;
    let sys=&selection.roots[0].1;

    let (stamp,record)=if let Some((stamp,record))=previous {
        // Do not borrow a partly prepared pool from a different op, even if
        // the fixed physical roots and their role names happen to match.
        record.matches(&selection,op)?;
        sys.only(Some(PENDING))?;
        (stamp,record)
    }else{
        for (_,dir) in &selection.roots{dir.require_fresh()?;}
        let (original,origin)=sys.anonymous_record()?;
        let own_birth=fs::Birth::read(&original)?;
        let r=Record::new(&selection,op,origin,own_birth)?;
        fs::initial_frontier(&original,r.digest())?;
        // This is the first visible write and occurs only on selected SYSTEM.
        // Verified complete bytes are published atomically from an unnamed inode.
        sys.add_record(original,PENDING,&r.bytes)?;
        let (stamp,observed)=current_record(sys,PENDING)?.ok_or_else(||invalid("registration reservation missing"))?;
        if observed.bytes!=r.bytes{return Err(invalid("unexpected original registration reservation"));}
        (stamp,observed)
    };
    checkpoint(InitStep::Reservation)?;
    // Always finish the entire read-only admission before changing the first label.
    for (_,dir) in &selection.roots[1..]{dir.only(None)?;}
    verify_claims(&selection,&record,true)?;
    // The link could already be visible after a prior directory-sync failure.
    // Its durable reservation must precede the first claim on any data root.
    sys.file.sync_all()?;
    require_original(sys,PENDING,&stamp,&record)?;
    selection.require_lineage()?;
    for (i,(_,dir)) in selection.roots.iter().enumerate(){
        dir.require_named()?;
        record.require_root(i,dir)?;
        let label=record.label(i);
        match dir.label()?{
            None=>dir.claim(&label)?,
            Some(old) if old==label=>dir.file.sync_all()?,
            _=>return Err(invalid("volume claimed by another original operation")),
        }
        checkpoint(InitStep::Label(i))?;
    }
    verify_claims(&selection,&record,false)?;
    for (_,dir) in &selection.roots[1..]{dir.only(None)?;}
    sys.only(Some(PENDING))?;
    require_original(sys,PENDING,&stamp,&record)?;
    selection.require_lineage()?;
    checkpoint(InitStep::BeforePublication)?;
    sys.rename_registration(PENDING,READY)?;
    checkpoint(InitStep::RenamedBeforeSync)?;
    sys.file.sync_all()?;
    checkpoint(InitStep::Publication)?;
    let (new_stamp,actual)=current_record(sys,READY)?.ok_or_else(||invalid("published registration missing"))?;
    if new_stamp.stat.entity()!=stamp.stat.entity() || new_stamp.birth!=stamp.birth || actual.bytes!=record.bytes{return Err(invalid("published registration differs from original"));}
    verify_claims(&selection,&record,false)?;
    Ok(record.info)
}

fn require_system(dir:&HeldDir,r:&Record)->io::Result<()>{
    r.require_root(0,dir)?;
    let root=&r.info.volumes[0].volume;
    if root.entity!=dir.entity||root.path.as_os_str()!=dir.path.as_os_str()
        || dir.label()?.as_deref()!=Some(r.label(0).as_slice()){
        return Err(invalid("SYSTEM register not bound to this original root"));
    }
    dir.require_named()
}
fn require_original(dir:&HeldDir,name:&CStr,stamp:&fs::OriginalStamp,r:&Record)->io::Result<()>{
    let observed=current_record(dir,name)?.ok_or_else(||invalid("original registration disappeared"))?;
    if &observed.0!=stamp||observed.1.bytes!=r.bytes{return Err(invalid("original registration inode or bytes changed"));}
    dir.require_named()
}

/// Read all original held registrations. An unavailable volume is an error;
/// a matching name, arbitrary directory contents or unlabelled media are not used.
pub fn inspect(system:&Path)->io::Result<PoolInfo>{
    inspect_with_sync(system,|sys|sys.file.sync_all())
}
fn inspect_with_sync(system:&Path,sync:impl FnOnce(&HeldDir)->io::Result<()>)->io::Result<PoolInfo>{
    let initial=HeldDir::open(system)?;initial.share()?;
    let (stamp,record)=current_record(&initial,READY)?.ok_or_else(||invalid("SYSTEM has no published registration"))?;
    if initial.record(PENDING,MAX_RECORD)?.is_some(){return Err(invalid("ambiguous SYSTEM registration"));}
    require_system(&initial,&record)?;
    // A visible rename is not a durable publication. A current observer must
    // cross the same original SYSTEM directory sync under its retained lock.
    sync(&initial)?;
    require_original(&initial,READY,&stamp,&record)?;
    let data=record.info.volumes[2..].iter().map(|v|v.volume.path.clone()).collect::<Vec<_>>();
    let selection=Selection::with_system(initial,&record.info.volumes[1].volume.path,&data)?;
    for (_,dir) in &selection.roots[1..]{dir.share()?;}
    selection.require_lineage()?;
    record.matches(&selection,record.info.initialized_by)?;
    verify_claims(&selection,&record,false)?;
    require_original(&selection.roots[0].1,READY,&stamp,&record)?;
    Ok(record.info)
}

#[cfg(test)]
#[path="../tests/unit/registration.rs"]
mod tests;

#[cfg(test)]
include!("../tests/unit/attached/pool/items.rs");
