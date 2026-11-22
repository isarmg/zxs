//! Current kernel observations for fixed site names and the two explicitly
//! approved paths. Durable IDs never come from kernel mount numbering.
mod diagnostics;
use super::{approval::check_path,denied,layout};
use crate::fs::Entity;
use std::{collections::BTreeMap,ffi::{CStr,CString},fs::File,io::{self,Read},
    os::{fd::{AsRawFd,FromRawFd,RawFd},unix::{ffi::OsStrExt,fs::{FileExt,MetadataExt}}},
    path::{Component,Path,PathBuf}};

const BASE:u64=libc::RESOLVE_BENEATH|libc::RESOLVE_NO_MAGICLINKS|libc::RESOLVE_NO_SYMLINKS;
const PIN:i32=libc::O_PATH|libc::O_NOFOLLOW|libc::O_CLOEXEC;
const RD_DIR:i32=libc::O_RDONLY|libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC|libc::O_NOATIME;
const MAX_INDEX:usize=4*1024*1024;
const MAX_MOUNTS:usize=16_384;
const BTRFS:u32=0x9123_683e;
unsafe extern "C"{
    fn zxs_mount_id(fd:libc::c_int,mount:*mut u64)->libc::c_int;
}
fn fd(raw:RawFd)->io::Result<File>{
    if raw<0{return Err(io::Error::last_os_error());}
    // SAFETY: every caller transfers a newly opened descriptor or -1.
    Ok(unsafe{File::from_raw_fd(raw)})
}
fn open(parent:&File,name:&CStr,flags:i32,resolve:u64)->io::Result<File>{
    // SAFETY: open_how is a C ABI integer record; reserved fields stay zero.
    let mut how:libc::open_how=unsafe{std::mem::zeroed()};
    how.flags=flags as u64;how.resolve=resolve;
    // SAFETY: fixed live names, open_how and parent. No O_CREAT, no write access.
    let h=unsafe{libc::syscall(libc::SYS_openat2,parent.as_raw_fd(),name.as_ptr(),&how,std::mem::size_of_val(&how))};
    if h<0{return Err(io::Error::last_os_error());}
    fd(i32::try_from(h).map_err(|_|denied())?)
}
pub(super) fn leaf(parent:&File,name:&CStr)->io::Result<File>{open(parent,name,PIN,BASE|libc::RESOLVE_NO_XDEV)}
pub(super) fn fd_mount(raw:RawFd)->io::Result<u64>{
    let mut v=0;
    // SAFETY: this exact descriptor and one writable result live for the call.
    if unsafe{zxs_mount_id(raw,&mut v)}!=0{return Err(io::Error::last_os_error());}
    if v==0{return Err(denied());}Ok(v)
}
fn mount(file:&File)->io::Result<u64>{fd_mount(file.as_raw_fd())}
fn magic(file:&File)->io::Result<u32>{
    // SAFETY: Linux fstatfs initializes a correctly laid out output record.
    let mut s:libc::statfs=unsafe{std::mem::zeroed()};
    if unsafe{libc::fstatfs(file.as_raw_fd(),&mut s)}!=0{return Err(io::Error::last_os_error());}
    Ok(s.f_type as u32)
}

