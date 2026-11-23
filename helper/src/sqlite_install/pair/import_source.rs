//! Offline, non-authorizing H03 source inventory for exact independently maintained v20.
//! A complete result describes two bounded metadata walks, not a copy plan.
use super::{Id,observed,retire_ordinary};
use crate::{maintenance::DiscoveryView,pinned::{self,Entity,Mount,Pinned},sqlite_install::{DATABASE,bad,
    catalog::{self,Canonical,Initial},engine::{self,Access},markers::SystemMark,
    originals::{LockedSystem,guarded_named_backing},reservation::Existing}};
use rustix::{fs::{Access as Perm,AtFlags,FlockOperation,Mode,OFlags,RawDir,ResolveFlags,StatVfsMountFlags,
    accessat,fgetxattr,flock,fstatvfs,openat2},io::Errno};
use sha2::{Digest,Sha256};
use std::{collections::{BTreeMap,BTreeSet},ffi::{CStr,CString},fs::File,io,mem::MaybeUninit,
    os::unix::fs::MetadataExt,path::Path,sync::Arc};

pub(super) const MAX_ENTRIES:u16=2048;
const MAX_DEPTH:u8=32;
pub(super) const MANAGED:u16=1<<0;
pub(super) const CONTROL:u16=1<<1;
pub(super) const BOUNDARY:u16=1<<2;
pub(super) const NON_PLAIN:u16=1<<3;
pub(super) const HARD_LINK:u16=1<<4;
pub(super) const NAME:u16=1<<5;
pub(super) const PERMISSION:u16=1<<6;
pub(super) const EMPTY:u16=1<<7;
pub(super) const KNOWN:u16=(1<<8)-1;
const SAME_TREE:ResolveFlags=ResolveFlags::BENEATH.union(ResolveFlags::NO_XDEV).union(ResolveFlags::NO_SYMLINKS);
fn unknown()->io::Error{bad("no complete stable metadata observation of this exact original import source")}

#[derive(Clone,Copy,Debug,Default,Eq,PartialEq)]
pub(super) struct Counts{
    pub(super) files:u16,pub(super) dirs:u16,pub(super) empty_dirs:u16,pub(super) logical_bytes:u128,
}
impl Counts{
    pub(super) fn valid(self)->bool{
        self.files.checked_add(self.dirs).is_some_and(|n|(1..=MAX_ENTRIES).contains(&n))
            &&self.empty_dirs<=self.dirs&&self.logical_bytes<=u128::from(self.files)*(i64::MAX as u128)
    }
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) enum Finding { Blocked(u16), Inventoried(Counts) }

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Stamp{entity:Entity,mount:Mount,mode:u32,uid:u32,gid:u32,links:u64,len:u64,mtime:i64,mtime_ns:i64,ctime:i64,ctime_ns:i64}
impl Stamp{
    fn get(fd:&File)->io::Result<Self>{
        let m=fd.metadata()?;
        let mount=Mount::on(fd)?.ok_or_else(unknown)?;
        if m.nlink()==0{return Err(unknown());}
        Ok(Self{entity:Entity::of(&m)?,mount,mode:m.mode(),uid:m.uid(),gid:m.gid(),links:m.nlink(),len:m.len(),
            mtime:m.mtime(),mtime_ns:m.mtime_nsec(),ctime:m.ctime(),ctime_ns:m.ctime_nsec()})
    }
    fn dir(self)->bool{self.mode&0o170_000==0o040_000}
    fn plain(self)->bool{self.mode&0o170_000==0o100_000}
    fn add(self,h:&mut Sha256){
        for n in [self.entity.dev,self.entity.ino,self.mount.id,self.links,self.len]{h.update(n.to_be_bytes());}
        for n in [self.mode,self.uid,self.gid]{h.update(n.to_be_bytes());}
        for n in [self.mtime,self.mtime_ns,self.ctime,self.ctime_ns]{h.update(n.to_be_bytes());}
        h.update([match self.mount.at_root{None=>0,Some(false)=>1,Some(true)=>2}]);
    }
}

