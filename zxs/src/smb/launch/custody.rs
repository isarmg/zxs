//! Original host proof for the ZXS parent. The paired private startup may
//! act only after the actual Site, lowered preflight and terminal approval;
//! the kernel proof on its own never chooses a site or authorizes listeners.
mod view;
mod birth;
mod foreground;
mod listeners;
mod preflight;
mod start;
pub(super) use preflight::VerifiedPreflight;
pub(super) use start::{PreparedPair,authorize,prepare_site,stopping,launch_internal_smb,launch_internal_web};
use super::{Fault,Result,Role,descriptor::{self,UniqueMountId},
    held_open::{self,ProcOpen},kernel::{self,BootSocket,OriginalProcess}};
use std::{
    ffi::{CStr,CString,c_int,c_long},fs::{File,OpenOptions},io::Read,marker::PhantomData,
    os::fd::AsRawFd,os::unix::fs::{MetadataExt,OpenOptionsExt},rc::Rc,
};

const PROC:libc::c_long=libc::PROC_SUPER_MAGIC as libc::c_long;
const NSFS:libc::c_long=0x6e736673;
// Linux 6.12 fs/proc and include/uapi/linux/pidfd.h. A numeric proc
// entry is not a process grant; only the initial namespace's reserved PID 1
// may be looked up, to compare actual host namespaces and root.
const PID_INIT:u64=0xefff_fffc;
const USER_INIT:u64=0xefff_fffd;
const PIN:i32=libc::O_PATH|libc::O_CLOEXEC|libc::O_NOFOLLOW;
const DIR:i32=PIN|libc::O_DIRECTORY;
const SERVICE_CAPS:u64=(1<<0)|(1<<1)|(1<<3)|(1<<6)|(1<<7)|(1<<10);
const SUPERVISOR_CAPS:u64=SERVICE_CAPS|(1<<5)|(1<<8)|(1<<18)|(1<<19)|(1<<21);

#[repr(C)]
#[derive(Default)]
struct OwnProcessIds{pid:i32,uid:u32,euid:u32,gid:u32,egid:u32}

unsafe extern "C" {
    fn observe_own_task_ids(tid:*mut c_long)->i32;
    fn observe_own_process_ids(ids:*mut OwnProcessIds);
    fn observe_waitable_children()->c_int;
    fn observe_namespace_kind(fd:c_int,kind:u8)->c_int;
    fn open_process_namespace(pidfd:c_int,kind:u8)->c_int;
    fn open_namespace_owner(nsfd:c_int)->c_int;
}