#[derive(Clone,Copy,Eq,PartialEq)]
pub(in crate::smb::launch) struct Place{
    pub(in crate::smb::launch) entity:Entity,
    pub(in crate::smb::launch) mount:u64,
}
pub(super) fn place(f:&File)->io::Result<Place>{
    let m=f.metadata()?;
    if m.ino()==0||m.nlink()==0{return Err(denied());}
    Ok(Place{entity:Entity{device:m.dev(),inode:m.ino()},mount:mount(f)?})
}
#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) struct Stamp{
    pub(super) place:Place,
    pub(super) bytes:u64,
    pub(super) ctime:(i64,i64),
    pub(super) mtime:(i64,i64),
    pub(super) mode:u32,
    pub(super) uid:u32,
    pub(super) gid:u32,
}
pub(super) fn regular(file:&File)->io::Result<Stamp>{
    let m=file.metadata()?;
    if !m.is_file()||m.nlink()!=1{return Err(denied());}
    Ok(Stamp{place:place(file)?,bytes:m.len(),ctime:(m.ctime(),m.ctime_nsec()),
        mtime:(m.mtime(),m.mtime_nsec()),mode:m.mode(),uid:m.uid(),gid:m.gid()})
}
fn access(file:&File,mask:i32,want:i32)->io::Result<()>{
    // SAFETY: fcntl GETFL and GETFD do not take third arguments.
    let (fl,of)=unsafe{(libc::fcntl(file.as_raw_fd(),libc::F_GETFL),libc::fcntl(file.as_raw_fd(),libc::F_GETFD))};
    if fl<0||of<0||fl&mask!=want||of&libc::FD_CLOEXEC==0{return Err(denied());}
    Ok(())
}
pub(super) fn rdonly(file:&File)->io::Result<()>{
    access(file,libc::O_ACCMODE|libc::O_PATH,libc::O_RDONLY)
}
pub(super) fn no_xattr(file:&File,name:&CStr)->io::Result<()>{
    // SAFETY: null output and size 0 asks only whether this fixed key exists.
    if unsafe{libc::fgetxattr(file.as_raw_fd(),name.as_ptr(),std::ptr::null_mut(),0)}<0
        && io::Error::last_os_error().raw_os_error()==Some(libc::ENODATA){return Ok(());}
    Err(denied())
}
pub(super) fn dir_reader(pin:&File)->io::Result<File>{
    let out=open(pin,c".",RD_DIR,BASE|libc::RESOLVE_NO_XDEV)?;
    if place(&out)?!=place(pin)?{return Err(denied());}
    rdonly(&out)?;Ok(out)
}
/// All callers hold the actual numeric own task issued by the host guard.
pub(super) fn proc_pin(parent:&File,name:&CStr)->io::Result<File>{
    if magic(parent)?!=libc::PROC_SUPER_MAGIC as u32{return Err(denied());}
    // A terminal bare O_PATH lookup does not request O_DIRECTORY and never
    // activates unqualified proc children, magic links or mount crossings.
    let f=open(parent,name,PIN,libc::RESOLVE_BENEATH|libc::RESOLVE_NO_XDEV|libc::RESOLVE_NO_MAGICLINKS)?;
    if mount(&f)?!=mount(parent)?||magic(&f)?!=libc::PROC_SUPER_MAGIC as u32{return Err(denied());}
    Ok(f)
}
pub(super) fn proc_dir(parent:&File,name:&CStr)->io::Result<File>{
    let pin=proc_pin(parent,name)?;
    if !pin.metadata()?.is_dir(){return Err(denied());}
    Ok(pin)
}
enum OriginalAccess { Read, AppendLog }
pub(super) fn read_original(task:&File,pin:&File,stamp:Stamp)->io::Result<File>{
    reopen_original(task,pin,stamp,OriginalAccess::Read)
}
/// Reopen the existing checked inode, not its arbitrary filesystem name.
/// Only the kernel's literal decimal link for our retained pin is followed.
fn reopen_original(task:&File,pin:&File,stamp:Stamp,how:OriginalAccess)->io::Result<File>{
    if regular(pin)?!=stamp{return Err(denied());}
    let table=proc_dir(task,c"fd")?;
    let name=CString::new(pin.as_raw_fd().to_string()).map_err(|_|denied())?;
    let link=proc_pin(&table,&name)?;
    if !link.metadata()?.is_symlink(){return Err(denied());}
    let link_at=place(&link)?;
    let follow=|flags:i32|->io::Result<File>{
        // SAFETY: exact decimal current descriptor within genuine procfs
        // for our original task. Fixed flags; no create or truncate.
        fd(unsafe{libc::openat(table.as_raw_fd(),name.as_ptr(),flags|libc::O_CLOEXEC)})
    };
    let previously=follow(libc::O_PATH)?;
    if regular(&previously)?!=stamp||regular(pin)?!=stamp{return Err(denied());}
    let mode=match how{OriginalAccess::Read=>libc::O_RDONLY,OriginalAccess::AppendLog=>libc::O_WRONLY|libc::O_APPEND};
    let out=follow(mode|libc::O_NONBLOCK|libc::O_NOCTTY|libc::O_NOATIME)?;
    access(&out,libc::O_ACCMODE|libc::O_PATH|libc::O_APPEND,mode)?;
    let now=proc_pin(&table,&name)?;
    if regular(&out)?!=stamp||regular(pin)?!=stamp||place(&now)?!=link_at||!now.metadata()?.is_symlink(){
        return Err(denied());
    }
    Ok(out)
}
pub(super) fn dir_names(dir:&File,limit:usize,cap:usize)->io::Result<Vec<Vec<u8>>>{
    let reader=dir_reader(dir)?; // fresh NOATIME OFD: no caller cursor or atime mutation
    let mut chunk=[0u8;8192];
    let mut used=0usize;
    let mut out=Vec::new();
    loop{
        // SAFETY: live directory and correctly bounded Linux getdents64
        // output; parse byte offsets instead of relying on struct padding.
        let got=unsafe{libc::syscall(libc::SYS_getdents64,reader.as_raw_fd(),chunk.as_mut_ptr(),chunk.len())};
        if got<0{return Err(io::Error::last_os_error());}
        if got==0{break;}
        let mut records=chunk.get(..got as usize).ok_or_else(denied)?;
        while !records.is_empty(){
            let width=usize::from(u16::from_ne_bytes(records.get(16..18).ok_or_else(denied)?.try_into().map_err(|_|denied())?));
            let (record,rest)=records.split_at_checked(width).ok_or_else(denied)?;
            let name=record.get(19..).and_then(|n|n.split(|&b|b==0).next().filter(|head|head.len()<n.len()))
                .filter(|head|(1..=255).contains(&head.len())).ok_or_else(denied)?;
            if name!=b"."&&name!=b".."{
                used=used.checked_add(name.len()+1).filter(|sum|*sum<=cap).ok_or_else(denied)?;
                if out.len()>=limit{return Err(denied());}
                out.push(name.to_vec());
            }
            records=rest;
        }
    }
    if place(&reader)?!=place(dir)?{return Err(denied());}
    out.sort();if out.windows(2).any(|v|v[0]==v[1]){return Err(denied());}Ok(out)
}
pub(super) fn require_spelling(dir:&File,want:&[u8])->io::Result<()>{
    if !dir_names(dir,4096,256*1024)?.iter().any(|n|n.as_slice()==want){return Err(denied());}
    Ok(())
}
pub(super) fn exactly(dir:&File,want:&[&[u8]])->io::Result<()>{
    let names=dir_names(dir,want.len(),1024)?;
    if !names.iter().map(Vec::as_slice).eq(want.iter().copied()){return Err(denied());}
    Ok(())
}

