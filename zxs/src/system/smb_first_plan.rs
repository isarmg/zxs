//! The approved exact boot plan and its original committed setup receipts.
//! Decoding a plan never creates an SMB session or a directory handle.
use super::{Connection,InitialReader,denied};
use super::super::{Catalog,check,share_mode::{self,Mode,Setting}};
use crate::system::reader::credential::{ActiveRustMeta,Pair};
use crate::{OperationId,Role,VolumeId,display,fs::invalid,hash::{Sha256,digest},identity::ShareId};
use std::{collections::BTreeSet,io,rc::{Rc,Weak},sync::Arc};

#[path="smb_rust_maintenance.rs"]
mod maintenance;

#[path="smb_rust_capacity.rs"]
mod capacity;
pub(crate) use capacity::{CapacityUnits,RustRootCapacity};

#[path="smb_rust_preflight.rs"]
pub(super) mod preflight;

pub(crate) const PLAN_MAGIC:&[u8;16]=b"ZXS-RUST-SMB-I2\0";
const RUST_IDS:&[u8]=b"ZXS-RUST-SMB-I2/ID\0";
pub(crate) const PLAN_MAX_BYTES:usize=17_596;
type Id=[u8;16];

pub(super) struct Folder{
    name:String,volume:VolumeId,share:ShareId,publish:OperationId,first:OperationId,mode:Mode,
}
impl Folder{
    pub(super) fn name(&self)->&str{&self.name}
    pub(super) fn share(&self)->ShareId{self.share}
    pub(super) fn volume(&self)->VolumeId{self.volume}
}

pub(super) struct Plan{
    scope:[Id;5], sha:[u8;32], folders:Vec<Folder>, recycle:String,
}

fn take<'a>(p:&mut &'a[u8],len:usize)->io::Result<&'a[u8]>{
    let (out,rest)=p.split_at_checked(len).ok_or_else(denied)?;*p=rest;Ok(out)
}
fn fixed<const N:usize>(p:&mut &[u8])->io::Result<[u8;N]>{
    Ok(take(p,N)?.try_into().expect("exact requested length"))
}
fn scope(p:&mut &[u8])->io::Result<([Id;5],BTreeSet<Id>)>{
    let mut seen=BTreeSet::new();
    let mut scope=[[0u8;16];5];
    for id in &mut scope{
        *id=fixed(p)?;
        if *id==[0;16]||!seen.insert(*id){return Err(denied());}
    }
    Ok((scope,seen))
}
fn host(p:&mut &[u8])->io::Result<String>{
    let [n]=fixed(p)?;
    if !(1..=253).contains(&n){return Err(denied());}
    let host=take(p,usize::from(n))?;
    for (i,label) in host.split(|&b|b==b'.').enumerate(){
        if !(1..=63).contains(&label.len())||(i==0&&(label.len()>15||!label.iter().any(u8::is_ascii_lowercase)))||
           label.first()==Some(&b'-')||label.last()==Some(&b'-')||
           !label.iter().all(|b|b.is_ascii_lowercase()||b.is_ascii_digit()||*b==b'-'){
           return Err(denied());
        }
    }
    Ok(std::str::from_utf8(host).expect("canonical ASCII").to_owned())
}

