//! Match the manager's retained main invocation to an actual live process.
//! One pidfd prevents accepting a reused numeric PID. No process is signaled
//! by numeric PID, no credentials or full process environment are read.
use super::{bad,web_systemd_state::CGROUP};
use crate::{pinned::{Entity,Mount,Pinned},sqlite_install::approval};
use rustix::{event::{PollFd,PollFlags,Timespec,poll},
    fs::{Mode,OFlags,PROC_SUPER_MAGIC,RawDir,ResolveFlags,fstatfs,openat,openat2},io::Errno,
    process::{Pid,PidfdFlags,Signal,pidfd_open,pidfd_send_signal}};
use std::{ffi::CStr,fs::File,io::{self,Read},mem::MaybeUninit,os::unix::ffi::OsStrExt};

const DIRECTORY:OFlags=OFlags::PATH.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
const SAME:ResolveFlags=ResolveFlags::BENEATH.union(ResolveFlags::NO_XDEV).union(ResolveFlags::NO_MAGICLINKS).union(ResolveFlags::NO_SYMLINKS);
fn no()->io::Error{bad("the systemd main process is not provably the original native Web executable, identity and pool")}
pub(super) struct Process{original:File,proc:File,pid:Pid}
impl Process{
    pub(super) fn select(pid:Pid)->io::Result<Self>{
        let original=File::from(pidfd_open(pid,PidfdFlags::empty())?);
        let (proc,_)=approval::own_descriptor_table()?;
        let out=Self{original,proc,pid};out.live()?;Ok(out)
    }
    pub(super) fn exited(&self)->io::Result<bool>{
        let mut fds=[PollFd::new(&self.original,PollFlags::IN)];
        let count=poll(&mut fds,Some(&Timespec{tv_sec:0,tv_nsec:0}))?;
        match (count,fds[0].revents()){
            (0,f) if f.is_empty()=>Ok(false),
            (1,f) if f.contains(PollFlags::IN)&&(f-PollFlags::IN-PollFlags::HUP).is_empty()=>Ok(true),
            _=>Err(no()),
        }
    }
    fn live(&self)->io::Result<()>{if self.exited()?{Err(no())}else{Ok(())}}
    pub(super) fn terminate(&self)->io::Result<()>{
        self.live()?;pidfd_send_signal(&self.original,Signal::TERM)?;Ok(())
    }
    pub(super) fn require(&self,elf:&File,system:&Pinned,marker:Entity,uid:u32,gid:u32)->io::Result<()>{
        self.live()?;let mount=Mount::on(&self.proc)?.ok_or_else(no)?;
        if mount.at_root!=Some(true)||fstatfs(&self.proc)?.f_type!=PROC_SUPER_MAGIC{return Err(no());}
        let dir=File::from(openat2(&self.proc,self.pid.as_raw_pid().to_string().as_str(),DIRECTORY,Mode::empty(),SAME)?);
        for name in [c"exe",c"root"]{
            let pin=File::from(openat2(&dir,name,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty(),SAME)?);
            if !pin.metadata()?.file_type().is_symlink()||Mount::on(&pin)?.is_none_or(|m|m.id!=mount.id){
                return Err(no());
            }
        }
        let exe=File::from(openat(&dir,c"exe",OFlags::PATH|OFlags::CLOEXEC,Mode::empty())?);
        // The approved unit intentionally gets a different mount namespace.
        if !exe.metadata()?.is_file()||Entity::of(&exe.metadata()?)?!=Entity::of(&elf.metadata()?)?{
            return Err(no());
        }
        let root=File::from(openat(&dir,c"root",OFlags::PATH|OFlags::DIRECTORY|OFlags::CLOEXEC,Mode::empty())?);
        let d=File::from(openat2(&root,&system.path().as_os_str().as_bytes()[1..],DIRECTORY,Mode::empty(),
            ResolveFlags::BENEATH|ResolveFlags::NO_MAGICLINKS|ResolveFlags::NO_SYMLINKS)?);
        if Entity::of(&d.metadata()?)?!=system.entity(){return Err(no());}
        let status=data(&dir,c"status",65536)?;
        let values=|key:&[u8],want:u32,count:usize|{
            let mut lines=status.split(|b|*b==b'\n').filter_map(|l|l.strip_prefix(key));
            let val=want.to_string();
            lines.next().is_some_and(|line|{
                line.split(u8::is_ascii_whitespace).filter(|s|!s.is_empty())
                    .eq(std::iter::repeat_n(val.as_bytes(),count))
            })&&lines.next().is_none()
        };
        let clear=|key:&[u8]|{
            let mut lines=status.split(|b|*b==b'\n').filter_map(|l|l.strip_prefix(key));
            lines.next().is_some_and(|v|v.strip_prefix(b"\t")==Some(b"0000000000000000"))
                &&lines.next().is_none()
        };
        if !values(b"Pid:",self.pid.as_raw_pid() as u32,1)||!values(b"PPid:",1,1)||
            !values(b"Uid:",uid,4)||!values(b"Gid:",gid,4)||!values(b"NoNewPrivs:",1,1)
            ||[b"CapBnd:",b"CapPrm:",b"CapEff:",b"CapAmb:"].iter().any(|k|!clear(k.as_slice())){
            return Err(no());
        }
        if data(&dir,c"cgroup",4096)?!=format!("0::{CGROUP}\n").as_bytes(){return Err(no());}
        let source=system.entity();let running_mount=Mount::on(&d)?.ok_or_else(no)?.id;
        if !holds_original_system(&dir,mount.id,source,marker,running_mount,self.pid)?{
            return Err(io::Error::new(io::ErrorKind::WouldBlock,"the exact process has not shown both the shared SYSTEM lease and retained same-pool marker"));
        }
        self.live()
    }
}
fn pid_info(data:&[u8],source:Entity,mount:u64,pid:Pid)->bool{
    let Ok(text)=std::str::from_utf8(data)else{return false;};
    let mut got=[false;3];
    for row in text.lines(){
        let Some((k,v))=row.split_once(':')else{return false;};
        let v=v.trim_ascii();
        let i=match k{
            "mnt_id" if v.parse::<u64>()==Ok(mount)=>0,
            "ino" if v.parse::<u64>()==Ok(source.ino)=>1,
            "lock"=>{
                let fields=v.split_ascii_whitespace().collect::<Vec<_>>();
                let ["1:","FLOCK","ADVISORY","READ",owner,device,"0","EOF"]=fields.as_slice()else{return false;};
                if owner.parse::<i32>()!=Ok(pid.as_raw_pid()){return false;}
                let parts=device.split(':').collect::<Vec<_>>();
                let [major,minor,ino]=parts.as_slice()else{return false;};
                if u32::from_str_radix(major,16)!=Ok(rustix::fs::major(source.dev))
                    ||u32::from_str_radix(minor,16)!=Ok(rustix::fs::minor(source.dev))
                    ||ino.parse::<u64>()!=Ok(source.ino){return false;}2
            }
            "mnt_id"|"ino"=>return false,_=>continue,
        };
        if std::mem::replace(&mut got[i],true){return false;}
    }
    got==[true;3]&&text.ends_with('\n')
}
#[derive(Clone,Copy,PartialEq,Eq)]
enum Held{System,Marker}
fn holds_original_system(dir:&File,proc_mount:u64,source:Entity,marker:Entity,mount:u64,pid:Pid)->io::Result<bool>{
    if source==marker{return Err(no());}
    // Per-PID procfs descriptors, never the process's environment or any
    // descriptor contents. O_PATH following the kernel's fd link supplies
    // only its resolved entity. The pinned ZXS InitialReader holds the
    // selected pool marker FD through its service lifetime, not just a path
    // to the same directory. Verify that exact inode without reading its FD.
    let fds=File::from(openat2(dir,c"fd",(DIRECTORY-OFlags::PATH)|OFlags::RDONLY,Mode::empty(),SAME)?);
    let info=File::from(openat2(dir,c"fdinfo",DIRECTORY,Mode::empty(),SAME)?);
    if [&fds,&info].iter().any(|f|Mount::on(*f).ok().flatten().is_none_or(|m|m.id!=proc_mount)){return Err(no());}
    let mut buffer=[MaybeUninit::uninit();4096];let mut entries=RawDir::new(&fds,&mut buffer);
    let mut count=0;let(mut locked,mut retained)=(false,false);
    while let Some(entry)=entries.next(){
        let entry=entry?;let name=entry.file_name();let raw=name.to_bytes();
        if matches!(raw,b"."|b".."){continue;}
        count+=1;
        if count>4096||raw.is_empty()||raw.len()>10||!raw.iter().all(u8::is_ascii_digit){return Err(no());}
        let pin=match openat2(&fds,name,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty(),SAME){
            Ok(f)=>File::from(f),Err(Errno::NOENT)=>continue,Err(e)=>return Err(e.into()),
        };
        if !pin.metadata()?.file_type().is_symlink()||Mount::on(&pin)?.is_none_or(|m|m.id!=proc_mount){return Err(no());}
        let selects=||->io::Result<Option<Held>>{
            let f=match openat(&fds,name,OFlags::PATH|OFlags::CLOEXEC,Mode::empty()){
                Ok(f)=>File::from(f),Err(Errno::NOENT)=>return Ok(None),Err(e)=>return Err(e.into()),
            };
            let m=f.metadata()?;
            if !m.is_dir()&&!m.is_file(){return Ok(None);}
            let original=Entity::of(&m)?;
            if original!=source&&original!=marker || Mount::on(&f)?.is_none_or(|m|m.id!=mount){return Ok(None);}
            Ok(if m.is_dir()&&original==source{Some(Held::System)}
                else if m.is_file()&&original==marker{Some(Held::Marker)}else{None})
        };
        match selects()?{
            Some(Held::System) if pid_info(&data(&info,name,4096)?,source,mount,pid)=>{
                if selects()?!=Some(Held::System)||!pid_info(&data(&info,name,4096)?,source,mount,pid)
                    ||selects()?!=Some(Held::System){return Err(no());}
                locked=true;
            }
            Some(Held::Marker)=>{
                if selects()?!=Some(Held::Marker){return Err(no());}
                retained=true;
            }
            _=>{},
        }
    }
    Ok(locked&&retained)
}
fn data(dir:&File,name:&CStr,limit:u64)->io::Result<Vec<u8>>{
    let file=File::from(openat2(dir,name,OFlags::RDONLY|OFlags::NOFOLLOW|OFlags::NONBLOCK|OFlags::CLOEXEC,Mode::empty(),SAME)?);
    if !file.metadata()?.is_file()||fstatfs(&file)?.f_type!=PROC_SUPER_MAGIC{return Err(no());}
    let mut out=Vec::new();file.take(limit+1).read_to_end(&mut out)?;
    if out.len() as u64>limit||!out.ends_with(b"\n"){return Err(no());}Ok(out)
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn availability_of_the_selected_path_is_not_an_original_shared_lease(){
        let source=Entity{dev:rustix::fs::makedev(8,1),ino:1000};
        let pid=Pid::from_raw(200).unwrap();
        let exact="pos:\t0\nflags:\t02100000\nmnt_id:\t40\nino:\t1000\nlock:\t1: FLOCK ADVISORY READ 200 08:01:1000 0 EOF\n";
        assert!(pid_info(exact.as_bytes(),source,40,pid));
        for line in [exact.replace("READ","WRITE"),exact.replace("200 08","201 08"),exact.replace("08:01","08:02"),
            exact.replace("08:01:1000","08:01:1001"),exact.replace("40","10"),exact.lines().take(4).collect::<Vec<_>>().join("\n")+"\n"]{
            assert!(!pid_info(line.as_bytes(),source,40,pid));
        }
    }
}
