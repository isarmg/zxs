//! H04 read-only evidence for one explicitly selected original F. v18
//! has no retirement operation, unpublish or state writer. A clear observation
//! is not a durable receipt, permission to retire, or a reusable lease.
use super::{Id,observed};
use crate::{maintenance::HostMounts,pinned::{Entity,Kind,Mount,Pinned},sqlite_install::{MARKER,bad,
    catalog::{self,Canonical},engine::{self,Access},markers::{OrdinaryMark,SystemMark},
    originals::{HeldRoot,LockedSystem,named_backing},reservation::Existing}};
use rusqlite::{Transaction,params};
use rustix::fs::{FlockOperation,RawDir,flock};
use std::{fs::File,io,mem::MaybeUninit,os::unix::{ffi::OsStrExt,fs::MetadataExt},path::{Path,PathBuf}};

const ROW_CEILING:i64=2048;
const V17_TABLES:&[&str]=&[
    "operation","pool_birth","volume_claim","account_pair","web_password","credential_reset",
    "smb_rust_revision","root_share","share_mode","root_entry","file_location","directory_entry","directory_abort",
    "directory_location","directory_rename","directory_rename_abort","file_transfer","file_transfer_source",
    "file_transfer_void","file_rename","file_rename_abort","first_work","work_edit","manual_node",
];
fn unknown()->io::Error{bad("cannot prove a complete original ordinary-volume retirement inventory")}
fn sql(_:rusqlite::Error)->io::Error{unknown()}

#[derive(Debug)]
pub(super) struct Scope{pool:Id,init:Id,volume:Id,system:PathBuf,ordinary:PathBuf}
impl Scope{
    pub(super) fn new(pool:Id,init:Id,volume:Id,system:PathBuf,ordinary:PathBuf)->io::Result<Self>{
        for p in [&system,&ordinary]{Kind::StorageRoot.check_path(p.as_os_str().as_bytes()).map_err(bad)?;}
        if [pool,init,volume].contains(&[0;16])||pool==volume||system.to_str().is_none()
            ||ordinary.starts_with(&system)||system.starts_with(&ordinary){return Err(unknown());}
        Ok(Self{pool,init,volume,system,ordinary})
    }
    pub(super) fn ids(&self)->[Id;3]{[self.pool,self.init,self.volume]}
    pub(super) fn system(&self)->&Path{&self.system}
    pub(super) fn ordinary(&self)->&Path{&self.ordinary}
}

#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct Responsibilities{
    pub(super) publication:bool,pub(super) selected_file:bool,pub(super) directory_origin:bool,
    pub(super) file_birth:bool,pub(super) file_rename_history:bool,pub(super) transfer_history:bool,
    pub(super) pending_transfer:bool,pub(super) armed_cleanup:bool,pub(super) unproved_cleanup:bool,
}
impl Responsibilities{
    fn any(&self)->bool{self.publication||self.selected_file||self.directory_origin||self.file_birth||self.file_rename_history
        ||self.transfer_history||self.pending_transfer||self.armed_cleanup||self.unproved_cleanup}
}

/// Metadata snapshot only. Even with no blockers, nothing is unregistered.
/// The INIT ID is historical provenance, never a retirement ticket. No
/// retirement operation ID, grant or encoded proof exits this observer.
#[derive(Debug)]
pub(super) struct Preflight{
    pub(super) ordinal:u8,pub(super) volume:Id,pub(super) original_init:Id,
    pub(super) witnesses:Responsibilities,pub(super) only_ordinary:bool,pub(super) extra_entry:bool,
}
impl Preflight{
    pub(super) fn no_observed_blockers(&self)->bool{!self.only_ordinary&&!self.extra_entry&&!self.witnesses.any()}
}