#[derive(Clone,Copy)]
struct MountRow<'a>{parent:u64,device:u64,whole:bool,point:&'a[u8],magic:Option<u32>}
struct Mounts<'a>{rows:BTreeMap<u64,MountRow<'a>>}
fn number<T:std::str::FromStr>(b:&[u8])->Option<T>{
    if b.is_empty()||!b.iter().all(u8::is_ascii_digit){return None;}
    std::str::from_utf8(b).ok()?.parse().ok()
}
fn family(raw:&[u8])->Option<u32>{
    match raw{
        b"ext2"|b"ext3"|b"ext4"=>Some(0xef53),
        b"xfs"=>Some(0x5846_5342),
        b"btrfs"=>Some(BTRFS),
        b"f2fs"=>Some(0xf2f5_2010),
        _=>None,
    }
}
fn raw_mounts(task:&File)->io::Result<Vec<u8>>{
    let pin=proc_pin(task,c"mountinfo")?;
    let stamp=regular(&pin)?;
    // Activate this already qualified original proc inode by its private
    // descriptor, not the potentially changed name a second time.
    let f=read_original(task,&pin,stamp)?;
    let mut b=Vec::new();
    (&f).take((MAX_INDEX+1) as u64).read_to_end(&mut b)?;
    if regular(&f)?!=stamp||regular(&proc_pin(task,c"mountinfo")?)?!=stamp{return Err(denied());}
    Ok(b)
}
fn escaped(bytes:&[u8])->Vec<u8>{
    let mut out=Vec::with_capacity(bytes.len());
    for &b in bytes{
        match b{ b' '=>out.extend_from_slice(br"\040"), b'\n'=>out.extend_from_slice(br"\012"),
            b'\t'=>out.extend_from_slice(br"\011"), b'\\'=>out.extend_from_slice(br"\134"), _=>out.push(b) }
    }
    out
}
impl<'a> Mounts<'a>{
    fn parse(bytes:&'a[u8])->io::Result<Self>{
        if bytes.is_empty()||bytes.len()>MAX_INDEX||bytes.last()!=Some(&b'\n'){return Err(denied());}
        let mut rows=BTreeMap::new();
        for line in bytes[..bytes.len()-1].split(|&b|b==b'\n'){
            let mut p=line.split(|&b|b==b' ');
            let id=p.next().and_then(number::<u64>).filter(|n|*n!=0).ok_or_else(denied)?;
            let parent=p.next().and_then(number::<u64>).filter(|n|*n!=0).ok_or_else(denied)?;
            let dev=p.next().ok_or_else(denied)?;
            let colon=dev.iter().position(|&b|b==b':').ok_or_else(denied)?;
            let major=number::<u32>(&dev[..colon]).ok_or_else(denied)?;
            let minor=number::<u32>(&dev[colon+1..]).ok_or_else(denied)?;
            let root=p.next().filter(|a|a.starts_with(b"/")).ok_or_else(denied)?;
            let point=p.next().filter(|a|a.starts_with(b"/")).ok_or_else(denied)?;
            if p.next().is_none_or(|v|v.is_empty())||!p.by_ref().any(|a|a==b"-"){return Err(denied());}
            let fs=p.next().filter(|v|!v.is_empty()).ok_or_else(denied)?;
            if !(0..2).all(|_|p.next().is_some_and(|v|!v.is_empty()))||p.next().is_some(){return Err(denied());}
            let row=MountRow{parent,device:libc::makedev(major,minor) as u64,whole:root==b"/",point,magic:family(fs)};
            if rows.len()>=MAX_MOUNTS||rows.insert(id,row).is_some(){return Err(denied());}
        }
        Ok(Self{rows})
    }
    fn observe(&self,f:&File,expected:u64)->io::Result<Place>{
        let m=mount(f)?;
        let row=self.rows.get(&m).filter(|r|r.whole&&r.magic.is_some()).ok_or_else(denied)?;
        if m!=expected||row.magic!=Some(magic(f)?){return Err(denied());}
        let p=place(f)?;
        if p.mount!=m||row.magic!=Some(BTRFS)&&row.device!=p.entity.device{return Err(denied());}
        Ok(p)
    }
    fn crossing(&self,prefix:&[u8],here:u64,allowed:bool)->io::Result<Option<u64>>{
        let x=escaped(prefix);
        let mut names=self.rows.iter().filter(|(_,r)|r.point==x.as_slice());
        let Some((&id,row))=names.next()else{return Ok(None);};
        if names.next().is_some()||!allowed||row.parent!=here||id==here||!row.whole||row.magic.is_none(){
            return Err(denied());
        }
        Ok(Some(id))
    }
}