#[must_use]
pub(super) struct Parent{
    original:OriginalProcess,
    proc:File,
    root:File,
    image:File,
    stamp:ImageStamp,
    _main_thread:PhantomData<Rc<()>>,
}
#[must_use]
pub(super) struct PreparedHandoff<'a>{
    parent:&'a Parent,
    host:BootSocket,
    child:BootSocket,
}
impl Parent{
    pub(super) fn pin()->Result<Self>{
        for fd in 0..=2{descriptor::flags(fd)?;}
        let proc=proc_root()?;
        no_other_descriptors(&proc)?;
        let original=OriginalProcess::current()?;
        let root=host_scope(&proc,&original)?;
        let image=running_image(&proc)?;
        let stamp=ImageStamp::read(&image)?;
        let this=Self{original,proc,root,image,stamp,_main_thread:PhantomData};
        this.require()?;
        Ok(this)
    }
    pub(super) fn require(&self)->Result<()>{
        self.original.require_live()?;
        if object(&self.proc)?!=object(&proc_root()?)?||object(&self.root)?!=object(&host_scope(&self.proc,&self.original)?)?
            ||self.stamp!=ImageStamp::read(&self.image)?||self.stamp!=ImageStamp::read(&running_image(&self.proc)?)?{
            return Err(Fault::Handoff);
        }
        self.original.require_live()
    }
    pub(super) fn own_proc(&self)->Result<&File>{self.require()?;Ok(&self.proc)}
    pub(super) fn own_task(&self)->Result<File>{self.require()?;current_task(&self.proc)}
    pub(super) fn host(&self)->Result<&File>{self.require()?;Ok(&self.root)}
    /// Identity comparison only. The independently selected approved program
    /// and its digest/whole closure must still be checked by their real owner.
    /// The eventually executed FD must be the same one that was compared.
    pub(super) fn same_image(&self,approved:&File)->Result<()>{
        self.require()?;
        if ImageStamp::read(approved)?!=self.stamp{return Err(Fault::Handoff);}
        self.original.require_live()
    }
    // pthread_join joins the one assembler's userspace lifetime; a brief
    // kernel exit bookkeeping window can still show two threads. No exec
    // birth is accepted until the unchanged complete one-thread proof.
    fn after_builder(&self)->Result<()>{
        let until=std::time::Instant::now()+std::time::Duration::from_millis(200);
        loop{
            let status=bounded(&current_task(&self.proc)?,c"status",65_536)?;
            let n=words(row(&status,b"Threads:")?).collect::<Vec<_>>();
            match n.as_slice(){
                [b"1"]=>return self.require(),
                [b"2"] if std::time::Instant::now()<until=>std::thread::sleep(std::time::Duration::from_millis(1)),
                _=>return Err(Fault::Handoff),
            }
        }
    }
    pub(super) fn prepare(&self)->Result<PreparedHandoff<'_>>{
        self.require()?;
        let (host,child)=kernel::fresh_pair()?;
        self.require()?;
        Ok(PreparedHandoff{parent:self,host,child})
    }
}