#[derive(Debug)]
struct ProtectedVolume;
impl std::fmt::Display for ProtectedVolume{
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{
        f.write_str("SYSTEM and the sole recycle volume cannot be retired")
    }
}
impl std::error::Error for ProtectedVolume{}
pub(super) fn protected_volume(e:&io::Error)->bool{e.get_ref().is_some_and(|x|x.is::<ProtectedVolume>())}

pub(super) fn compiled_v18()->io::Result<()>{super::fresh_issuer::compiled_v18()}
fn template()->io::Result<Canonical>{compiled_v18()?;Canonical::new()}

/// Some v18 relationships have no index by volume. Bound each complete
/// input first, in the SAME transaction. A partial search never gives a
/// negative witness. Only static table names, no raw row data, are used.
fn complete_small_cut(tx:&Transaction<'_>)->io::Result<()>{
    for name in V17_TABLES{
        let query=format!("SELECT count(*) FROM (SELECT 1 FROM main.{name} LIMIT ?1)");
        let n:i64=tx.query_row(&query,[ROW_CEILING+1],|r|r.get(0)).map_err(sql)?;
        if !(0..=ROW_CEILING).contains(&n){return Err(bad("retirement inventory exceeds the bounded v18 snapshot; not cleared"));}
    }
    let mut verify=tx.prepare("PRAGMA main.foreign_key_check").map_err(sql)?;
    if verify.column_count()!=4{return Err(unknown());}
    if verify.query([]).map_err(sql)?.next().map_err(sql)?.is_some(){return Err(unknown());}
    let orphan:bool=tx.query_row(include_str!("retire_ordinary/unbound-pending.sql"),[],|r|r.get(0)).map_err(sql)?;
    if orphan{return Err(unknown());}
    Ok(())
}
fn responsibilities(tx:&Transaction<'_>,volume:&Id)->io::Result<Responsibilities>{
    tx.query_row(include_str!("retire_ordinary/responsibilities.sql"),[&volume[..]],|r|Ok(Responsibilities{
        publication:r.get(0)?,selected_file:r.get(1)?,directory_origin:r.get(2)?,file_birth:r.get(3)?,
        file_rename_history:r.get(4)?,transfer_history:r.get(5)?,pending_transfer:r.get(6)?,
        armed_cleanup:r.get(7)?,unproved_cleanup:r.get(8)?,
    })).map_err(sql)
}

#[derive(Eq,PartialEq)]
struct Selection{marker:[u8;101],ordinal:u8,limit:u8,recycle_name:PathBuf,other_names:Vec<PathBuf>,findings:Responsibilities}
fn at_scope(tx:&Transaction<'_>,template:&Canonical,s:HeldRoot<'_>,scope:&Scope)->io::Result<Selection>{
    if s.path().as_os_str().as_bytes()!=scope.system.as_os_str().as_bytes(){return Err(unknown());}
    let claims=catalog::initial_metadata(tx,template,s,&scope.pool,&scope.init)?;
    let recycle_name=catalog::initial_recycle(&claims)?.path.clone();
    let (n,claim)=claims.iter().enumerate().find(|(_,r)|r.marker[37..53]==scope.volume).ok_or_else(unknown)?;
    if n<2||claim.marker[20]!=2{return Err(io::Error::new(io::ErrorKind::PermissionDenied,ProtectedVolume));}
    if claim.path.as_os_str().as_bytes()!=scope.ordinary.as_os_str().as_bytes(){return Err(unknown());}
    let limit:i64=tx.query_row("SELECT component_limit FROM main.volume_claim
        WHERE claim_op=?1 AND ordinal=?2 AND role=2 AND volume_id=?3 AND root_path=?4",
        params![&scope.init[..],n as i64,&scope.volume[..],scope.ordinary.as_os_str().as_bytes()],|r|r.get(0)).map_err(sql)?;
    let limit=u8::try_from(limit).ok().filter(|l|usize::from(*l)>=MARKER.len()).ok_or_else(unknown)?;
    complete_small_cut(tx)?;
    Ok(Selection{marker:claim.marker,ordinal:n as u8,limit,recycle_name,
        other_names:claims.iter().enumerate().filter(|(i,_)|*i!=n).map(|(_,r)|r.path.clone()).collect(),
        findings:responsibilities(tx,&scope.volume)?})
}

