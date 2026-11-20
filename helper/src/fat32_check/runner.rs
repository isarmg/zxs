//! One isolated invocation. The checker never gets host /dev, the caller's
//! namespace/terminal, any host directory, or an arbitrary option. Only the
//! leased branches bind just the explicitly saved copy.
use super::{Format,btrfs,ext4,ntfs,xfs,group::Group,image::{ExfatSealed,Sealed},runtime::Runtime};
use crate::pinned::invalid;
use rustix::{
    fd::{AsFd,AsRawFd,OwnedFd},
    fs::{OFlags,fcntl_getfl,fcntl_setfl},
    io::{FdFlags,fcntl_dupfd_cloexec,fcntl_setfd},
    pipe::{PipeFlags,pipe_with},
    process::{Resource,Rlimit,getgid,getrlimit,getuid,setrlimit},
};
use std::{fs::File,io::{self,Read,Write},process::{Command,Stdio},thread,time::{Duration,Instant}};

const OUTPUT:usize=64*1024;
const STATUS:usize=4096;
const LIMIT:Duration=Duration::from_secs(30);
const LEASED_LIMIT:Duration=Duration::from_secs(10*60);

#[derive(Clone,Copy,PartialEq,Eq)]
enum Budget{Sealed,Leased,Xfs}
pub(super) fn prepare_worker()->io::Result<()> {prepare(Budget::Sealed)}
pub(super) fn prepare_leased_worker()->io::Result<()> {prepare(Budget::Leased)}
pub(super) fn prepare_xfs_worker()->io::Result<()> {prepare(Budget::Xfs)}
fn prepare(budget:Budget)->io::Result<()> {
    // This command owns the entire CLI process. Never lower process rlimits or
    // manipulate descriptor inheritance in an embedding service.
    rustix::thread::set_no_new_privs(true)?;
    crate::maintenance::diagnostic_worker(getuid().as_raw(),getgid().as_raw())?;
    helper_linux_evidence::isolate_check_descriptors()?;
    super::group::prepare()?;
    let leased=budget!=Budget::Sealed;
    // The original xfs_repair refuses to proceed if it cannot set a finite
    // FSIZE to infinity. Do not raise an inherited limit, even for this child.
    if budget==Budget::Xfs&&getrlimit(Resource::Fsize).current.is_some(){
        return Err(invalid("upstream XFS requires an inherited unlimited file-size limit; nothing was raised"));
    }
    // The ext guest has thirteen authenticated slots with each original
    // ancestry held, not eight. Allow those fds plus temporary probes/pipes.
    for (r,n) in [(Resource::Cpu,12),(Resource::As,if leased{2<<30}else{512<<20}),
                  (Resource::Fsize,if leased{64<<10}else{140<<20}),
                  (Resource::Core,0),(Resource::Nofile,if leased{192}else{128}),(Resource::Nproc,if leased{256}else{64})]{
        if leased&&r==Resource::Cpu || budget==Budget::Xfs&&r==Resource::Fsize{continue;}
        let old=getrlimit(r);
        let hard=old.maximum.map_or(n,|h|h.min(n));let soft=old.current.map_or(hard,|s|s.min(hard));
        // XFS attempts to restore NOFILE/AS soft from hard, so make the
        // contained process's limits equal and never higher than inheritance.
        let hard=if budget==Budget::Xfs{soft}else{hard};
        setrlimit(r,Rlimit{current:Some(soft),maximum:Some(hard)})?;
    }
    Ok(())
}
fn give(fd:&impl AsFd)->io::Result<OwnedFd> {
    // Every other descriptor was placed behind a CLOEXEC barrier. Never
    // clear that bit on an original, only on these private, explicitly passed duplicates.
    let dupe=fcntl_dupfd_cloexec(fd,10)?;
    fcntl_setfd(&dupe,FdFlags::empty())?;Ok(dupe)
}

