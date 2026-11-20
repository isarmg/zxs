//! Prepare two role roots without mounting anything in the real host namespace.
//! Only an already selected FD can be a source. No block device, shell, global
//! chroot, or mountpoint creation in the real host filesystem.
use linux_raw_sys::general as u;
use rustix::{fs::{Mode,OFlags,PROC_SUPER_MAGIC,fstatfs,mkdirat,open,openat},io::Errno};
use std::{collections::BTreeSet,ffi::OsStr,fs::File,io::{self,Write},os::{fd::{AsRawFd,FromRawFd},unix::{ffi::OsStrExt,fs::{FileTypeExt,MetadataExt}}},
    path::{Path,PathBuf},ptr,thread};

const DIR:OFlags=OFlags::PATH.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
const CREATE:OFlags=OFlags::WRONLY.union(OFlags::CREATE).union(OFlags::EXCL).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
const SAFE:u32=u::MOUNT_ATTR_NOSUID|u::MOUNT_ATTR_NODEV;
fn fail()->io::Error{io::Error::other("the dedicated private SMB runtime views could not be assembled")}

/// Only this kernel null object may be exposed as a device. Its pathname is
/// never taken from the environment, manifest or command arguments.
pub fn null_device()->io::Result<File>{
    let dev=File::from(open(c"/dev",DIR,Mode::empty())?);
    let pin=File::from(openat(&dev,c"null",OFlags::PATH|OFlags::CLOEXEC|OFlags::NOFOLLOW,Mode::empty())?);
    let m=pin.metadata()?;
    if !m.file_type().is_char_device()||m.rdev()!=rustix::fs::makedev(1,3)||m.uid()!=0||m.gid()!=0||
        m.mode()&0o7777!=0o666||!matches!(fstatfs(&pin)?.f_type as u64,0x01021994){return Err(fail());}
    // Use the genuine, held current proc-fd identity, not a second /dev name.
    let p=File::from(open(c"/proc",DIR,Mode::empty())?);
    if fstatfs(&p)?.f_type!=PROC_SUPER_MAGIC{return Err(fail());}
    let name=format!("thread-self/fd/{}",pin.as_raw_fd());
    let file=File::from(openat(&p,name.as_str(),OFlags::RDWR|OFlags::NOCTTY|OFlags::CLOEXEC,Mode::empty())?);
    let f=file.metadata()?;
    if !f.file_type().is_char_device()||(f.dev(),f.ino(),f.rdev())!=(m.dev(),m.ino(),m.rdev()){return Err(fail());}
    Ok(file)
}

#[derive(Clone,Copy)]
pub enum Access { Code, Public, Data, Null }
/// `path` is relative to the new role root, never relative to the caller's cwd.
pub struct Source<'a>{pub path:&'a Path,pub file:&'a File,pub access:Access}
/// Four fixed public NSS files only. No passwd data from the host.
pub struct Public<'a>{pub path:&'a Path,pub bytes:&'a [u8]}
struct Detached{path:PathBuf,tree:File}