/// Public descriptor syntax and a nonsecret SQL selection. These values
/// carry no live site lock, vault provenance, protocol proof or OPEN grant.
pub(super) struct RustPlan{plan:Plan,host:String}
impl RustPlan{
    pub(super) fn parse(raw:&[u8])->io::Result<Self>{
        let mut p=raw.strip_prefix(PLAN_MAGIC).filter(|_|raw.len()<=PLAN_MAX_BYTES).ok_or_else(denied)?;
        let (ids,seen)=scope(&mut p)?;
        let host=host(&mut p)?;
        let plan=Plan::parse_namespace(p,ids,seen,digest(raw)?)?;
        Ok(Self{plan,host})
    }
    pub(super) fn hostname(&self)->&str{&self.host}
    /// Same original S cut as the installed folder receipts. The caller must
    /// still prove the actual site/record/lock and authenticate the client.
    pub(super) fn active_at(&self,db:&Connection,c:&Catalog)->io::Result<ActiveRustMeta>{
        let pair=Pair::confirmed(db,c)?.ok_or_else(denied)?;
        let m=pair.active_rust_at(db,c)?.ok_or_else(denied)?;
        if self.plan.scope!=[m.pool.0,m.init.0,m.instance,m.bind,m.setup.0]||
            m.install_sha!=self.plan.sha{return Err(denied());}
        self.plan.require_original_namespace(db,c,&pair)?;
        Ok(m)
    }
}
impl Plan{
    fn parse_namespace(mut p:&[u8],scope:[Id;5],mut seen:BTreeSet<Id>,sha:[u8;32])->io::Result<Self>{
        let [n]=fixed(&mut p)?;
        if !(1..=62).contains(&n){return Err(denied());}
        let mut folders=Vec::<Folder>::with_capacity(n as usize);
        let mut names=BTreeSet::new();
        for _ in 0..n {
            let id=fixed(&mut p)?;
            if id==[0;16]||folders.last().is_some_and(|f|f.volume.0>=id)||!seen.insert(id){return Err(denied());}
            let mode=match fixed(&mut p)?{[1]=>Mode::ReadOnly,[2]=>Mode::ReadWrite,_=>return Err(denied())};
            let k=u16::from_be_bytes(fixed(&mut p)?) as usize;
            let name=display::component(take(&mut p,k)?,display::MAX)?;
            if !names.insert(name){return Err(denied());}
            let next=|kind|{
                let mut h=Sha256::new();h.update(RUST_IDS)?;h.update(&sha)?;
                h.update(&[kind])?;h.update(&id)?;
                Ok::<Id,io::Error>(h.finish()[..16].try_into().expect("derive 16B"))
            };
            folders.push(Folder{name:name.to_owned(),volume:VolumeId(id),mode,share:ShareId(next(1)?),
                publish:OperationId(next(2)?),first:OperationId(next(3)?)});
        }
        let k=u16::from_be_bytes(fixed(&mut p)?) as usize;
        let recycle=display::component(take(&mut p,k)?,display::MAX)?;
        if names.contains(recycle)||!p.is_empty()||sha==[0;32]{return Err(denied());}
        for f in &folders{
            for id in [f.share.0,f.publish.0,f.first.0]{
                if id==[0;16]||!seen.insert(id){return Err(denied());}
            }
        }
        Ok(Self{scope,sha,folders,recycle:recycle.to_owned()})
    }
    pub(super) fn sha(&self)->[u8;32]{self.sha}
    pub(super) fn folders(&self)->&[Folder]{&self.folders}
    fn require_original_namespace(&self,db:&Connection,c:&Catalog,pair:&Pair)->io::Result<()>{
        if !c.rows().get(1).is_some_and(|r|r.ordinal==1&&r.role==Role::Recycle){return Err(denied());}
        for f in self.folders(){self.original(db,c,pair,f)?;}
        Ok(())
    }
    fn original(&self,db:&Connection,c:&Catalog,pair:&Pair,f:&Folder)->io::Result<()>{
        if !c.rows().iter().any(|v|v.role==Role::File&&v.volume==f.volume){return Err(denied());}
        let prefix=|domain:&[u8],op:OperationId|->io::Result<Sha256>{
            let mut h=Sha256::new();h.update(domain)?;
            for v in [c.birth.pool.0,op.0,pair.web.0,pair.setup.0]{h.update(&v)?;}
            h.update(&1i64.to_be_bytes())?;Ok(h)
        };
        let mut pub_sha=prefix(b"ZXS-SQLITE-1/PUBLISH-ROOT\0",f.publish)?;
        pub_sha.update(&f.volume.0)?;pub_sha.update(&(f.name.len() as u16).to_be_bytes())?;pub_sha.update(f.name.as_bytes())?;
        let mut first_sha=prefix(b"ZXS-SQLITE-1/SHARE-MODE\0",f.first)?;
        first_sha.update(&f.share.0)?;first_sha.update(&f.publish.0)?;first_sha.update(&[f.mode as u8])?;
        let s=db.prepare(c"SELECT p.kind,p.state,p.actor_id,p.request_sha,r.share_id,r.author_auth_op,r.volume_id,r.display_label,o.kind,o.state,o.actor_id,o.request_sha,m.share_id,m.previous_op,m.author_auth_op,m.revision,m.mode FROM main.root_share r JOIN main.operation p ON p.op_id=r.publish_op JOIN main.share_mode m ON m.share_id=r.share_id AND m.revision=1 JOIN main.operation o ON o.op_id=m.change_op WHERE r.publish_op=?1 AND m.change_op=?2")?;
        s.bind_bytes(1,&f.publish.0)?;s.bind_bytes(2,&f.first.0)?;
        match s.step(){100=>{},101=>return Err(denied()),rc=>{check(rc)?;return Err(invalid("invalid original planned share receipt"));}}
        if s.integer(0)?!=3||s.integer(1)?!=1||s.array::<16>(2)?!=pair.web.0||s.array::<32>(3)?!=pub_sha.finish()||
            s.array::<16>(4)?!=f.share.0||s.array::<16>(5)?!=pair.setup.0||s.array::<16>(6)?!=f.volume.0||
            !s.blob_eq(7,display::MAX,f.name.as_bytes())?||s.integer(8)?!=4||s.integer(9)?!=1||
            s.array::<16>(10)?!=pair.web.0||s.array::<32>(11)?!=first_sha.finish()||s.array::<16>(12)?!=f.share.0||
            s.array::<16>(13)?!=f.publish.0||s.array::<16>(14)?!=pair.setup.0||s.integer(15)?!=1||s.byte(16)?!=f.mode as u8{
            return Err(denied());
        }
        s.done()
    }
}