pub(super) fn check(runtime:&Runtime,image:&Sealed)->io::Result<super::Report> {
    runtime.require_format(Format::Fat32)?;
    let (code,out,err,status)=invoke(runtime,Operand::Sealed(image.file()))?;
    classify(code,&out,&err,&status,image.geometry().data_clusters)
}
pub(super) fn check_exfat(runtime:&Runtime,image:&ExfatSealed)->io::Result<super::exfat::Report>{
    runtime.require_format(Format::Exfat)?;
    let (code,out,err,status)=invoke(runtime,Operand::Sealed(image.file()))?;
    if !delivered(&status,code){return Err(invalid("exFAT checker delivery and actual exit not confirmed"));}
    super::exfat::classify(code,&out,&err,image.frame())
}
pub(super) fn check_ext4(runtime:&Runtime,image:&ext4::Image)->io::Result<ext4::Report>{
    runtime.require_ext4()?;
    let (code,out,err,status)=invoke(runtime,Operand::LeasedExt4(image))?;
    image.require()?;
    if !delivered(&status,code){return Err(invalid("e2fsck delivery or actual exit not confirmed"));}
    Ok(ext4::classify(code,&out,&err,image.extent()))
}
pub(super) fn check_xfs(runtime:&Runtime,image:&xfs::Image)->io::Result<xfs::Report>{
    runtime.require_xfs()?;
    let (code,out,err,status)=invoke(runtime,Operand::LeasedXfs(image))?;
    image.require()?;
    if !delivered(&status,code){return Err(invalid("xfs_repair delivery or actual exit not confirmed"));}
    Ok(xfs::classify(code,&out,&err,image.extent()))
}
pub(super) fn observe_ntfs(runtime:&Runtime,image:&ntfs::Image)->io::Result<ntfs::Report>{
    runtime.require_ntfs()?;
    let (code,out,err,status)=invoke(runtime,Operand::LeasedNtfs(image))?;
    image.require()?;
    if !delivered(&status,code){return Err(invalid("ntfsfix delivery or actual exit not confirmed"));}
    Ok(ntfs::classify(code,&out,&err,image.extent()))
}
pub(super) fn check_btrfs(runtime:&Runtime,image:&btrfs::Image)->io::Result<btrfs::Report>{
    runtime.require_btrfs()?;
    let (code,out,err,status)=invoke(runtime,Operand::LeasedBtrfs(image))?;
    image.require()?;
    if !delivered(&status,code){return Err(invalid("Btrfs checker delivery or actual exit not confirmed"));}
    Ok(btrfs::classify(code,&out,&err,image.extent()))
}
#[derive(Clone,Copy)]
enum Operand<'a>{Sealed(&'a File),LeasedExt4(&'a ext4::Image),LeasedXfs(&'a xfs::Image),LeasedNtfs(&'a ntfs::Image),LeasedBtrfs(&'a btrfs::Image)}
impl<'a> Operand<'a>{
    fn lease(self)->Option<&'a File>{
        match self{Self::Sealed(_)=>None,Self::LeasedExt4(i)=>Some(i.file()),Self::LeasedXfs(i)=>Some(i.file()),Self::LeasedNtfs(i)=>Some(i.file()),Self::LeasedBtrfs(i)=>Some(i.file())}
    }
    fn sandbox(self,cmd:&mut Command)->io::Result<OwnedFd>{
        match self{
            Self::Sealed(copy)=>{
                // The sealed readback has rewound this description to zero.
                let fd=give(copy)?;
                cmd.args(["--perms","0400","--ro-bind-data",&fd.as_raw_fd().to_string(),"/image"]);
                Ok(fd)
            }
            _=>{
                let fd=give(self.lease().expect("leased operand"))?;
                cmd.args(["--ro-bind-fd",&fd.as_raw_fd().to_string(),"/image"]);
                Ok(fd)
            }
        }
    }
}
fn invoke(runtime:&Runtime,copy:Operand<'_>)->io::Result<(i32,Vec<u8>,Vec<u8>,Vec<u8>)>{
    let ext=matches!(copy,Operand::LeasedExt4(_));
    let xfs=matches!(copy,Operand::LeasedXfs(_));
    let ntfs=matches!(copy,Operand::LeasedNtfs(_));
    let btrfs=matches!(copy,Operand::LeasedBtrfs(_));
    let (stdin,fin)=pipe_with(PipeFlags::CLOEXEC)?;drop(fin); // real EOF, never a terminal
    let empty=if ext||ntfs||btrfs{Some(give(&stdin)?)}else{None};
    let (rstatus,wstatus)=pipe_with(PipeFlags::CLOEXEC)?;
    let mut b=Command::new("/no-path-execution-for-helper-checker");
    b.env_clear().env("LC_ALL","C").current_dir("/")
        .stdin(Stdio::from(File::from(stdin))).stdout(Stdio::piped()).stderr(Stdio::piped())
        .args(["--unshare-user","--unshare-pid","--unshare-net","--unshare-ipc","--unshare-uts",
               "--disable-userns","--die-with-parent","--cap-drop","ALL",
               "--clearenv","--setenv","LC_ALL","C","--chdir","/"]);
    let mut given=Vec::new();
    for (f,dest) in runtime.sandbox_files(){
        let fd=give(f)?;b.args(["--ro-bind-fd",&fd.as_raw_fd().to_string(),dest]);given.push(fd);
    }
    if let Some(ref fd)=empty{
        if ext{
            // This e2fsck opens /dev/null O_RDWR only for open/close fd checks.
            // It is an empty regular file on a dedicated tiny tmpfs, never a
            // node or a path to host /dev. No mount table or other host resource
            // is supplied to the guest, which only sees the saved regular image.
            b.args(["--setenv","EXT2FS_NO_MTAB_OK","1","--size","65536","--tmpfs","/dev",
                    "--perms","0600","--file",&fd.as_raw_fd().to_string(),"/dev/null"]);
        }else if ntfs{
            // The pinned ntfsfix queries /proc/mounts then /etc/mtab.
            // Supply an empty file from this guest's EOF pipe, not a host
            // table, and seal it with the guest root. It says nothing
            // about the original disk's mount or Windows state.
            b.args(["--perms","0444","--file",&fd.as_raw_fd().to_string(),"/etc/mtab"]);
        }else{
            // btrfs-progs 6.14 check_mounted_where only opens this path.
            // Give it only a fixed, empty ordinary file in this new guest
            // (nothing mounted from the image), never procfs or a host
            // table. This does not infer that an original device is idle.
            b.args(["--perms","0444","--file",&fd.as_raw_fd().to_string(),"/proc/self/mounts"]);
        }
    }
    if xfs{
        // No devices or host tmp. This is just private, sized backing for
        // the native XFS O_TMPFILE scratch. The original image stays RO.
        b.args(["--perms","0700","--size","536870912","--tmpfs","/dev/shm"]);
    }
    let (filter_read,filter_write)=pipe_with(PipeFlags::CLOEXEC)?;
    {
        use helper_linux_evidence::diagnostic_process::{diagnostic_group_filter,ext4_group_filter,xfs_group_filter};
        let mut w=File::from(filter_write);
        w.write_all(&if ext{ext4_group_filter()}else if xfs{xfs_group_filter()}else{diagnostic_group_filter()})?;
    }
    let filter=give(&filter_read)?;b.args(["--seccomp",&filter.as_raw_fd().to_string()]);
    let operand=copy.sandbox(&mut b)?;let status=give(&wstatus)?;
    b.args(["--remount-ro","/","--json-status-fd",&status.as_raw_fd().to_string(),"--","/checker"]);
    if ext{b.args(["-fn","-N","/image","/image"]);}
    else if xfs{b.args(["-n","-f","-P","-t","0","-o","phase2_threads=1","/image"]);}
    else if btrfs{b.args(["check","--readonly","/image"]);}
    else{b.args(["-n","/image"]);} // includes the limited NTFS observer
    runtime.recheck()?;
    not_cancelled()?;
    use helper_linux_evidence::diagnostic_process::{prepare_diagnostic_exec,prepare_leased_diagnostic_exec,prepare_xfs_diagnostic_exec};
    match copy{
        Operand::Sealed(_)=>prepare_diagnostic_exec(&mut b,runtime.launcher())?,
        Operand::LeasedExt4(image)=>{image.require()?;prepare_leased_diagnostic_exec(&mut b,runtime.launcher())?;},
        Operand::LeasedNtfs(image)=>{image.require()?;prepare_leased_diagnostic_exec(&mut b,runtime.launcher())?;},
        Operand::LeasedBtrfs(image)=>{image.require()?;prepare_leased_diagnostic_exec(&mut b,runtime.launcher())?;},
        Operand::LeasedXfs(image)=>{image.require()?;prepare_xfs_diagnostic_exec(&mut b,runtime.launcher())?;},
    }
    let mut group=Group::new(b.spawn()?);
    // Only the sandbox monitor and its explicitly selected setup descriptions
    // retain writers. In particular the parent must not keep the status pipe open.
    drop(b);drop(given);drop(operand);drop(status);drop(wstatus);drop(filter);drop(filter_read);drop(empty);
    let lease=copy.lease();
    let result=collect(&mut group,File::from(rstatus),if lease.is_some(){LEASED_LIMIT}else{LIMIT},lease);
    let reaped=group.finish()?;
    let (code,out,err,status)=result?;
    not_cancelled()?;
    if reaped.leader_code!=Some(code){return Err(invalid("monitor exit did not match its independently reaped status"));}
    runtime.recheck()?;
    Ok((code,out,err,status))
}