#[allow(unsafe_code)]
fn returned_fd(rc:libc::c_long)->io::Result<File>{
    if rc<0{Err(io::Error::last_os_error())}else{
        // These calls either fail or return a fresh close-on-exec descriptor.
        Ok(unsafe{File::from_raw_fd(rc as i32)})
    }
}
fn status(rc:libc::c_long)->io::Result<()>{if rc==0{Ok(())}else{Err(io::Error::last_os_error())}}
#[allow(unsafe_code)]
fn detach(f:&File,recursive:bool)->io::Result<File>{
    // A source from another mount namespace cannot be rebound by this call
    // on Linux 6.12. Leaves are therefore cloned in the original supervisor;
    // only the two newly assembled roots are cloned by the private thread.
    let flags=u::OPEN_TREE_CLONE|u::OPEN_TREE_CLOEXEC|u::AT_EMPTY_PATH|u::AT_NO_AUTOMOUNT|
        if recursive{u::AT_RECURSIVE}else{0};
    returned_fd(unsafe{libc::syscall(libc::SYS_open_tree,f.as_raw_fd(),c"".as_ptr(),flags)})
}
#[allow(unsafe_code)]
fn attributes(f:&File,set:u32,privatize:bool)->io::Result<()>{
    // Mount-local only, never recursive or on the real source. In particular
    // this cannot make the host data mount read-only, reinterpret UID/GID, or
    // clear a noexec/readonly restriction from the installed origin.
    let a=u::mount_attr{attr_set:u64::from(set),attr_clr:0,propagation:if privatize{libc::MS_PRIVATE}else{0},userns_fd:0};
    status(unsafe{libc::syscall(libc::SYS_mount_setattr,f.as_raw_fd(),c"".as_ptr(),u::AT_EMPTY_PATH,
        &raw const a,u::MOUNT_ATTR_SIZE_VER0 as usize)})
}
#[allow(unsafe_code)]
fn tmpfs()->io::Result<File>{
    unsafe{
        let fs=returned_fd(libc::syscall(libc::SYS_fsopen,c"tmpfs".as_ptr(),u::FSOPEN_CLOEXEC))?;
        let fd=fs.as_raw_fd();
        use u::fsconfig_command::*;
        for (key,value) in [(c"mode",c"0755"),(c"size",c"8m"),(c"nr_inodes",c"32768")]{
            status(libc::syscall(libc::SYS_fsconfig,fd,FSCONFIG_SET_STRING as u32,key.as_ptr(),value.as_ptr(),0))?;
        }
        status(libc::syscall(libc::SYS_fsconfig,fd,FSCONFIG_SET_FLAG as u32,c"noswap".as_ptr(),ptr::null::<libc::c_char>(),0))?;
        status(libc::syscall(libc::SYS_fsconfig,fd,FSCONFIG_CMD_CREATE as u32,
            ptr::null::<libc::c_char>(),ptr::null::<libc::c_char>(),0))?;
        returned_fd(libc::syscall(libc::SYS_fsmount,fd,u::FSMOUNT_CLOEXEC,SAFE|u::MOUNT_ATTR_NOEXEC))
    }
}
#[allow(unsafe_code)]
fn attach(mount:i32,target:i32)->io::Result<()>{
    status(unsafe{libc::syscall(libc::SYS_move_mount,mount,c"".as_ptr(),target,c"".as_ptr(),
        u::MOVE_MOUNT_F_EMPTY_PATH|u::MOVE_MOUNT_T_EMPTY_PATH)})
}
#[allow(unsafe_code)]
fn private_namespace()->io::Result<File>{
    // CLONE_NEWNS implies CLONE_FS: only this thread/child gets the copied
    // mount topology and fs_struct. First remove ALL propagation before
    // there is any attach. Use already required genuine /proc as the existing
    // scratch mountpoint: never mkdir/chmod under the inherited host FS.
    status(unsafe{libc::syscall(libc::SYS_unshare,libc::CLONE_NEWNS)})?;
    status(unsafe{libc::mount(ptr::null(),c"/".as_ptr(),ptr::null(),libc::MS_PRIVATE|libc::MS_REC,ptr::null())} as libc::c_long)?;
    let proc=File::from(open(c"/proc",DIR,Mode::empty())?);
    if fstatfs(&proc)?.f_type!=PROC_SUPER_MAGIC{return Err(io::ErrorKind::PermissionDenied.into());}
    Ok(proc)
}