/// The running Rust endpoint's original public installation. This holds the
/// actual Core S SH, registry catalogue and initially selected plan and
/// credential together. It is not a protocol authentication or file grant.
/// There is no path/bytes/scalar-only constructor.
#[must_use]
pub(crate) struct QualifiedRustInstall{
    origin:Arc<crate::smb::InheritedRun>,
    source:InitialReader,
    installed:RustPlan,
    original:ActiveRustMeta,
}
impl QualifiedRustInstall{
    pub(crate) fn attach(origin:Arc<crate::smb::InheritedRun>)->io::Result<Self>{
        Self::attach_selected(origin,InitialReader::open)
    }
    fn attach_selected(origin:Arc<crate::smb::InheritedRun>,
        open:impl FnOnce(&std::path::Path,crate::PoolId)->io::Result<InitialReader>)->io::Result<Self>{
        origin.require_live_source()?;
        let installed=RustPlan::parse(origin.rust_plan())?;
        origin.require_listener_binding()?;
        let source=open(origin.system_path(),crate::PoolId(installed.plan.scope[0]))?;
        if source.dir.root.entity!=origin.system_entity(){return Err(denied());}
        let original=source.at(|db,c|installed.active_at(db,&c))?;
        if !origin.current_public_revision_is(original.canonical.0,original.epoch.op.0,original.record,
            &original.sid,original.epoch.generation,&original.login){return Err(denied());}
        origin.require_live_source()?;
        Ok(Self{origin,source,installed,original})
    }