#[derive(Clone,Copy,Eq,PartialEq)]
struct Object{device:u64,inode:u64,mount:UniqueMountId}
fn object(file:&File)->Result<Object>{
    let m=file.metadata().map_err(|_|Fault::Handoff)?;
    let mount=descriptor::unique_mount(file.as_raw_fd()).ok_or(Fault::Handoff)?;
    if m.ino()==0{return Err(Fault::Handoff);}
    Ok(Object{device:m.dev(),inode:m.ino(),mount})
}
#[derive(Clone,Copy,Eq,PartialEq)]
struct ImageStamp{
    object:Object,bytes:u64,mode:u32,uid:u32,gid:u32,ctime:(i64,i64),mtime:(i64,i64),
}
impl ImageStamp{
    fn read(f:&File)->Result<Self>{
        let m=f.metadata().map_err(|_|Fault::Handoff)?;
        if m.mode()!=0o100755||m.uid()!=0||m.gid()!=0||m.nlink()!=1||m.len()==0{return Err(Fault::Handoff);}
        Ok(Self{object:object(f)?,bytes:m.len(),mode:m.mode(),uid:m.uid(),gid:m.gid(),
            ctime:(m.ctime(),m.ctime_nsec()),mtime:(m.mtime(),m.mtime_nsec())})
    }
}
fn proc_root()->Result<File>{
    // A bare terminal O_PATH must not activate an unexpected automount.
    let p=OpenOptions::new().read(true).custom_flags(PIN).open("/proc").map_err(|_|Fault::Handoff)?;
    let m=p.metadata().map_err(|_|Fault::Handoff)?;
    if kernel::fs_magic(p.as_raw_fd())?!=PROC||!m.is_dir()||m.ino()!=1{return Err(Fault::Handoff);}
    Ok(p)
}
fn proc_at(dir:&File,name:&CStr,recipe:ProcOpen)->Result<File>{
    let f=File::from(held_open::proc(dir,name,recipe).map_err(|_|Fault::Handoff)?);
    if kernel::fs_magic(f.as_raw_fd())?!=PROC{return Err(Fault::Handoff);}
    Ok(f)
}
fn follow_kernel_link(dir:&File,name:&CStr,flags:i32)->Result<File>{
    let link=proc_at(dir,name,ProcOpen::Pin)?;
    let stamp=object(&link)?;
    if !link.metadata().map_err(|_|Fault::Handoff)?.file_type().is_symlink()||stamp.mount!=object(dir)?.mount{
        return Err(Fault::Handoff);
    }
    // SAFETY: follow just the previously pinned literal kernel proc link;
    // retain its source and require the matching proc inode again after.
    // There is no caller path, parser choice or directory-relative traversal.
    let target=unsafe{
        let ret=libc::openat(dir.as_raw_fd(),name.as_ptr(),flags|libc::O_CLOEXEC);
        File::from(kernel::new_fd(ret.into())?)
    };
    if object(&proc_at(dir,name,ProcOpen::Pin)?)?!=stamp{return Err(Fault::Handoff);}
    Ok(target)
}
fn proc_dir(parent:&File,name:&CStr)->Result<File>{
    let f=proc_at(parent,name,ProcOpen::Pin)?;
    if !f.metadata().map_err(|_|Fault::Handoff)?.is_dir(){return Err(Fault::Handoff);}
    Ok(f)
}
fn current_task(proc:&File)->Result<File>{
    let mut tid=0;
    // SAFETY: one native long destination for the actual task ID.
    let pid=unsafe{observe_own_task_ids(&raw mut tid)};
    if pid<=1||c_long::from(pid)!=tid{return Err(Fault::Handoff);}
    // thread-self is a terminal *ordinary symlink*, not a directory.
    // Validate its real bounded spelling, then pin the generated numeric
    // path inside this procfs without following the terminal alias.
    let link=proc_at(proc,c"thread-self",ProcOpen::Pin)?;
    let stamp=object(&link)?;
    if !link.metadata().map_err(|_|Fault::Handoff)?.file_type().is_symlink(){return Err(Fault::Handoff);}
    let name=CString::new(format!("{pid}/task/{tid}")).map_err(|_|Fault::Handoff)?;
    let mut bytes=[0u8;64];
    // SAFETY: empty path operates on the retained symlink; writable span
    // is bounded, and truncated results will fail the exact comparison.
    let n=unsafe{libc::readlinkat(link.as_raw_fd(),c"".as_ptr(),bytes.as_mut_ptr().cast(),bytes.len())};
    let want=name.as_bytes();
    if n!=want.len() as isize||!bytes.starts_with(want){return Err(Fault::Handoff);}
    let id=CString::new(pid.to_string()).map_err(|_|Fault::Handoff)?;
    let process=proc_dir(proc,&id)?;
    let tasks=proc_dir(&process,c"task")?;
    let thread=proc_dir(&tasks,&id)?;
    if object(&proc_at(proc,c"thread-self",ProcOpen::Pin)?)?!=stamp{return Err(Fault::Handoff);}
    Ok(thread)
}
fn running_image(proc:&File)->Result<File>{
    follow_kernel_link(&current_task(proc)?,c"exe",libc::O_RDONLY|libc::O_NONBLOCK|libc::O_NOCTTY)
}
fn no_other_descriptors(proc:&File)->Result<()>{
    let task=current_task(proc)?;
    let anchor=proc_dir(&task,c"fd")?;
    let dir=proc_at(&anchor,c".",ProcOpen::Directory)?;
    if object(&anchor)?!=object(&dir)?{return Err(Fault::Handoff);}
    let allowed=[0,1,2,proc.as_raw_fd(),task.as_raw_fd(),anchor.as_raw_fd(),dir.as_raw_fd()];
    let mut buf=[0u8;4096];
    loop{
        // SAFETY: Linux getdents64 on the retained current task's fd
        // table, with one bounded writable byte span and no extra directory.
        let n=unsafe{libc::syscall(libc::SYS_getdents64,dir.as_raw_fd(),buf.as_mut_ptr(),buf.len())};
        if n<0||n as usize>buf.len(){return Err(Fault::Handoff);}
        if n==0{return Ok(());}
        let mut raw=&buf[..n as usize];
        while !raw.is_empty(){
            let length=u16::from_ne_bytes(raw.get(16..18).ok_or(Fault::Handoff)?.try_into().expect("two bytes")) as usize;
            if length<20||!length.is_multiple_of(8)||length>raw.len(){return Err(Fault::Handoff);}
            let name=&raw[19..length]; // Linux linux_dirent64 d_name
            let name=name.split(|&v|v==0).next().ok_or(Fault::Handoff)?;
            if name==b"."||name==b".." {raw=&raw[length..];continue;}
            if name.is_empty()||!name.iter().all(u8::is_ascii_digit){return Err(Fault::Handoff);}
            let fd=std::str::from_utf8(name).map_err(|_|Fault::Handoff)?.parse::<i32>().map_err(|_|Fault::Handoff)?;
            if !allowed.contains(&fd){return Err(Fault::Handoff);}
            raw=&raw[length..];
        }
    }
}
fn row<'a>(data:&'a [u8],key:&[u8])->Result<&'a [u8]>{
    let mut rows=data.split(|&c|c==b'\n').filter_map(|r|r.strip_prefix(key));
    let r=rows.next().ok_or(Fault::Handoff)?;
    if rows.next().is_some()||r.first().is_none_or(|c|!c.is_ascii_whitespace()){return Err(Fault::Handoff);}
    Ok(r)
}
fn words(bytes:&[u8])->impl Iterator<Item=&[u8]>{
    bytes.split(u8::is_ascii_whitespace).filter(|w|!w.is_empty())
}
fn caps(data:&[u8],key:&[u8])->Result<u64>{
    let mut values=words(row(data,key)?);
    let value=values.next().ok_or(Fault::Handoff)?;
    if values.next().is_some()||value.len()!=16||!value.iter().all(u8::is_ascii_hexdigit){return Err(Fault::Handoff);}
    let text=std::str::from_utf8(value).map_err(|_|Fault::Handoff)?;
    u64::from_str_radix(text,16).map_err(|_|Fault::Handoff)
}
fn bounded(proc:&File,path:&CStr,max:u64)->Result<Vec<u8>>{
    let pin=proc_at(proc,path,ProcOpen::Pin)?;
    if !pin.metadata().map_err(|_|Fault::Handoff)?.is_file(){return Err(Fault::Handoff);}
    let stamp=object(&pin)?;
    let f=proc_at(proc,path,ProcOpen::Record)?;
    if object(&f)?!=stamp{return Err(Fault::Handoff);}
    let mut data=Vec::new();
    f.take(max+1).read_to_end(&mut data).map_err(|_|Fault::Handoff)?;
    if data.len() as u64>max||!data.ends_with(b"\n")||object(&proc_at(proc,path,ProcOpen::Pin)?)?!=stamp{return Err(Fault::Handoff);}
    Ok(data)
}
fn waitable_children()->Result<()>{
    // SAFETY: queries SIGCHLD without installing a handler.
    if unsafe{observe_waitable_children()}!=1{return Err(Fault::Handoff);}
    Ok(())
}
fn host_task(proc:&File)->Result<()>{
    let mut ids=OwnProcessIds::default();
    // SAFETY: the C observation only writes the fixed ID record.
    unsafe{observe_own_process_ids(&raw mut ids);}
    let OwnProcessIds{pid,uid,euid,gid,egid}=ids;
    if pid<=1||[uid,euid,gid,egid]!=[0;4]{return Err(Fault::Handoff);}
    let id=pid.to_string();
    let current=current_task(proc)?;
    let status=bounded(&current,c"status",65_536)?;
    for key in [b"Tgid:".as_slice(),b"Pid:",b"NSpid:"]{
        if !words(row(&status,key)?).eq([id.as_bytes()]){return Err(Fault::Handoff);}
    }
    if !words(row(&status,b"Threads:")?).eq([b"1".as_slice()])||
        !words(row(&status,b"Uid:")?).eq([b"0".as_slice();4])||
        !words(row(&status,b"Gid:")?).eq([b"0".as_slice();4])||
        caps(&status,b"CapEff:")?&SUPERVISOR_CAPS!=SUPERVISOR_CAPS||
        caps(&status,b"CapBnd:")?&SERVICE_CAPS!=SERVICE_CAPS{
        return Err(Fault::Handoff);
    }
    for map in [c"uid_map",c"gid_map"]{
        if !words(&bounded(&current,map,4096)?).eq([b"0".as_slice(),b"0",b"4294967295"]){return Err(Fault::Handoff);}
    }
    waitable_children()
}