fn disjoint(a:&Pinned,b:&Pinned)->bool{
    a.entity()!=b.entity()&&!a.entity_is_visible_ancestor_of(b)&&!b.entity_is_visible_ancestor_of(a)
}
#[derive(Eq,PartialEq)]
struct Identity{entry:Entity,mount:Mount,backing:u64,times:[i64;4],length:u64}
fn actual(pin:&Pinned,f:&File,host:&File,uid:u32,expected_limit:u8)->io::Result<Identity>{
    let m=f.metadata()?;let entry=Entity::of(&m)?;
    let mount=Mount::on(f)?.ok_or_else(unknown)?;
    if m.mode()!=0o040700||m.uid()!=uid||m.nlink()==0||pin.entity()!=entry||pin.require_same_named_mount()?!=mount
        ||helper_linux_evidence::portable_component_limit(f)?!=expected_limit{return Err(unknown());}
    let backing=named_backing(pin,f,&HostMounts::current()?,host,uid)?;
    Ok(Identity{entry,mount,backing,length:m.len(),times:[m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec()]})
}
fn recycle_is_distinct(choice:&Selection,f:&Identity)->io::Result<()> {
    // Negative alias check only: no R data FD, marker read, lease or claim
    // regarding its actual presence, health or rights. Unproved stays unknown.
    let r=Pinned::already_mounted(&choice.recycle_name,Kind::StorageRoot)?;
    let here=r.require_same_named_mount()?;
    let backing=HostMounts::current()?.full_mount(here.id)?.1;
    if f.entry.dev==r.entity().dev||f.mount.id==here.id||f.backing==backing{return Err(unknown());}
    if r.require_same_named_mount()?!=here{return Err(unknown());}
    Ok(())
}
struct OriginalF{pin:Pinned,dir:File,marker:OrdinaryMark,identity:Identity}
impl OriginalF{
    fn select(scope:&Scope,choice:&Selection,s:&Pinned,host:&File,uid:u32)->io::Result<Self>{
        // Only the explicitly supplied F name can supply a data descriptor.
        // A row with a path can never authorize an open on its own.
        let pin=Pinned::already_mounted(&scope.ordinary,Kind::StorageRoot)?;
        Self::other_names(&pin,s,choice)?;
        let dir=pin.storage_directory()?;
        flock(&dir,FlockOperation::NonBlockingLockExclusive)?;
        let identity=actual(&pin,&dir,host,uid,choice.limit)?;
        recycle_is_distinct(choice,&identity)?;
        let marker=OrdinaryMark::select(&dir,uid,&choice.marker)?;
        Ok(Self{pin,dir,marker,identity})
    }
    fn other_names(pin:&Pinned,s:&Pinned,choice:&Selection)->io::Result<()>{
        if !disjoint(pin,s){return Err(unknown());}
        // Unselected paths are at most passive name/alias evidence. No root
        // data opens, marker reads, or locks for them. Missing alias proof is
        // unknown in this bounded first preflight, not authorization.
        for name in choice.other_names.iter().filter(|p|p.as_path()!=s.path()){
            let entry=Pinned::already_mounted(name,Kind::StorageRoot)?;
            entry.require_same_named_mount()?;
            if !disjoint(pin,&entry){return Err(unknown());}
        }
        Ok(())
    }
    fn require(&self,s:&Pinned,host:&File,uid:u32,choice:&Selection)->io::Result<()>{
        Self::other_names(&self.pin,s,choice)?;
        if actual(&self.pin,&self.dir,host,uid,choice.limit)?!=self.identity{return Err(unknown());}
        recycle_is_distinct(choice,&self.identity)?;
        self.marker.verify(&self.dir,&choice.marker)
    }
    fn probe(&self,s:&Pinned,host:&File,uid:u32,choice:&Selection)->io::Result<bool>{
        self.require(s,host,uid,choice)?;
        let reader=self.pin.storage_directory()?;
        if Entity::of(&reader.metadata()?)?!=self.identity.entry||Mount::on(&reader)?!=Some(self.identity.mount){return Err(unknown());}
        let mut buf=[MaybeUninit::uninit();4096];let mut entries=RawDir::new(&reader,&mut buf);
        let (mut saw_marker,mut extra)=(false,false);let mut seen=0;
        while let Some(entry)=entries.next(){
            let entry=entry?;seen+=1;if seen>4{return Err(unknown());}
            match entry.file_name().to_bytes(){
                b"."|b".."=>(),
                name if name==MARKER&&!saw_marker=>saw_marker=true,
                name if name==MARKER=>return Err(unknown()),
                _=>{extra=true;break;}, // occupied, no need to open or report a name
            }
        }
        if !extra&&!saw_marker{return Err(unknown());} // clean requires original name and real EOF
        self.require(s,host,uid,choice)?;Ok(extra)
    }
}

