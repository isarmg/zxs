//! Same recorded INIT role at an actual, independent runtime source. An
//! absent or uncertain root has no placeholder mount and no authority at all.
use super::bad;
use crate::{maintenance::HostMounts,pinned::{Entity,Kind,Mount,Pinned},
    sqlite_install::{catalog::Initial,markers::RecycleMark,originals::named_backing}};
use rustix::{fs::{AtFlags,StatVfsMountFlags,fstatvfs,statat},io::Errno};
use std::{fs::File,io,os::unix::fs::MetadataExt,path::Path};

fn no()->io::Error{bad("cannot prove the original independent recycle disk")}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Identity{entity:Entity,mount:Mount,backing:u64}
impl Identity{
    fn actual(pin:&Pinned,file:&File,table:&HostMounts)->io::Result<Self>{
        let entity=Entity::of(&file.metadata()?)?;
        let mount=Mount::on(file)?.ok_or_else(no)?;
        if entity!=pin.entity()||mount!=pin.require_same_named_mount()?{return Err(no());}
        Ok(Self{entity,mount,backing:table.full_mount(mount.id)?.1})
    }
    fn separate(self,other:Self)->bool{
        self.entity.dev!=other.entity.dev&&self.mount.id!=other.mount.id&&self.backing!=other.backing
    }
}
fn disjoint(a:&Pinned,b:&Pinned)->bool{
    a.entity()!=b.entity()&&!a.entity_is_visible_ancestor_of(b)&&!b.entity_is_visible_ancestor_of(a)
}
fn validate_source<'a>(pin:&Pinned,dir:&File,host:&File,uid:u32,
    other:impl IntoIterator<Item=(&'a Pinned,&'a File)>)->io::Result<Identity>{
    let m=dir.metadata()?;
    if m.mode()!=0o040700||m.uid()!=uid||m.nlink()==0||fstatvfs(dir)?.f_flag.contains(StatVfsMountFlags::RDONLY){
        return Err(no());
    }
    let table=HostMounts::current()?;let r=Identity::actual(pin,dir,&table)?;
    if named_backing(pin,dir,&table,host,uid)?!=r.backing{return Err(no());}
    for (p,d) in other {
        if !disjoint(pin,p)||!r.separate(Identity::actual(p,d,&table)?){return Err(no());}
    }
    Ok(r)
}

pub(super) struct Recycle{pin:Pinned,dir:File,mark:RecycleMark,wanted:[u8;101],identity:Identity}
impl Recycle{
    pub(super) fn select<'a>(claim:&Initial,host:&File,uid:u32,
        other:impl IntoIterator<Item=(&'a Pinned,&'a File)>)->io::Result<Self>{
        Self::selected(Pinned::open(&claim.path,Kind::StorageRoot)?,claim,host,uid,other)
    }
    fn selected<'a>(pin:Pinned,claim:&Initial,host:&File,uid:u32,
        other:impl IntoIterator<Item=(&'a Pinned,&'a File)>)->io::Result<Self>{
        if pin.path()!=claim.path.as_path()||claim.marker[20]!=1{return Err(no());}
        let dir=pin.storage_directory()?;
        let identity=validate_source(&pin,&dir,host,uid,other)?;
        // Only the bounded, protected 101-byte marker is read. The business
        // contents, old/foreign names, pathname aliases and devices are not.
        let mark=RecycleMark::select(&dir,uid,&claim.marker)?;
        if Identity::actual(&pin,&dir,&HostMounts::current()?)?!=identity{return Err(no());}
        Ok(Self{pin,dir,mark,wanted:claim.marker,identity})
    }
    pub(super) fn require<'a>(&self,host:&File,uid:u32,
        other:impl IntoIterator<Item=(&'a Pinned,&'a File)>)->io::Result<()>{
        if validate_source(&self.pin,&self.dir,host,uid,other)?!=self.identity{return Err(no());}
        self.mark.verify(&self.dir,&self.wanted)?;
        if self.pin.require_same_named_mount()?!=self.identity.mount{return Err(no());}
        Ok(())
    }
    pub(super) fn source(&self)->(&Path,&File){(self.pin.path(),&self.dir)}
    pub(super) fn not_site(&self,site:&Pinned)->bool{disjoint(&self.pin,site)}
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Observation{Verified,Missing,Unconfirmed}
impl Observation{
    fn text(self)->&'static str{match self{
        // This is launch-time qualification, not a health check or a
        // statement about an unrelated block device with a similar label.
        Self::Verified=>"回收盘原件已核；仅供非 root 存储进程。",
        Self::Missing=>"回收盘不在位：启动核验时可信父目录下缺原路径，本次未投射。",
        Self::Unconfirmed=>"回收盘身份未确认：本次未投射，不能据此判离线、空盘或健康。",
    }}
}
pub(super) enum AtStartup{Attached(Recycle),Missing,Unconfirmed}
impl AtStartup{
    pub(super) fn assess<'a>(claim:&Initial,host:&File,uid:u32,site:&Pinned,
        other:impl IntoIterator<Item=(&'a Pinned,&'a File)>)->Self{
        match Pinned::open(&claim.path,Kind::StorageRoot){
            Ok(pin)=>{
                if !disjoint(&pin,site){return Self::Unconfirmed;}
                match Recycle::selected(pin,claim,host,uid,other){
                    Ok(r)=>Self::Attached(r),Err(_)=>Self::Unconfirmed,
                }
            }
            Err(e) if no_entry(&e)&&trusted_missing(&claim.path,host,uid).unwrap_or(false)=>Self::Missing,
            Err(_)=>Self::Unconfirmed,
        }
    }
    pub(super) fn original(&self)->Option<&Recycle>{match self{Self::Attached(r)=>Some(r),_=>None}}
    pub(super) fn private_status(&self)->&'static str{
        let observed=match self{Self::Attached(_)=>Observation::Verified,
            Self::Missing=>Observation::Missing,Self::Unconfirmed=>Observation::Unconfirmed};
        observed.text()
    }
}
fn no_entry(e:&io::Error)->bool{e.raw_os_error()==Some(Errno::NOENT.raw_os_error())}

