//! Detached data-only view for an independently approved static ZXS image.
//! No host /etc, runtime program, loader/DSO, site or credential is projected.
use super::{Fault,Parent,PROC,Result,Role,kernel,object};
use crate::fs::MountFlags;
use std::{ffi::{CStr,CString},fs::File,mem::{size_of,zeroed},os::{fd::AsRawFd,unix::{ffi::OsStrExt,fs::MetadataExt}},
    path::Path,ptr,thread};

// Linux v6.12 include/uapi/linux/mount.h. These are deliberately fixed
// checked operations, not a generic mount interface or a host target path.
const OPEN_TREE_CLONE:u32=1;
const FSOPEN_CLOEXEC:u32=1;
const FSMOUNT_CLOEXEC:u32=1;
const SET_FLAG:u32=0;
const SET_STRING:u32=1;
const CREATE:u32=6;
const MOVE_FROM_EMPTY:u32=4;
const MOVE_TO_EMPTY:u32=0x40;
const RO:u64=1;
const NO_SUID:u64=2;
const NO_DEV:u64=4;
const NO_EXEC:u64=8;
const DATA_SAFE:u64=NO_SUID|NO_DEV|NO_EXEC;
const TMPFS_MAGIC:libc::c_long=0x0102_1994;
const PIN:i32=libc::O_PATH|libc::O_NOFOLLOW|libc::O_CLOEXEC;

#[repr(C)]
struct Attributes{set:u64,clear:u64,propagation:u64,userns_fd:u64}

#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) enum Kind{System,Ordinary,Recycle}
#[derive(Clone,Copy)]
pub(super) struct Source<'a>{pub(super) path:&'a Path,pub(super) original:&'a File,pub(super) kind:Kind}
impl Source<'_>{
    fn forced_read_only(self,role:Role)->bool{role==Role::Preflight||role==Role::Web&&self.kind==Kind::Ordinary}
}
struct Leaf{components:Vec<CString>,mount:File}

#[must_use]
pub(super) struct View<'a>{root:File,originals:Vec<Source<'a>>,role:Role}