/// Only called post-clone by fixed launch code. The input role root is a
/// detached tree produced below, not another namespace's stale rooted path.
#[allow(dead_code,unsafe_code)]
pub(super) fn enter(view:i32)->io::Result<()>{
    let anchor=private_namespace()?;
    attach(view,anchor.as_raw_fd())?;
    // No more unshare: this mount really belongs to this child's topology.
    if unsafe{libc::fchdir(view)}!=0||unsafe{libc::chroot(c".".as_ptr())}!=0||unsafe{libc::chdir(c"/".as_ptr())}!=0{
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn component_path(p:&Path)->io::Result<Vec<&OsStr>>{
    let raw=p.as_os_str().as_bytes();
    if raw.is_empty()||raw.len()>4095{return Err(fail());}
    let mut out=Vec::new();
    for s in raw.split(|b|*b==b'/'){
        if s.is_empty()||s.len()>255||s.contains(&0)||[b".".as_slice(),b".."].contains(&s)||out.len()==256{
            return Err(fail());}
        out.push(OsStr::from_bytes(s));
    }Ok(out)
}
fn parents(root:&File,path:&[&OsStr])->io::Result<File>{
    let mut here=root.try_clone()?;
    for n in path{
        match mkdirat(&here,*n,Mode::from_bits_truncate(0o755)){
            Ok(())|Err(Errno::EXIST)=>(),Err(e)=>return Err(e.into()),
        }
        here=File::from(openat(&here,*n,DIR,Mode::empty())?);
    }Ok(here)
}
fn mountpoint(root:&File,path:&Path,is_dir:bool)->io::Result<File>{
    let names=component_path(path)?;
    let (leaf,above)=names.split_last().ok_or_else(fail)?;
    let p=parents(root,above)?;
    if is_dir{
        // These directory leaves are exclusive, not any pre-seeded ancestry.
        mkdirat(&p,*leaf,Mode::from_bits_truncate(0o755))?;
        Ok(File::from(openat(&p,*leaf,DIR,Mode::empty())?))
    }else{
        let place=File::from(openat(&p,*leaf,CREATE,Mode::from_bits_truncate(0o444))?);
        Ok(place)
    }
}
fn paths(sources:&[Source<'_>],copy:&[Public<'_>],dirs:&[&str])->io::Result<()>{
    let mut sorted=BTreeSet::new();
    for p in sources.iter().map(|s|s.path).chain(copy.iter().map(|s|s.path)).chain(dirs.iter().map(Path::new)){
        component_path(p)?;
        if !sorted.insert(p){return Err(fail());}
    }
    // Reject genuine prefix overlap. Don't reserve an entire conventional
    // parent just because the approved runtime also uses a different child.
    for &p in &sorted{
        if p.ancestors().skip(1).any(|a|sorted.contains(a)){return Err(fail());}
    }Ok(())
}
fn allowed_source(s:&Source<'_>)->io::Result<()>{
    let m=s.file.metadata()?;
    let ok=match s.access{
        Access::Code=>m.is_file(),
        Access::Public=>m.is_file()||m.is_dir(),
        Access::Data=>m.is_dir(),
        Access::Null=>s.path==Path::new("dev/null")&&m.file_type().is_char_device()&&m.rdev()==rustix::fs::makedev(1,3),
    };
    if !ok{return Err(fail());}Ok(())
}
fn leaves(sources:&[Source<'_>])->io::Result<Vec<Detached>>{
    if sources.len()>840{return Err(fail());}
    sources.iter().map(|s|{
        allowed_source(s)?;
        use Access::*;
        let mode=match s.access{Code=>SAFE|u::MOUNT_ATTR_RDONLY,
            Public=>SAFE|u::MOUNT_ATTR_RDONLY|u::MOUNT_ATTR_NOEXEC,
            Data=>SAFE|u::MOUNT_ATTR_NOEXEC,
            Null=>u::MOUNT_ATTR_NOSUID|u::MOUNT_ATTR_NOEXEC};
        let tree=detach(s.file,false)?;
        attributes(&tree,mode,true)?;
        Ok(Detached{path:s.path.to_owned(),tree})
    }).collect()
}
fn role(stage:&File,name:&OsStr,leaves:&[Detached],copy:&[Public<'_>],empty:&[&str])->io::Result<File>{
    let root=tmpfs()?;
    let spot=mountpoint(stage,Path::new(name),true)?;
    attach(root.as_raw_fd(),spot.as_raw_fd())?;
    for path in empty{mountpoint(&root,Path::new(path),true)?;}
    for p in copy{
        if !(1..=512).contains(&p.bytes.len()) {return Err(fail());}
        let names=component_path(p.path)?;
        let(leaf,above)=names.split_last().ok_or_else(fail)?;
        let dir=parents(&root,above)?;
        let mut f=File::from(openat(&dir,*leaf,CREATE,Mode::from_bits_truncate(0o644))?);
        f.write_all(p.bytes)?;
    }
    for entry in leaves{
        // Mountpoint creation is always on the empty tmpfs first; paths()
        // has already proved no leaf can be an ancestor of another.
        let spot=mountpoint(&root,&entry.path,entry.tree.metadata()?.is_dir())?;
        attach(entry.tree.as_raw_fd(),spot.as_raw_fd())?;
    }
    attributes(&root,u::MOUNT_ATTR_RDONLY,false)?; // deliberately not recursive
    detach(&root,true)
}

#[allow(unsafe_code)]
fn fresh_mode_mask(){
    // Used only after CLONE_NEWNS made the current thread's fs_struct private.
    unsafe{libc::umask(0o022)};
}

/// Designed specifically for the fixed Pair. The authorized caller remains
/// in the original ns and can still check each host name/mount. After join
/// only two detached trees survive, not a running builder or exposed mount.
pub fn prepare(daemon:&[Source<'_>],storage:&[Source<'_>],nss:&[Public<'_>])->io::Result<(File,File)>{
    let d_empty=["proc","instance/cache","instance/lock","instance/run"];
    let s_empty=["proc"];
    let exact_nss=["etc/nsswitch.conf","etc/passwd","etc/group","etc/zxs-smb-identities"];
    if daemon.iter().filter(|s|matches!(s.access,Access::Null)).count()!=1||
        storage.iter().any(|s|matches!(s.access,Access::Null)){return Err(fail());}
    if nss.len()!=exact_nss.len()||nss.iter().zip(exact_nss).any(|(p,n)|p.path!=Path::new(n)){return Err(fail());}
    paths(daemon,nss,&d_empty)?;paths(storage,&[],&s_empty)?;
    let daemon=leaves(daemon)?;let storage=leaves(storage)?;
    thread::scope(|scope|->io::Result<_>{
        thread::Builder::new().name("smb-private-views".to_owned()).spawn_scoped(scope, move||{
            let anchor=private_namespace()?;
            fresh_mode_mask();
            let scratch=tmpfs()?;
            attach(scratch.as_raw_fd(),anchor.as_raw_fd())?;
            Ok((role(&scratch,OsStr::new("smbd"),&daemon,nss,&d_empty)?,
                role(&scratch,OsStr::new("storage"),&storage,&[],&s_empty)?))
        })?.join().map_err(|_|fail())?
    })
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn actual_prefix_collisions_do_not_reserve_unrelated_layout_branches(){
        assert_eq!(component_path(Path::new(OsStr::from_bytes(b"usr/local/\xff"))).unwrap().len(),3);
        for bad in ["/usr/a","usr//a","usr/../a","usr/./a","usr/a/"]{
            assert!(component_path(Path::new(bad)).is_err());
        }
        assert!(paths(&[],&[],&["usr/local/volume","usr/lib/runtime.so","proc"]).is_ok());
        assert!(paths(&[],&[],&["usr","usr/lib/runtime.so"]).is_err());
        assert!(paths(&[],&[],&["proc/data-root","proc"]).is_err());
    }
}