fn named_place(pin:&Pinned)->io::Result<[u64;3]>{
    let e=pin.entity();Ok([e.dev,e.ino,pin.require_same_named_mount()?.id])
}
fn spelling(a:&Path,b:&Path)->bool{a.starts_with(b)||b.starts_with(a)}
fn overlap(a:&Pinned,b:&Pinned)->bool{
    spelling(a.path(),b.path())||a.entity()==b.entity()
        ||a.entity_is_visible_ancestor_of(b)||b.entity_is_visible_ancestor_of(a)
}

// No R/F media descriptor is opened or read. These O_PATH names are
// exclusions only, never a source/target selection or a grant.
struct Exclusions(Vec<(Pinned,Mount)>);
impl Exclusions{
    fn pin(claims:&[Initial],scope:&Arc<DiscoveryView>)->io::Result<Self>{
        let mut roots=Vec::with_capacity(claims.len());
        for claim in claims{
            let pin=Pinned::local_storage(&claim.path,scope)?;
            let mount=pin.require_same_named_mount()?;
            scope.full_local_mount(mount.id)?; // incomplete backing cannot clear an alias
            roots.push((pin,mount));
        }
        Ok(Self(roots))
    }
    fn require(&self)->io::Result<()>{
        for (pin,mount) in &self.0{
            if pin.require_same_named_mount()?!=*mount{return Err(unknown());}
        }
        Ok(())
    }
    fn excludes(&self,source:&Pinned)->bool{self.0.iter().any(|(pin,_)|overlap(source,pin))}
}

struct HeldSource<'a>{pin:&'a Pinned,dir:File,stamp:Stamp,backing:u64}
impl<'a> HeldSource<'a>{
    fn open(pin:&'a Pinned,host:&File,uid:u32)->io::Result<Self>{
        let dir=pin.storage_directory()?; // exact source; real RDONLY|NOATIME
        flock(&dir,FlockOperation::NonBlockingLockExclusive)?;
        let stamp=Stamp::get(&dir)?;
        if !stamp.dir(){return Err(unknown());}
        let backing=guarded_named_backing(pin,&dir,host,uid)?;
        let source=Self{pin,dir,stamp,backing};source.require(host,uid)?;Ok(source)
    }
    fn require(&self,host:&File,uid:u32)->io::Result<()>{
        if Stamp::get(&self.dir)?!=self.stamp
            ||guarded_named_backing(self.pin,&self.dir,host,uid)?!=self.backing{
            return Err(unknown());
        }
        Ok(())
    }
    fn snapshot(&self,host:&File,uid:u32)->io::Result<Snapshot>{
        self.require(host,uid)?;
        // Independent directory streams; never reuse the lock OFD offset.
        // "." is relative to the already-qualified held directory.
        let fresh=scan_dir(&self.dir)?;
        let cut=scan(&fresh,self.stamp)?;
        self.require(host,uid)?;Ok(cut)
    }
}