/// The authenticated lowered read worker lends the exclusive original S.
/// The bounded observer is called while the original F and its marker are
/// still held. It receives facts only, never a reusable lock or marker token.
pub(super) fn observe<T>(locked:&LockedSystem,scope:&Scope,actor:&Id,report:impl FnOnce(Preflight)->T)->io::Result<T>{
    let (host,s)=locked.retirement_host()?;
    let tpl=template()?;let original=Existing::observed_system(locked)?;let mark=SystemMark::before_sql(locked.source())?;
    let mut anchor=engine::existing(&original,Access::Read)?;
    let read=|tx:&Transaction<'_>|{
        let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.pool_birth b JOIN main.operation i
            ON i.op_id=b.init_op AND i.kind=1 AND i.state=1
            WHERE b.singleton=1 AND b.pool_id=?1 AND b.init_op=?2 AND i.actor_id=?3)",
            params![&scope.pool[..],&scope.init[..],&actor[..]],|r|r.get(0)).map_err(sql)?;
        if !valid{return Err(unknown());}
        at_scope(tx,&tpl,locked.source(),scope)
    };
    let (before,choice)=observed::snapshot(&mut anchor,&scope.pool,&scope.init,locked,&original,&tpl,&mark,read)?;
    let uid=locked.source().uid();
    let f=OriginalF::select(scope,&choice,s,host,uid)?;
    let occupied=f.probe(s,host,uid,&choice)?;
    // Keep the first VFS anchor alive, without a transaction, while obtaining
    // the second independent readonly cut. No SQL is open during F inventory.
    if !anchor.is_autocommit(){return Err(unknown());}engine::original_name(&anchor)?;
    let (after,repeat)=observed::at_original(&scope.pool,&scope.init,locked,&original,&tpl,&mark,read)?;
    if after!=before||repeat!=choice||f.probe(s,host,uid,&choice)?!=occupied{return Err(unknown());}
    engine::original_name(&anchor)?;original.sidecars_absent()?;mark.verify(locked.source(),&before)?;
    anchor.close().map_err(|_|unknown())?;
    locked.retirement_host()?;original.sidecars_absent()?;mark.verify(locked.source(),&before)?;
    f.require(s,host,uid,&choice)?;
    let value=report(Preflight{volume:scope.volume,original_init:scope.init,ordinal:choice.ordinal,
        witnesses:choice.findings,only_ordinary:choice.other_names.len()==2,extra_entry:occupied});
    drop(f);Ok(value)
}

#[cfg(test)]
fn inspect(locked:&LockedSystem,scope:&Scope)->io::Result<Preflight>{observe(locked,scope,&[2;16],|v|v)}

#[cfg(test)]
#[path="../../../tests/sqlite/retire_ordinary.rs"] mod cases;