#[derive(Clone,Copy)]
pub(super) enum Role { FixedDir, System(u32,Entity), Media(u32,Entity), Image(Entity) }
impl Role{
    fn permits_crossing(self,prefix:&Path)->bool{
        // The fixed selector, instance and log directory all live on the
        // /zxs mount (which may itself be the host filesystem). A rule
        // based on depth would also allow a separate /zxs/state or /zxs/log.
        !matches!(self,Self::FixedDir)||prefix==Path::new(layout::ROOT)
    }
    fn storage_uid(self)->Option<u32>{
        match self{Self::System(uid,_)|Self::Media(uid,_)=>Some(uid),_=>None}
    }
    fn product(self)->Option<Entity>{
        match self{Self::System(_,p)|Self::Media(_,p)|Self::Image(p)=>Some(p),Self::FixedDir=>None}
    }
    fn accepts(self,path:&Path)->bool{
        match self{
            Self::System(_,_)|Self::Media(_,_)=>layout::media(path),
            Self::Image(_)=>layout::image(path),
            Self::FixedDir=>true,
        }
    }
}
pub(super) struct PathPin{path:PathBuf,role:Role,chain:Vec<Place>,file:File}
impl PathPin{
    pub(super) fn select(host:&File,task:&File,path:&Path,role:Role)->io::Result<Self>{
        if matches!(role,Role::Media(..)){
            crate::fs::check_path(path)?;
            if path.components().any(|c|c.as_os_str().as_bytes().len()>255){return Err(denied());}
        }else{check_path(path)?;}
        if !role.accepts(path){return Err(denied());}
        let index=raw_mounts(task)?;let tab=Mounts::parse(&index)?;
        let mut file=host.try_clone()?;let m=mount(&file)?;
        if tab.rows.get(&m).is_none_or(|r|r.point!=b"/"){return Err(denied());}
        let mut chain=vec![tab.observe(&file,m)?];
        let mut prefix=PathBuf::from("/");
        let mut disks_group=false;
        let mut components=path.components().filter_map(|n|match n{Component::Normal(c)=>Some(c),_=>None}).peekable();
        while let Some(n)=components.next(){
            let parent=file.metadata()?;
            let trusted=role.storage_uid()==Some(parent.uid())||parent.uid()==0;
            let sticky=role.storage_uid().is_some()&&parent.mode()&0o1000!=0;
            if !parent.is_dir()||!trusted||parent.mode()&0o022!=0&&!sticky{return Err(denied());}
            if role.storage_uid().is_none(){
                let d=dir_reader(&file)?;
                no_xattr(&d,c"system.posix_acl_access")?;
                if matches!(role,Role::FixedDir)&&prefix==Path::new(layout::ROOT){no_xattr(&d,c"system.posix_acl_default")?;}
            }
            if matches!(role,Role::FixedDir)&&prefix==Path::new(layout::ROOT)&&
                (parent.gid()!=0||parent.mode()!=0o040755){return Err(denied());}
            let here=*chain.last().ok_or_else(denied)?;
            let at_product=role.product()==Some(here.entity);
            if at_product{
                // Apply the same component reservation even when a whole
                // local mount presents this original directory by another name.
                let relative=path.strip_prefix(&prefix).map_err(|_|denied())?;
                if !role.accepts(&Path::new(layout::ROOT).join(relative)){return Err(denied());}
            }
            if matches!(role,Role::FixedDir)||at_product||disks_group&&components.peek().is_none(){
                require_spelling(&file,n.as_bytes())?;
            }
            disks_group=at_product&&n.as_bytes()==b"disks";
            prefix.push(n);
            let parent=here.mount;
            let crossing=tab.crossing(prefix.as_os_str().as_bytes(),parent,
                role.permits_crossing(&prefix))?;
            let c=CString::new(n.as_bytes()).map_err(|_|denied())?;
            let resolve=BASE|if crossing.is_some(){libc::RESOLVE_CACHED}else{libc::RESOLVE_NO_XDEV};
            // Open a bare O_PATH so an unexpected special entry is never
            // activated. Only the exact known whole local crossing is tried.
            file=open(&file,&c,PIN,resolve)?;
            let found=tab.observe(&file,crossing.unwrap_or(parent))?;
            if matches!(role,Role::FixedDir){
                // Below the product root, even a same-mount device change
                // is not the fixed original. At /zxs prove the held root
                // itself, not just a matching spelling in mountinfo.
                if let Some(id)=crossing{
                    let row=tab.rows.get(&id).ok_or_else(denied)?;
                    if row.device==0||!mount_root(&file)?||row.magic==Some(BTRFS)&&found.entity.inode!=256{
                        return Err(denied());
                    }
                }else if found.entity.device!=here.entity.device{return Err(denied());}
            }
            if chain.iter().any(|s|s.entity==found.entity){return Err(denied());}
            chain.push(found);
        }
        let meta=file.metadata()?;
        match role{
            Role::System(uid,_)|Role::Media(uid,_) if meta.is_dir()&&meta.uid()==uid&&meta.mode()==0o040700=>{},
            Role::FixedDir if meta.is_dir()&&meta.uid()==0=>{},
            Role::Image(_) if meta.is_file()&&meta.uid()==0=>{},
            _=>return Err(denied()),
        }
        Ok(Self{path:path.to_owned(),role,chain,file})
    }
    pub(super) fn require(&self,host:&File,task:&File)->io::Result<()>{
        let named=Self::select(host,task,&self.path,self.role)?;
        if named.chain!=self.chain||place(&self.file)?!=named.location(){return Err(denied());}
        Ok(())
    }
    pub(super) fn held(&self)->&File{&self.file}
    pub(super) fn path(&self)->&Path{&self.path}
    pub(super) fn parent_place(&self)->Option<Place>{
        self.chain.split_last()?.1.last().copied()
    }
    /// Requires both directions of the original visible ancestry, strict
    /// whole local mounts and no reserved ZXS claim along the ancestors.
    /// The result is a current superblock witness, not a physical-disk ID.
    pub(super) fn backing(&self,host:&File,task:&File)->io::Result<u64>{
        let owner=self.role.storage_uid().ok_or_else(denied)?;
        self.require(host,task)?;
        let table=raw_mounts(task)?;let mounts=Mounts::parse(&table)?;
        let mut current=self.file.try_clone()?;
        let mut below:Option<(u64,bool,u64)>=None;
        let mut backing=None;
        for (i,want) in self.chain.iter().rev().enumerate(){
            let actual=mounts.observe(&current,want.mount)?;
            let row=mounts.rows.get(&actual.mount).filter(|r|r.whole&&r.device!=0).ok_or_else(denied)?;
            let m=current.metadata()?;
            if actual!=*want||!m.is_dir()||!(m.uid()==0||m.uid()==owner)||
                m.mode()&0o022!=0&&m.mode()&0o1000==0{return Err(denied());}
            no_xattr(&dir_reader(&current)?,c"user.zxs.volume")?;
            if i==0{backing=Some(row.device);}
            else{
                match leaf(&current,crate::system::ROOT_MARK){
                    Err(e) if e.kind()==io::ErrorKind::NotFound=>{},
                    _=>return Err(denied()),
                }
                if let Some((mount,at_root,parent))=below{
                    if actual.mount!=mount&&(!at_root||parent!=actual.mount){return Err(denied());}
                }
            }
            let at_root=mount_root(&current)?;
            if i+1==self.chain.len(){
                if !at_root||actual!=place(host)?{return Err(denied());}
            }else{
                below=Some((actual.mount,at_root,row.parent));
                // SAFETY: exactly the real parent of a still-held directory
                // already qualified by the forward local mount walk.
                current=fd(unsafe{libc::openat(current.as_raw_fd(),c"..".as_ptr(),PIN|libc::O_DIRECTORY)})?;
            }
        }
        self.require(host,task)?;backing.ok_or_else(denied)
    }
    pub(super) fn location(&self)->Place{*self.chain.last().expect("host root plus original named object")}
    pub(super) fn overlaps(&self,other:&Self)->bool{
        self.path.starts_with(&other.path)||other.path.starts_with(&self.path)||
            self.chain.iter().any(|v|v.entity==other.location().entity)||
            other.chain.iter().any(|v|v.entity==self.location().entity)
    }
}