fn scan_dir(pin:&File)->io::Result<File>{
    pinned::require_no_automount(pin)?;
    Ok(File::from(openat2(pin,c".",OFlags::RDONLY|OFlags::DIRECTORY|OFlags::NOFOLLOW|OFlags::NONBLOCK
        |OFlags::NOATIME|OFlags::CLOEXEC,Mode::empty(),SAME_TREE)?))
}
fn at(dir:&File,name:&CStr)->rustix::io::Result<File>{
    // Single relative component. Tail O_PATH|NOFOLLOW does not open a file,
    // device, FIFO, or symlink target; the lookup cannot cross a mount/bind.
    openat2(dir,name,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty(),SAME_TREE).map(File::from)
}
fn user_name(raw:&[u8],directory:bool)->bool{
    // Necessary predicate for the maintained ordinary display name and
    // FILE's 33-byte identity suffix. No claim about a future target budget.
    let max=if directory{255}else{222};
    if raw.is_empty()||raw.len()>max{return false;}
    std::str::from_utf8(raw).is_ok_and(|s|!s.ends_with('.')&&!s.ends_with(' ')&&s.encode_utf16().count()<=255
        &&!s.chars().any(|c|c.is_control()||matches!(c,'<'|'>'|':'|'"'|'/'|'\\'|'|'|'?'|'*')))
}
fn control_name(raw:&[u8])->bool{
    raw.starts_with(b".zxs-new-")||raw.starts_with(b".zxs-moveout-")
        ||raw==DATABASE.as_bytes()||matches!(raw,b"SYSTEM.sqlite3-journal"|b"SYSTEM.sqlite3-wal"|b"SYSTEM.sqlite3-shm"
            |b"REGISTRATION"|b"CONFIRMED"|b"FILE")
}
fn old_volume(dir:&File)->io::Result<bool>{
    let mut none=[0u8;0];
    match fgetxattr(dir,c"user.zxs.volume",&mut none[..]){
        Err(Errno::NODATA)=>Ok(false),
        Ok(_)|Err(Errno::RANGE)=>Ok(true),
        Err(e)=>Err(e.into()),
    }
}
#[derive(Debug,Eq,PartialEq)]
struct Snapshot{stamp:[u8;32],entries:u16,counts:Counts,blockers:u16}
impl Snapshot{
    fn finding(self)->io::Result<Finding>{
        if self.blockers!=0{return Ok(Finding::Blocked(self.blockers));}
        if self.entries==0{return Ok(Finding::Blocked(EMPTY));}
        if !self.counts.valid()||self.counts.files+self.counts.dirs!=self.entries{return Err(unknown());}
        Ok(Finding::Inventoried(self.counts))
    }
}
struct Walk{root:Stamp,hash:Sha256,entries:u16,counts:Counts,blockers:u16,dirs:BTreeSet<u64>}
fn scan(dir:&File,root:Stamp)->io::Result<Snapshot>{
    let mut walk=Walk{root,hash:Sha256::new_with_prefix(b"helper import inventory one live source 1\0"),entries:0,
        counts:Counts::default(),blockers:0,dirs:BTreeSet::new()};
    walk.visit(dir,root,0,0)?;
    Ok(Snapshot{stamp:walk.hash.finalize().into(),entries:walk.entries,counts:walk.counts,blockers:walk.blockers})
}
impl Walk{
    fn visit(&mut self,dir:&File,original:Stamp,depth:u8,prefix:usize)->io::Result<()>{
        if depth>MAX_DEPTH||!original.dir()||!self.dirs.insert(original.entity.ino)
            ||Stamp::get(dir)?!=original{return Err(unknown());}
        self.hash.update(b"D");original.add(&mut self.hash);
        if old_volume(dir)?{self.blockers|=CONTROL;self.hash.update(b"V");return Ok(());}
        let mut buf=[MaybeUninit::uninit();4096];
        let mut entries=RawDir::new(dir,&mut buf);
        let mut names=BTreeMap::new();let mut dots=[false;2];
        while let Some(entry)=entries.next(){
            let entry=entry?;
            let raw=entry.file_name().to_bytes();
            if let Some(dot)=[b".".as_slice(),b"..".as_slice()].iter().position(|&n|n==raw){
                if std::mem::replace(&mut dots[dot],true){return Err(unknown());}
                continue;
            }
            if self.entries>=MAX_ENTRIES||raw.is_empty()||raw.len()>255||raw.contains(&b'/')||entry.ino()==0
                ||names.insert(raw.to_vec(),entry.ino()).is_some(){return Err(unknown());}
            self.entries+=1;
        }
        if Stamp::get(dir)?!=original{return Err(unknown());}
        if names.is_empty()&&depth!=0{self.counts.empty_dirs+=1;}
        if depth==MAX_DEPTH&&!names.is_empty(){return Err(unknown());}
        for (raw,ino) in names{
            let length=prefix+usize::from(prefix!=0)+raw.len();
            if length>crate::path_rules::MAX_PATH_BYTES{return Err(unknown());}
            self.hash.update(b"N");self.hash.update((raw.len() as u16).to_be_bytes());self.hash.update(&raw);
            self.hash.update(ino.to_be_bytes());
            if control_name(&raw){self.blockers|=CONTROL;self.hash.update(b"C");continue;}
            let name=CString::new(raw).map_err(|_|unknown())?;
            let child=match at(dir,&name){
                Ok(pin)=>pin,
                Err(Errno::XDEV)=>{self.blockers|=BOUNDARY;self.hash.update(b"M");continue;}
                Err(e)=>return Err(e.into()),
            };
            pinned::require_no_automount(&child)?;
            let first=Stamp::get(&child)?;
            first.add(&mut self.hash);
            if first.mount.id!=self.root.mount.id||first.entity.dev!=self.root.entity.dev{
                self.blockers|=BOUNDARY;continue;
            }
            if first.entity.ino!=ino{return Err(unknown());}
            if first.dir()||first.plain(){
                if !user_name(name.to_bytes(),first.dir()){self.blockers|=NAME;}
                if first.dir(){
                    self.counts.dirs+=1;
                    let opened=scan_dir(&child)?;
                    self.visit(&opened,first,depth+1,length)?;
                }else{
                    self.counts.files+=1;
                    if first.links!=1{self.blockers|=HARD_LINK;}
                    if first.len>i64::MAX as u64{return Err(unknown());}
                    self.counts.logical_bytes+=u128::from(first.len);
                    // Permission evidence only. An ordinary file never gets
                    // a reader (not even when the read permission check passes).
                    match accessat(&child,c"",Perm::READ_OK,AtFlags::EMPTY_PATH|AtFlags::EACCESS){
                        Ok(())=>self.hash.update(b"A"),
                        Err(Errno::ACCESS|Errno::PERM)=>{self.blockers|=PERMISSION;self.hash.update(b"X");}
                        Err(e)=>return Err(e.into()),
                    }
                }
            }else{self.blockers|=NON_PLAIN;}
            let independently_named=at(dir,&name)?;
            if Stamp::get(&child)?!=first||Stamp::get(&independently_named)?!=first{return Err(unknown());}
        }
        self.hash.update(b"Z");
        if Stamp::get(dir)?!=original{return Err(unknown());}
        Ok(())
    }
}