    fn current_ordinary_at(&self,db:&Connection,c:&Catalog,label:&str)->io::Result<Setting>{
        if self.installed.active_at(db,c)?!=self.original{return Err(denied());}
        if label==self.installed.plan.recycle.as_str(){
            return Err(io::Error::new(io::ErrorKind::Unsupported,"the retained namespace requires its own original role"));
        }
        let setting=self.installed.plan.current(db,c,label)?.ok_or(io::ErrorKind::NotFound)?;
        if setting.mode==Mode::Disabled{return Err(denied());}
        Ok(setting)
    }
    fn current_writable_at(&self,source:&InitialReader,db:&Connection,c:&Catalog,label:&str)
        ->io::Result<(crate::identity::AccountId,crate::OperationId,i64,Setting)>{
        if !std::ptr::eq(source,&self.source){return Err(denied());}
        let setting=self.current_ordinary_at(db,c,label)?;
        if setting.mode!=Mode::ReadWrite{return Err(denied());}
        let m=&self.original;
        Ok((m.smb,m.epoch.op,m.epoch.generation,setting))
    }
    pub(in crate::system::reader::smb) fn move_reader(&self,member:&crate::smb::RustSessionWitness)
        ->io::Result<&InitialReader>{
        if !member.is_set_info()||!member.is_current_on(self){return Err(denied());}
        Ok(&self.source)
    }
    pub(in crate::system::reader::smb) fn move_folder_at(
        &self,cut:&crate::system::reader::smb::file_transfer::TransferCut<'_>,label:&str,
    )->io::Result<(crate::identity::AccountId,crate::OperationId,i64,Setting)>{
        let (source,db,c)=cut.parts();
        self.current_writable_at(source,db,c,label)
    }

    /// Current original startup owner and database epoch.
    pub(crate) fn require_current(&self)->io::Result<()>{
        self.origin.require_live_source()?;
        if self.source.dir.root.entity!=self.origin.system_entity(){return Err(denied());}
        let current=self.source.at(|db,c|self.installed.active_at(db,&c))?;
        if current!=self.original{return Err(denied());}
        self.origin.require_live_source()
    }

    // This name (not the client UNC or a resolver) is the trust anchor for
    // the single name/SPN policy. Its presence is not a file/tree grant.
    pub(crate) fn hostname(&self)->&str{self.installed.hostname()}

    pub(crate) fn is_original_start(&self,origin:&crate::smb::InheritedRun)->bool{
        std::ptr::eq(self.origin.as_ref(),origin)
    }

    /// Compare the entire public prefix of the already protected record.
    /// Matching bytes do not establish provenance or authenticate a client.
    pub(crate) fn matches_record_coordinates(&self,ids:&[[u8;16];8],generation:i64,login:&str)->bool{
        let m=&self.original;
        *ids==[m.pool.0,m.init.0,m.setup.0,m.instance,m.bind,m.canonical.0,m.epoch.op.0,m.record]&&
            generation==m.epoch.generation&&login==m.login.as_str()
    }

}


#[derive(Eq,PartialEq)]
struct LogicalChild<'p>{name:&'p str, number:u64, mode:Option<Setting>}
impl Plan{
    /// All visible children in exact display-byte order after a validated
    /// cursor. No media access, no host paths, and no fake disk state.
    fn logical_after<'p>(&'p self,db:&Connection,c:&Catalog,left:&[u8])->io::Result<Vec<LogicalChild<'p>>>{
        let mut selected=Vec::new();
        if self.recycle.as_bytes()>left{
            selected.push(LogicalChild{name:&self.recycle,number:2,mode:None});
        }
        for f in self.folders(){
            if f.name().as_bytes()<=left{continue;}
            let now=self.current(db,c,f.name())?.ok_or_else(denied)?;
            if now.mode==Mode::Disabled{continue;}
            let volume=c.rows().iter().find(|v|v.role==Role::File&&v.volume==f.volume()).ok_or_else(denied)?;
            selected.push(LogicalChild{name:f.name(),number:u64::from(volume.ordinal)+1,mode:Some(now)});
        }
        selected.sort_unstable_by_key(|r|r.name.as_bytes());
        Ok(selected)
    }

    fn current(&self,db:&Connection,c:&Catalog,label:&str)->io::Result<Option<Setting>>{
        let Some(f)=self.folders().iter().find(|f|f.name()==label)else{return Ok(None);};
        let now=share_mode::current(db,c,f.share)?;
        if now.volume!=f.volume||now.revision<1{return Err(denied());}
        Ok(Some(now))
    }
}