#[derive(Clone,Copy)]
#[repr(u8)]
enum Namespace{Ipc=0,Mount=1,Net=2,Pid=3,PidForChildren=4,User=5,Uts=6}
impl Namespace{
    fn name(self)->&'static CStr{
        match self{
            Self::Ipc=>c"ipc",
            Self::Mount=>c"mnt",
            Self::Net=>c"net",
            Self::Pid=>c"pid",
            Self::PidForChildren=>c"pid_for_children",
            Self::User=>c"user",
            Self::Uts=>c"uts",
        }
    }
    fn checked(self,f:File)->Result<File>{
        if kernel::fs_magic(f.as_raw_fd())?!=NSFS{return Err(Fault::Handoff);}
        // SAFETY: fixed kind observation on the held namespace descriptor.
        if unsafe{observe_namespace_kind(f.as_raw_fd(),self as u8)}!=1{return Err(Fault::Handoff);}
        Ok(f)
    }
    fn of_parent(self,pid:&OriginalProcess)->Result<File>{
        // SAFETY: fixed namespace request on the held original pidfd;
        // success returns one new owned descriptor.
        let ns=unsafe{File::from(kernel::new_fd(open_process_namespace(pid.as_raw_fd(),self as u8).into())?)};
        descriptor::cloexec(ns.as_raw_fd())?;
        self.checked(ns)
    }
    fn of_init(self,ns:&File)->Result<File>{
        self.checked(follow_kernel_link(ns,self.name(),libc::O_RDONLY|libc::O_NOCTTY)?)
    }
    fn owner_of_pid(self,pid:&File)->Result<File>{
        if !matches!(self,Self::User){return Err(Fault::Handoff);}
        // SAFETY: NS_GET_USERNS on the already verified original PID ns
        // yields one new CLOEXEC file, not a borrowed pointer or numeric id.
        let f=unsafe{File::from(kernel::new_fd(open_namespace_owner(pid.as_raw_fd()).into())?)};
        descriptor::cloexec(f.as_raw_fd())?;
        self.checked(f)
    }
}
fn host_scope(proc:&File,pid:&OriginalProcess)->Result<File>{
    pid.require_live()?;
    host_task(proc)?;
    let active=Namespace::Pid.of_parent(pid)?;
    let user=Namespace::User.of_parent(pid)?;
    if object(&active)?.inode!=PID_INIT||object(&user)?.inode!=USER_INIT||
        object(&active)?!=object(&Namespace::PidForChildren.of_parent(pid)?)?{return Err(Fault::Handoff);}
    // Only after proving the actual kernel initial PID namespace can /proc's
    // reserved PID 1 be compared. thread-self/status above must also expose
    // the actual calling PID with no ancestor/descendant translation.
    let init=proc_dir(proc,c"1")?;
    let namespaces=proc_dir(&init,c"ns")?;
    for n in [Namespace::Pid,Namespace::User,Namespace::Mount,Namespace::Net]{
        if object(&n.of_parent(pid)?)?!=object(&n.of_init(&namespaces)?)?{return Err(Fault::Handoff);}
    }
    let root=follow_kernel_link(&init,c"root",libc::O_PATH|libc::O_DIRECTORY)?;
    let current=OpenOptions::new().read(true).custom_flags(DIR).open("/").map_err(|_|Fault::Handoff)?;
    if object(&root)?!=object(&current)?{return Err(Fault::Handoff);}
    pid.require_live()?;
    Ok(root)
}