fn vfs_without_access_time(locked:&LockedSystem)->io::Result<()>{
    // The stock Unix SQLite VFS does not ask O_NOATIME. Do not use it in
    // this command unless the *existing* original S prevents that metadata write.
    let f=fstatvfs(locked.source().dir())?.f_flag;
    if !f.intersects(StatVfsMountFlags::RDONLY|StatVfsMountFlags::NOATIME){return Err(unknown());}
    Ok(())
}

/// Only invoked by the attested lowered observer, with the actual original
/// S EX lent by its foreground parent. The callback gets bounded facts,
/// not source paths, descriptors, a resume ID or any authority.
pub(super) fn observe<T>(locked:&LockedSystem,pool:&Id,init:&Id,actor:&Id,path:&Path,location:[u64;3],
    report:impl FnOnce(Finding)->T)->io::Result<T>{
    retire_ordinary::compiled_schema()?;
    let (host,system)=locked.retirement_host()?;
    let scope=system.local_scope().ok_or_else(unknown)?;
    vfs_without_access_time(locked)?;
    let pin=Pinned::local_storage(path,scope)?;
    if named_place(&pin)?!=location{return Err(unknown());}
    let source=locked.source();
    let template=Canonical::new()?;
    let original=Existing::observed_system(locked)?;
    let marker=SystemMark::before_sql(source)?;
    vfs_without_access_time(locked)?;
    let mut anchor=engine::existing(&original,Access::Read)?;
    let read=|tx:&rusqlite::Transaction<'_>|{
        let original:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.operation
            WHERE op_id=?1 AND actor_id=?2 AND kind=1 AND state=1)",
            rusqlite::params![&init[..],&actor[..]],|r|r.get(0)).map_err(|_|unknown())?;
        if !original{return Err(unknown());}
        catalog::initial_metadata(tx,&template,source,pool,init)
    };
    let (before,claims)=observed::OriginalRead::new(locked,&original,&template,&marker).snapshot(&mut anchor,pool,init,read)?;
    // Obvious enrolled path intersection is conclusive without even opening
    // a source directory stream. Otherwise preserve passive alias exclusions.
    let obvious=claims.iter().any(|c|spelling(path,&c.path));
    let exclusions=if obvious{None}else{Some(Exclusions::pin(&claims,scope)?)};
    let blocked=obvious||exclusions.as_ref().is_some_and(|e|e.excludes(&pin));
    let held=if blocked{None}else{Some(HeldSource::open(&pin,host,source.uid())?)};
    let first=held.as_ref().map(|f|f.snapshot(host,source.uid())).transpose()?;
    if !anchor.is_autocommit(){return Err(unknown());}
    engine::original_name(&anchor)?;
    vfs_without_access_time(locked)?;
    let (after,repeat)=observed::at_original(pool,init,locked,&original,&template,&marker,read)?;
    let equal=claims.iter().map(|c|(&c.path,&c.marker)).eq(repeat.iter().map(|c|(&c.path,&c.marker)));
    if before!=after||!equal{return Err(unknown());}
    if held.as_ref().map(|f|f.snapshot(host,source.uid())).transpose()?!=first{return Err(unknown());}
    engine::original_name(&anchor)?;original.sidecars_absent()?;marker.verify(source,&before)?;
    anchor.close().map_err(|_|unknown())?;
    locked.retirement_host()?;vfs_without_access_time(locked)?;
    original.sidecars_absent()?;marker.verify(source,&before)?;
    scope.require_unchanged()?;
    if let Some(e)=&exclusions{e.require()?;}
    if named_place(&pin)?!=location{return Err(unknown());}
    if let Some(f)=&held {f.require(host,source.uid())?;}
    let value=report(match first{Some(f)=>f.finding()?,None if blocked=>Finding::Blocked(MANAGED),None=>return Err(unknown())});
    drop(held);Ok(value)
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn nonempty_v20_only_known_unsafe_names_and_exact_business_display_bounds_are_claimed(){
        assert!(user_name("目录".as_bytes(),true)&&user_name(".notes".as_bytes(),false));
        assert!(user_name(&[b'a';222],false)&&user_name(&[b'a';255],true));
        assert!(!user_name(&[b'a';223],false)&&!user_name(b"trail ",true)&&!user_name(b"file.",false));
        assert!(!user_name(b"x:y",false)&&!user_name(b"raw-\xff",true));
        for n in [b"REGISTRATION".as_slice(),b"FILE",b"SYSTEM.sqlite3",b".zxs-new-unfinished"]{
            assert!(control_name(n));
        }
        assert!(!control_name(b"budget-file.txt"));
        assert!(spelling(Path::new("/m/s"),Path::new("/m/s/child")));
        assert!(!spelling(Path::new("/m/source"),Path::new("/m/source-staging")));
    }

    #[test]
    #[ignore="source-only metadata inventory; needs a disposable existing supported local filesystem"]
    fn nonempty_v20_actual_scan_keeps_empty_dirs_and_abstains_on_links_control_names_and_a_changed_tree(){
        use std::{fs,os::unix::fs::symlink};
        let area=crate::test_area::data_directory();
        let path=area.path().join("legacy");
        fs::create_dir(&path).unwrap();fs::create_dir(path.join("empty")).unwrap();
        fs::write(path.join("body"),b"xyz").unwrap();fs::write(path.join("zero"),b"").unwrap();
        let scope=Arc::new(crate::maintenance::HostMounts::discovery_view(false).unwrap());
        let selected=Pinned::local_storage(&path,&scope).unwrap();
        let f=selected.storage_directory().unwrap();let initial=Stamp::get(&f).unwrap();
        let probe=|stamp|scan(&scan_dir(&f).unwrap(),stamp);
        let a=probe(initial).unwrap();assert_eq!(a,probe(initial).unwrap());
        assert_eq!(a.finding().unwrap(),Finding::Inventoried(Counts{files:2,dirs:1,empty_dirs:1,logical_bytes:3}));
        symlink("/outside-unopened",path.join("symbolic")).unwrap();
        fs::hard_link(path.join("body"),path.join("hard")).unwrap();fs::create_dir(path.join("REGISTRATION")).unwrap();
        assert!(probe(initial).is_err());
        let changed=probe(Stamp::get(&f).unwrap()).unwrap().finding().unwrap();
        assert_eq!(changed,Finding::Blocked(NON_PLAIN|HARD_LINK|CONTROL));
        let parent=area.path();
        fs::rename(&path,parent.join("held")).unwrap();fs::create_dir(&path).unwrap();
        assert!(selected.require_same_named_mount().is_err());
    }
}