/// A pending result from the same authorized logical root, not a wire
/// cursor, search pattern, response, FileId, client or authority for I/O.
#[must_use]
pub(crate) struct RustRootProjection{
    root:Weak<()>,
    // Own the exact mode op and revision of every projected ordinary child.
    // In particular RW->RO->RW does not reuse an already prepared reply.
    children:Vec<(String,u64,Option<Setting>)>,
}
impl RustRootProjection{
    pub(crate) fn children(&self)->impl Iterator<Item=(&str,u64)>+'_{
        self.children.iter().map(|(name,number,_)|(name.as_str(),*number))
    }
    /// A coarse share-level fact, never an access mask for a concrete file.
    pub(crate) fn has_readwrite_child(&self)->bool{
        self.children.iter().any(|(_,_,s)|s.is_some_and(|s|s.mode==Mode::ReadWrite))
    }
}

/// Compare only the exact original tree. This is not a session or an OPEN.
#[must_use]
pub(crate) struct RustRootStamp(Weak<()>);

/// An actual application tree for this same process, installation and
/// authenticated protocol session. It does not own an SMB number or the
/// reply; the wire owner must still publish an exact original TCON.
#[must_use]
pub(crate) struct RustRoot{
    install:Rc<QualifiedRustInstall>,
    session:crate::smb::RustSessionWitness,
    lifetime:Rc<()>,
}
impl RustRoot{
    // The caller retains the actual signed, paid TCON and verifies its
    // canonical host, exact share and real transport from the same member.
    pub(crate) fn connect(install:Rc<QualifiedRustInstall>,session:crate::smb::RustSessionWitness)
        ->io::Result<(Self,RustRootProjection)>{
        if !session.is_tree_connect(){return Err(denied());}
        let tree=Self{install,session,lifetime:Rc::new(())};
        let scope=tree.project(&tree.session)?;
        Ok((tree,scope))
    }
    fn require_member(&self,member:&crate::smb::RustSessionWitness)->io::Result<()>{
        if !self.session.same_session(member)||!member.is_current_on(&self.install){return Err(denied());}
        self.install.origin.require_live_source()
    }
    // Only the fixed ordinary Core adapters can borrow this same reader. It
    // still checks actual Rust account and share mode inside each cut.
    pub(in crate::system::reader::smb) fn ordinary_reader(&self,member:&crate::smb::RustSessionWitness)
        ->io::Result<&InitialReader>{
        self.require_member(member)?;
        Ok(&self.install.source)
    }
    pub(in crate::system::reader::smb) fn ordinary_folder_at(&self,db:&Connection,c:&Catalog,label:&str)->io::Result<Setting>{
        if !std::ptr::eq(db,&self.install.source.conn){return Err(denied());}
        self.current_ordinary_at(db,c,label)
    }
    /// Spelling alone is not access. The retained OPEN separately qualifies
    /// the live member, same SQLite cut and distinct original physical R.
    pub(crate) fn names_retained_role(&self,label:&str)->bool{
        label==self.install.installed.plan.recycle.as_str()
    }
    pub(in crate::system::reader::smb) fn retained_volume_at(&self,db:&Connection,c:&Catalog)->io::Result<crate::VolumeId>{
        let site=&self.install;
        if !std::ptr::eq(db,&site.source.conn)||site.installed.active_at(db,c)?!=site.original{return Err(denied());}
        Ok(c.rows().get(1).filter(|r|r.role==crate::Role::Recycle&&r.ordinal==1).ok_or_else(denied)?.volume)
    }
    fn current_ordinary_at(&self,db:&Connection,c:&Catalog,label:&str)->io::Result<Setting>{
        self.install.current_ordinary_at(db,c,label)
    }
    /// Type comes only from the original mutable FILE engine. It accepts
    /// that engine's exact writer-side connection only for this same S;
    /// an arbitrary SQL connection never replaces the reader's DB.
    pub(in crate::system::reader::smb) fn ordinary_writer_at(
        &self,cut:&crate::system::reader::smb::work_change::WriterCut<'_>,label:&str,
    )->io::Result<(crate::identity::AccountId,crate::OperationId,i64,Setting)>{
        let (source,db,c)=cut.parts();
        self.current_writable_at(source,db,c,label)
    }
    pub(in crate::system::reader::smb) fn ordinary_birth_at(
        &self,cut:&crate::system::reader::smb::file_create::CreateCut<'_>,label:&str,
    )->io::Result<(crate::identity::AccountId,crate::OperationId,i64,Setting)>{
        let (source,db,c)=cut.parts();
        self.current_writable_at(source,db,c,label)
    }
    pub(in crate::system::reader::smb) fn ordinary_directory_birth_at(
        &self,cut:&crate::system::reader::smb::ordinary_directory::DirectoryCreateCut<'_>,label:&str,
    )->io::Result<(crate::identity::AccountId,crate::OperationId,i64,Setting)>{
        let (source,db,c)=cut.parts();
        self.current_writable_at(source,db,c,label)
    }
    pub(in crate::system::reader::smb) fn ordinary_transfer_at(
        &self,cut:&crate::system::reader::smb::file_transfer::TransferCut<'_>,label:&str,
    )->io::Result<(crate::identity::AccountId,crate::OperationId,i64,Setting)>{
        let (source,db,c)=cut.parts();
        self.current_writable_at(source,db,c,label)
    }
    pub(in crate::system::reader::smb) fn ordinary_rename_at(
        &self,cut:&crate::system::reader::smb::file_rename::RenameCut<'_>,label:&str,
    )->io::Result<(crate::identity::AccountId,crate::OperationId,i64,Setting)>{
        let (source,db,c)=cut.parts();
        self.current_writable_at(source,db,c,label)
    }
    pub(in crate::system::reader::smb) fn ordinary_directory_relabel_at(
        &self,cut:&crate::system::reader::smb::ordinary_directory::DirectoryRelabelCut<'_>,label:&str,
    )->io::Result<(crate::identity::AccountId,crate::OperationId,i64,Setting)>{
        let (source,db,c)=cut.parts();
        self.current_writable_at(source,db,c,label)
    }
    fn current_writable_at(&self,source:&InitialReader,db:&Connection,c:&Catalog,label:&str)
        ->io::Result<(crate::identity::AccountId,crate::OperationId,i64,Setting)>{
        self.install.current_writable_at(source,db,c,label)
    }
    /// A progress check during an already authorized ordinary FILE read,
    /// never a replacement for the before/after S, placement and media cuts.
    pub(in crate::system::reader::smb) fn ordinary_read_progress(&self,member:&crate::smb::RustSessionWitness)->io::Result<()>{
        if !self.session.same_session(member){return Err(denied());}
        member.require_read_progress(&self.install)
    }
    /// Cheap same-session fence inside the original SAVE transaction. Do
    /// not perform Core qualification, blocking locks or SQLite here.
    pub(in crate::system::reader::smb) fn original_write_live(&self,member:&crate::smb::RustSessionWitness)->io::Result<()>{
        if !self.session.same_session(member){return Err(denied());}
        member.require_write_turn_inside(&self.install)
    }
    /// A fresh signed, paid member in the actual current tree/open is
    /// required by the backend. An idle saved witness is not that member.
    /// Recheck the real credential and select all rows in ONE original cut.
    pub(crate) fn project(&self,member:&crate::smb::RustSessionWitness)->io::Result<RustRootProjection>{
        self.require_member(member)?;
        let site=&self.install;
        let children=site.source.at(|db,c|{
            if site.installed.active_at(db,&c)?!=site.original{return Err(denied());}
            let result=site.installed.plan.logical_after(db,&c,b"")?;
            Ok(result.into_iter().map(|v|(v.name.to_owned(),v.number,v.mode)).collect())
        })?;
        self.require_member(member)?;
        Ok(RustRootProjection{root:Rc::downgrade(&self.lifetime),children})
    }
    /// Used by the original command/reply owner before/after write to check
    /// the rights and visible revision, not to commit a cursor or acknowledge.
    pub(crate) fn require_projection(&self,member:&crate::smb::RustSessionWitness,expected:&RustRootProjection)
        ->io::Result<()>{
        if !expected.root.ptr_eq(&Rc::downgrade(&self.lifetime)){return Err(denied());}
        let current=self.project(member)?;
        if current.children!=expected.children{return Err(denied());}
        Ok(())
    }
    pub(crate) fn ensure_current(&self,member:&crate::smb::RustSessionWitness)->io::Result<()>{
        self.require_member(member)?;
        self.install.require_current()?;
        self.require_member(member)
    }
    /// The original ZXS logical root of the only exposed logical volume.
    /// This is not a physical F or R, and its number is outside the >255
    /// namespace of actual allocated FILE numbers.
    pub(crate) fn logical_root_disk_id(&self,member:&crate::smb::RustSessionWitness)->io::Result<(u64,u64)>{
        if !(member.is_create()||member.is_query_info()){return Err(denied());}
        self.ensure_current(member)?;
        Ok((1,1)) // (logical root index, single logical volume's index)
    }
    pub(crate) fn pool(&self)->crate::PoolId{self.install.original.pool}
    pub(crate) fn stamp(&self)->RustRootStamp{RustRootStamp(Rc::downgrade(&self.lifetime))}
    pub(crate) fn matches_stamp(&self,s:&RustRootStamp)->bool{
        s.0.ptr_eq(&Rc::downgrade(&self.lifetime))
    }
}