fn mount_root(f:&File)->io::Result<bool>{
    // SAFETY: initialized output, empty-path metadata on the checked local FD.
    let mut s:libc::statx=unsafe{std::mem::zeroed()};
    let bit=libc::STATX_ATTR_MOUNT_ROOT as u64;
    if unsafe{libc::statx(f.as_raw_fd(),c"".as_ptr(),
        libc::AT_EMPTY_PATH|libc::AT_NO_AUTOMOUNT|libc::AT_STATX_DONT_SYNC,libc::STATX_MNT_ID,&mut s)}!=0||
        s.stx_attributes_mask&bit==0{return Err(denied());}
    Ok(s.stx_attributes&bit!=0)
}

pub(super) struct PublicLeaf{pin:File,stamp:Stamp,name:&'static CStr,value:Vec<u8>}
impl PublicLeaf{
    pub(super) fn select(task:&File,parent:&File,name:&'static CStr,mode:u32,limit:usize)->io::Result<Self>{
        let pin=leaf(parent,name)?;let stamp=regular(&pin)?;
        if stamp.mode!=mode||stamp.uid!=0||stamp.gid!=0||stamp.place.mount!=mount(parent)?||
            !(1..=limit as u64).contains(&stamp.bytes){return Err(denied());}
        let f=read_original(task,&pin,stamp)?;no_xattr(&f,c"system.posix_acl_access")?;
        let mut value=vec![0;stamp.bytes as usize];
        f.read_exact_at(&mut value,0)?;
        if f.read_at(&mut [0],stamp.bytes)?!=0||regular(&f)?!=stamp{return Err(denied());}
        let this=Self{pin,stamp,name,value};this.require(task,parent)?;Ok(this)
    }
    pub(super) fn require(&self,task:&File,parent:&File)->io::Result<()>{
        let same=||->io::Result<bool>{Ok(regular(&self.pin)?==self.stamp&&
            regular(&leaf(parent,self.name)?)?==self.stamp&&mount(parent)?==self.stamp.place.mount)};
        if !same()?{return Err(denied());}
        let f=read_original(task,&self.pin,self.stamp)?;no_xattr(&f,c"system.posix_acl_access")?;
        let mut current=vec![0;self.value.len()];
        f.read_exact_at(&mut current,0)?;
        if current!=self.value||f.read_at(&mut [0],self.stamp.bytes)?!=0||regular(&f)?!=self.stamp||!same()?{return Err(denied());}
        Ok(())
    }
    pub(super) fn bytes(&self)->&[u8]{&self.value}
}

#[cfg(test)]
#[path = "../../../../tests/unit/attached/smb/launch/site/held/tests.rs"]
mod tests;