fn not_cancelled()->io::Result<()> {
    if helper_linux_evidence::diagnostic_process::cancelled(){Err(io::Error::new(io::ErrorKind::Interrupted,"diagnostic cancelled"))}else{Ok(())}
}
fn nonblock(fd:&impl AsFd)->io::Result<()> {
    fcntl_setfl(fd,fcntl_getfl(fd)?|OFlags::NONBLOCK)?;Ok(())
}
fn drain(f:&mut impl Read,v:&mut Vec<u8>,max:usize)->io::Result<bool>{
    let mut buf=[0u8;4096];
    // A fixed number of reads per turn prevents continuous output from
    // starving the other pipe, wall-clock check or combined output budget.
    for _ in 0..8{match f.read(&mut buf){
        Ok(0)=>return Ok(true),Ok(n)=>{if n>max.saturating_sub(v.len()){return Err(invalid("checker exceeded the output budget"));}v.extend_from_slice(&buf[..n]);},
        Err(e) if e.kind()==io::ErrorKind::WouldBlock=>break,
        Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,Err(e)=>return Err(e),
    }} Ok(false)
}
fn collect(group:&mut Group,mut status:File,limit:Duration,lease:Option<&File>)->io::Result<(i32,Vec<u8>,Vec<u8>,Vec<u8>)>{
    let mut out=group.child.stdout.take().ok_or_else(||invalid("missing checker output pipe"))?;
    let mut err=group.child.stderr.take().ok_or_else(||invalid("missing checker error pipe"))?;
    nonblock(&out)?;nonblock(&err)?;nonblock(&status)?;
    let(mut o,mut e,mut s)=(Vec::new(),Vec::new(),Vec::new());
    let start=Instant::now();
    loop{
        not_cancelled()?;
        if let Some(file)=lease{helper_linux_evidence::diagnostic_process::require_copy_read_lease(file)?;}
        if start.elapsed()>limit{return Err(io::Error::new(io::ErrorKind::TimedOut,"isolated checker timed out"));}
        let a=drain(&mut out,&mut o,OUTPUT-e.len())?;let b=drain(&mut err,&mut e,OUTPUT-o.len())?;
        let c=drain(&mut status,&mut s,STATUS)?;
        if let Some(code)=group.peek_exit()? && a && b && c {
            return Ok((code,o,e,s));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

// Bubblewrap v0.12 reports this terminal object on its dedicated status fd
// only after its exec handoff. The checker cannot write to that descriptor.
fn delivered(status:&[u8],code:i32)->bool{
    let Ok(s)=std::str::from_utf8(status)else{return false;};
    let mut rows=s.lines();
    let expected=format!("{{ \"exit-code\": {code} }}");
    s.ends_with('\n') && rows.next_back()==Some(expected.as_str())
        && !rows.any(|line|line.contains("\"exit-code\""))
}
fn summary(line:&[u8],clusters:u32)->Option<(u32,u32)> {
    let s=std::str::from_utf8(line.strip_prefix(b"/image: ")?).ok()?;
    let (files,counts)=s.split_once(" files, ")?;
    let (used,total)=counts.strip_suffix(" clusters")?.split_once('/')?;
    let dec=|r:&str|(!r.is_empty()&&r.bytes().all(|b|b.is_ascii_digit())).then(||r.parse::<u32>().ok()).flatten();
    let (files,used,total)=(dec(files)?,dec(used)?,dec(total)?);
    (total==clusters && used<=total).then_some((files,used))
}
// With LC_ALL=C and no host gconv directories, upstream 4.2 can select
// its compiled-in CP850 table before opening /image. Only the exact complete
// startup notice is benign; never discard arbitrary per-file stderr.
const CP850_FALLBACK:&[u8]=b"Cannot initialize conversion from codepage 850 to ANSI_X3.4-1968: Invalid argument\nCannot initialize conversion from ANSI_X3.4-1968 to codepage 850: Invalid argument\nUsing internal CP850 conversion table\n";

fn classify(code:i32,out:&[u8],err:&[u8],status:&[u8],clusters:u32)->io::Result<super::Report> {
    let first=b"fsck.fat 4.2 (2021-01-31)";
    if !delivered(status,code) || !out.split(|b|*b==b'\n').next().is_some_and(|v|v==first) {
        return Err(invalid("checker launch, expected version or completed status not established"));
    }
    // Never echo image labels, filenames, terminal escapes or raw tool output.
    let Some(out)=out.strip_suffix(b"\n")else{return Err(invalid("checker output incomplete"));};
    let mut lines=out.split(|b|*b==b'\n');let _=lines.next();
    let (files,used)=lines.next_back().and_then(|l|summary(l,clusters))
        .ok_or_else(||invalid("missing complete final counts for the selected image"))?;
    let state=match code {
        0 if lines.next().is_none() && (err.is_empty()||err==CP850_FALLBACK)=>super::Outcome::NoFindingsReported,
        0=>super::Outcome::ReportedMessages,
        1=>super::Outcome::FindingsOrToolError,
        _=>return Err(invalid("filesystem check did not complete with a supported result")),
    };
    Ok(super::Report{state,files,used,total:clusters})
}

#[cfg(test)]
#[path="../../tests/fs_tools/fat32_runner.rs"]
mod tests;