/// Sealed exact originating S lease/marker and selected startup. Carries
/// only original filesystem proof, never SQLite, a session or a client socket.
#[must_use]
pub(in crate::system::reader::smb) struct OrdinaryMoveSite{
    origin:std::sync::Weak<crate::smb::InheritedRun>,
    dir:crate::system::lease::SystemDir,
    marker:crate::fs::ControlRead,
    pool:crate::PoolId,
    schema:[u8;32],
}
impl RustRoot{
    pub(in crate::system::reader::smb) fn ordinary_move_site(&self,member:&crate::smb::RustSessionWitness)
        ->io::Result<OrdinaryMoveSite>{
        if !member.is_set_info(){return Err(denied());}
        let original=self.ordinary_reader(member)?;
        Ok(OrdinaryMoveSite{origin:Arc::downgrade(&self.install.origin),
            dir:original.dir.duplicate_for_move()?,marker:original.marker.duplicate()?,
            pool:original.pool,schema:original.expected_schema})
    }
}
impl OrdinaryMoveSite{
    pub(in crate::system::reader::smb) fn attach(&self)->io::Result<QualifiedRustInstall>{
        let origin=self.origin.upgrade().ok_or_else(denied)?;
        QualifiedRustInstall::attach_selected(origin,|path,pool|{
            if path!=self.dir.root.path.as_path()||pool!=self.pool{return Err(denied());}
            // `alongside` re-proves the same S directory, mount and both
            // original control/DB leaves before opening an independent VFS.
            InitialReader::alongside(&self.dir,&self.marker,self.pool,self.schema)
        })
    }
}

#[cfg(test)]
#[path="../../tests/smb/core_credential.rs"]
mod rust_credential_tests;