impl<'a> View<'a>{
    pub(super) fn prepare(parent:&Parent,originals:Vec<Source<'a>>,role:Role)->Result<Self>{
        parent.require()?;
        // At most one S, 62 approved FILE roots and one independent R.
        if !(1..=64).contains(&originals.len())||originals[0].kind!=Kind::System||
            originals[1..].iter().any(|x|x.kind==Kind::System)||
            originals.iter().filter(|x|x.kind==Kind::Recycle).count()>1||
            role==Role::Preflight&&originals.len()!=1||
            role==Role::Web&&originals[1..].iter().any(|x|x.kind!=Kind::Ordinary){
            return Err(Fault::Handoff);
        }
        let mut leaves=Vec::new();leaves.try_reserve_exact(originals.len()).map_err(|_|Fault::Resources)?;
        for (i,src) in originals.iter().enumerate(){
            let names=components(src.path)?;
            if names.first().is_some_and(|n|n.as_bytes()==b"proc")||
                originals[..i].iter().any(|old|src.path.starts_with(old.path)||old.path.starts_with(src.path))
                ||!src.original.metadata().map_err(|_|Fault::Handoff)?.is_dir(){
                return Err(Fault::Handoff);
            }
            // Clone a nonrecursive leaf in the original host namespace.
            // Only the clone is marked NODEV, NOSUID, NOEXEC and private.
            let mount=detach(src.original,false)?;
            // The S-only preflight and the Web worker's ordinary F
            // proof mounts are never given body write authority.
            let setting=if src.forced_read_only(role){DATA_SAFE|RO}else{DATA_SAFE};
            attributes(&mount,setting,true)?;
            leaves.push(Leaf{components:names,mount});
        }
        let root=thread::Builder::new().name("zxs-rust-smb-view".into()).spawn(move||{
            let anchor=private_anchor()?;
            // SAFETY: the worker has a private fs_struct after unshare.
            unsafe{libc::umask(0o022)};
            let staging=tmpfs()?;attach(staging.as_raw_fd(),anchor.as_raw_fd())?;
            let scratch=mkdir(&staging,c"role",false)?;
            let view=tmpfs()?;attach(view.as_raw_fd(),scratch.as_raw_fd())?;
            mkdir(&view,c"proc",false)?;
            for leaf in leaves{
                let (name,ancestors)=leaf.components.split_last().ok_or(Fault::Handoff)?;
                let mut cursor=view.try_clone().map_err(|_|Fault::Handoff)?;
                for n in ancestors{cursor=mkdir(&cursor,n,true)?;}
                let spot=mkdir(&cursor,name,false)?;
                attach(leaf.mount.as_raw_fd(),spot.as_raw_fd())?;
            }
            attributes(&view,RO,false)?;
            detach(&view,true)
        }).map_err(|_|Fault::Resources)?.join().map_err(|_|Fault::Handoff)??;
        parent.after_builder()?;
        let this=Self{root,originals,role};
        this.require_prepared(parent)?;
        Ok(this)
    }
    pub(super) fn file(&self)->&File{&self.root}
    pub(super) fn role(&self)->Role{self.role}
    pub(super) fn require_prepared(&self,parent:&Parent)->Result<()>{
        parent.require()?;
        let root=object(&self.root)?;
        let proc=directory_at(&self.root,c"proc",true)?;
        if kernel::fs_magic(self.root.as_raw_fd())?!=TMPFS_MAGIC||object(&proc)?.mount!=root.mount
            ||!flags(&self.root)?.read_only_data(){return Err(Fault::Handoff);}
        self.require_sources()?;
        parent.require()
    }
    pub(super) fn require_sources(&self)->Result<()>{
        for input in &self.originals{
            let shown=projected(&self.root,input.path)?;
            let orig=object(input.original)?;
            let got=object(&shown)?;
            let a=input.original.metadata().map_err(|_|Fault::Handoff)?;
            let b=shown.metadata().map_err(|_|Fault::Handoff)?;
            let shown_flags=flags(&shown)?;
            let read_only=if input.forced_read_only(self.role){true}else{flags(input.original)?.read_only()};
            if (orig.device,orig.inode)!=(got.device,got.inode)||orig.mount==got.mount||
                (a.mode(),a.uid(),a.gid())!=(b.mode(),b.uid(),b.gid())||
                !shown_flags.has_data_restrictions()||
                shown_flags.read_only()!=read_only{return Err(Fault::Handoff);}
        }
        Ok(())
    }
}
fn components(path:&Path)->Result<Vec<CString>>{
    crate::fs::check_path(path).map_err(|_|Fault::Shape)?;
    let raw=path.as_os_str().as_bytes().strip_prefix(b"/").ok_or(Fault::Shape)?;
    raw.split(|&b|b==b'/').map(|part|{
        if !(1..=255).contains(&part.len()){return Err(Fault::Shape);}
        CString::new(part).map_err(|_|Fault::Shape)
    }).collect()
}
pub(super) fn flags(file:&File)->Result<MountFlags>{
    MountFlags::on_fd(file).map_err(|_|Fault::Handoff)
}
fn status(code:libc::c_long)->Result<()>{if code==0{Ok(())}else{Err(Fault::Handoff)}}
fn detach(file:&File,recursive:bool)->Result<File>{
    let flags=OPEN_TREE_CLONE|libc::O_CLOEXEC as u32|libc::AT_EMPTY_PATH as u32|libc::AT_NO_AUTOMOUNT as u32|
        if recursive{libc::AT_RECURSIVE as u32}else{0};
    // SAFETY: empty path selects a currently retained file; the syscall
    // returns a fresh owned detached tree, never a numeric caller's path.
    unsafe{
        let raw=libc::syscall(libc::SYS_open_tree,file.as_raw_fd(),c"".as_ptr(),flags);
        Ok(File::from(kernel::new_fd(raw)?))
    }
}
fn attributes(mount:&File,set:u64,private:bool)->Result<()>{
    let attr=Attributes{set,clear:0,propagation:if private{libc::MS_PRIVATE}else{0},userns_fd:0};
    // SAFETY: only an admitted new detached clone or private assembled root;
    // fixed ABI and flags add restrictions, and do not set an idmapping.
    status(unsafe{libc::syscall(libc::SYS_mount_setattr,mount.as_raw_fd(),c"".as_ptr(),libc::AT_EMPTY_PATH,
        &raw const attr,size_of::<Attributes>())})
}
fn tmpfs()->Result<File>{
    // SAFETY: fixed fsopen/fsconfig operations on a private mount context;
    // none attaches a mount to the host or chooses an external path.
    unsafe{
        let context=File::from(kernel::new_fd(libc::syscall(libc::SYS_fsopen,c"tmpfs".as_ptr(),FSOPEN_CLOEXEC))?);
        let fd=context.as_raw_fd();
        for (key,value) in [(c"mode",c"0755"),(c"size",c"8m"),(c"nr_inodes",c"32768")]{
            status(libc::syscall(libc::SYS_fsconfig,fd,SET_STRING,key.as_ptr(),value.as_ptr(),0u32))?;
        }
        status(libc::syscall(libc::SYS_fsconfig,fd,SET_FLAG,c"noswap".as_ptr(),ptr::null::<libc::c_char>(),0u32))?;
        status(libc::syscall(libc::SYS_fsconfig,fd,CREATE,ptr::null::<libc::c_char>(),ptr::null::<libc::c_char>(),0u32))?;
        Ok(File::from(kernel::new_fd(libc::syscall(libc::SYS_fsmount,fd,FSMOUNT_CLOEXEC,DATA_SAFE as u32))?))
    }
}
fn directory_at(parent:&File,name:&CStr,no_cross:bool)->Result<File>{
    // SAFETY: scoped retained root, literal/generated component, valid ABI.
    // No intermediate path symlinks, device reads or mount actions here.
    let mut how:libc::open_how=unsafe{zeroed()};
    how.flags=PIN as u64;how.resolve=libc::RESOLVE_BENEATH|libc::RESOLVE_NO_SYMLINKS;
    if no_cross{how.resolve|=libc::RESOLVE_NO_XDEV;}
    let selected=unsafe{File::from(kernel::new_fd(libc::syscall(libc::SYS_openat2,parent.as_raw_fd(),
        name.as_ptr(),&raw const how,size_of::<libc::open_how>()))?)};
    if !selected.metadata().map_err(|_|Fault::Handoff)?.is_dir(){return Err(Fault::Handoff);}
    Ok(selected)
}
fn projected(root:&File,absolute:&Path)->Result<File>{
    let relative=CString::new(absolute.as_os_str().as_bytes().strip_prefix(b"/").ok_or(Fault::Shape)?)
        .map_err(|_|Fault::Shape)?;
    directory_at(root,&relative,false)
}
fn mkdir(parent:&File,name:&CStr,ancestor:bool)->Result<File>{
    // SAFETY: parent and name refer only to the new private tmpfs, never
    // to an original source. The target leaf must never already exist.
    let rc=unsafe{libc::mkdirat(parent.as_raw_fd(),name.as_ptr(),0o755 as libc::mode_t)};
    if rc!=0&&!(ancestor&&std::io::Error::last_os_error().raw_os_error()==Some(libc::EEXIST)){
        return Err(Fault::Handoff);
    }
    directory_at(parent,name,true)
}
fn attach(tree:i32,spot:i32)->Result<()>{
    // SAFETY: both are retained fixed mount objects in the private caller;
    // neither syscall follows a user-supplied path.
    status(unsafe{libc::syscall(libc::SYS_move_mount,tree,c"".as_ptr(),spot,c"".as_ptr(),MOVE_FROM_EMPTY|MOVE_TO_EMPTY)})
}
fn private_anchor()->Result<File>{
    // SAFETY: the assembler is a dedicated worker with no other owners of
    // its state. No attach until all propagation on its new namespace is off.
    if unsafe{libc::unshare(libc::CLONE_NEWNS)}!=0||
        unsafe{libc::mount(ptr::null(),c"/".as_ptr(),ptr::null(),libc::MS_REC|libc::MS_PRIVATE,ptr::null())}!=0{
        return Err(Fault::Handoff);
    }
    // Use an existing genuine procfs mountpoint; never mkdir on a host path.
    let fd=unsafe{libc::open(c"/proc".as_ptr(),PIN)};
    let root=unsafe{File::from(kernel::new_fd(fd.into())?)};
    if kernel::fs_magic(root.as_raw_fd())?!=PROC||!root.metadata().map_err(|_|Fault::Handoff)?.is_dir(){return Err(Fault::Handoff);}
    Ok(root)
}

/// Fixed raw post-clone chroot; no allocation, Rust locks, environment
/// discovery or child work after an error. Call only in the fresh child.
pub(super) unsafe fn enter_child(view:i32)->bool{
    // SAFETY: the child has no shared VM/FS state; make its mount
    // namespace private before moving the one trusted detached root.
    unsafe{
        if libc::unshare(libc::CLONE_NEWNS)!=0||
            libc::mount(ptr::null(),c"/".as_ptr(),ptr::null(),libc::MS_REC|libc::MS_PRIVATE,ptr::null())!=0{return false;}
        let spot=libc::open(c"/proc".as_ptr(),PIN);
        if spot<0{return false;}
        let mut st:libc::statfs=zeroed();
        let moved=libc::fstatfs(spot,&raw mut st)==0&&st.f_type as libc::c_long==PROC&&
            libc::syscall(libc::SYS_move_mount,view,c"".as_ptr(),spot,c"".as_ptr(),MOVE_FROM_EMPTY|MOVE_TO_EMPTY)==0;
        libc::close(spot);
        moved&&libc::fchdir(view)==0&&libc::chroot(c".".as_ptr())==0&&libc::chdir(c"/".as_ptr())==0
    }
}