/// Positive *pathname absence*, not a physical disk inventory. Confirm the
/// first missing component relative to its existing, protected parent in the
/// real host namespace. A symlink, wrong type, substitute mount, read error
/// or changed parent never authorizes the normal absence classification.
fn trusted_missing(path:&Path,host:&File,uid:u32)->io::Result<bool>{
    Kind::StorageRoot.check_path(path.as_os_str().as_encoded_bytes()).map_err(bad)?;
    let mut component=path;
    loop{
        let parent=component.parent().ok_or_else(no)?;
        let name=component.file_name().ok_or_else(no)?;
        let top=parent==Path::new("/");
        let pin=match Pinned::open(parent,if top{Kind::Directory}else{Kind::StorageRoot}){
            Ok(p)=>p,Err(e) if !top&&no_entry(&e)=>{component=parent;continue;},Err(e)=>return Err(e),
        };
        let dir=if top{host.try_clone()?}else{pin.storage_directory()?};
        let absent=||match statat(&dir,name,AtFlags::NO_AUTOMOUNT|AtFlags::SYMLINK_NOFOLLOW){
            Err(Errno::NOENT)=>Ok(true),Ok(_)=>Ok(false),Err(e)=>Err(io::Error::from(e)),
        };
        let first=named_backing(&pin,&dir,&HostMounts::current()?,host,uid)?;
        if !absent()? {return Ok(false);}
        if named_backing(&pin,&dir,&HostMounts::current()?,host,uid)?!=first||!absent()?{
            return Ok(false);
        }
        return match Pinned::open(path,Kind::StorageRoot){
            Err(e) if no_entry(&e)=>Ok(true),Ok(_)=>Ok(false),Err(e)=>Err(e),
        };
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    fn key(dev:u64,mount:u64,backing:u64)->Identity{
        Identity{entity:Entity{dev,ino:4},mount:Mount{id:mount,at_root:Some(true)},backing}
    }
    #[test]fn all_three_backing_observations_must_be_separate(){
        let a=key(1,2,3);assert!(a.separate(key(4,5,6)));
        for reused in [key(1,5,6),key(4,2,6),key(4,5,3)]{assert!(!a.separate(reused));}
    }
    #[test]fn unavailable_statuses_cannot_supply_any_mount_source(){
        assert!(AtStartup::Missing.original().is_none());
        assert!(AtStartup::Unconfirmed.original().is_none());
        assert_ne!(AtStartup::Missing.private_status(),AtStartup::Unconfirmed.private_status());
        assert!(no_entry(&io::Error::from(Errno::NOENT)));
        for e in [Errno::ACCESS,Errno::NOTDIR,Errno::LOOP,Errno::IO]{
            assert!(!no_entry(&io::Error::from(e)));
        }
    }
    #[test]fn missing_path_is_not_a_proof_for_an_empty_directory_or_link(){
        let scope=tempfile::tempdir().unwrap();let host=File::open("/").unwrap();
        let uid=rustix::process::geteuid().as_raw();let absent=scope.path().join("absent/child");
        assert!(trusted_missing(&absent,&host,uid).unwrap());
        let existing=scope.path().join("empty");std::fs::create_dir(&existing).unwrap();
        assert!(!trusted_missing(&existing,&host,uid).unwrap());
        let link=scope.path().join("linked");std::os::unix::fs::symlink(&absent,&link).unwrap();
        assert!(!trusted_missing(&link,&host,uid).unwrap());
        let site=scope.path().join("instance");std::fs::create_dir(&site).unwrap();
        let site=Pinned::open(&site,Kind::StorageRoot).unwrap();
        let issued=crate::sqlite_install::catalog::claimed_marker(1,&[1;16],&[2;16],&[3;16],&[4;32]);
        assert!(matches!(AtStartup::assess(&Initial{path:absent,marker:issued},&host,uid,&site,[]),AtStartup::Missing));
        for path in [existing,link]{
            assert!(matches!(AtStartup::assess(&Initial{path,marker:issued},&host,uid,&site,[]),AtStartup::Unconfirmed));
        }
    }
}
