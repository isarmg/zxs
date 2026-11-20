//! Ask the exact host manager whether one fixed native-HTTP service is
//! currently unresolved. Without a main fragment, systemd can stop before
//! reading otherwise applicable drop-ins. This checks only the current
//! lookup; it never proves what a new fragment would load or launch.
use super::{bad,inactive_web_unit::{UNIT,require_amd64_elf}};
use crate::fs_tools::RuntimeFile;
use rustix::{io::Errno,net::{AddressFamily,SocketAddrUnix,SocketFlags,SocketType,connect,socket,sockopt},
    pipe::{PipeFlags,pipe_with}};
use std::{collections::BTreeMap,fs::File,io,os::fd::AsRawFd,
    process::{Child,Command,Stdio},time::{Duration,Instant}};

const TOOL:&[&str]=&["usr","bin","systemctl"];
const CAP:usize=8192;
const FIELDS:&[(&str,&str)]=&[("Id",UNIT),("LoadState","not-found"),("ActiveState","inactive"),
    ("FragmentPath",""),("DropInPaths",""),("SourcePath",""),("Transient","no")];
fn no()->io::Error{bad("the manager did not report the fixed Web service as currently unresolved")}
fn unresolved(data:&[u8])->io::Result<()>{
    let s=std::str::from_utf8(data).map_err(|_|no())?.strip_suffix('\n').ok_or_else(no)?;
    if data.len()>CAP{return Err(no());}
    let mut rows=BTreeMap::new();
    for line in s.split('\n'){
        let (k,v)=line.split_once('=').ok_or_else(no)?;
        if rows.insert(k,v).is_some(){return Err(no());}
    }
    if rows.len()!=FIELDS.len()||FIELDS.iter().any(|(k,v)|rows.get(k)!=Some(v)){return Err(no());}
    Ok(())
}
fn manager()->io::Result<()>{
    // Real host identity/namespace is checked by the caller and again before
    // mutation. Root systemctl's BUS_MANAGER local transport uses this exact
    // private pathname; no user bus, SSH, machine transport or environment
    // override is admitted. The actual kernel peer must be host PID 1.
    let peer=socket(AddressFamily::UNIX,SocketType::STREAM,SocketFlags::CLOEXEC|SocketFlags::NONBLOCK,None)?;
    connect(&peer,&SocketAddrUnix::new("/run/systemd/private")?)?;
    let who=sockopt::socket_peercred(&peer)?;
    if who.pid.as_raw_pid()!=1||!who.uid.is_root(){return Err(no());}
    Ok(())
}
struct Running(Option<Child>);
impl Drop for Running{
    fn drop(&mut self){if let Some(mut c)=self.0.take(){let _=c.kill();let _=c.wait();}}
}
fn call(tool:&RuntimeFile,check:impl Fn()->io::Result<()>)->io::Result<Vec<u8>>{
    check()?;manager()?;tool.recheck()?;
    let (read,write)=pipe_with(PipeFlags::CLOEXEC|PipeFlags::NONBLOCK)?;
    // Exec only the already verified executable descriptor. No command, unit,
    // options or connection originate in the browser or caller argv.
    let path=format!("/proc/thread-self/fd/{}",tool.file.as_raw_fd());
    let mut cmd=Command::new(path);
    let child=cmd.args(["--system","--no-ask-password","--no-pager","--all",
            "--property=Id,LoadState,ActiveState,FragmentPath,DropInPaths,SourcePath,Transient","show",UNIT])
        .env_clear().env("LC_ALL","C").env("PATH","/usr/bin")
        .stdin(Stdio::null()).stdout(Stdio::from(File::from(write))).stderr(Stdio::null()).spawn()?;
    drop(cmd);
    let mut child=Running(Some(child));let deadline=Instant::now()+Duration::from_secs(10);
    let mut bytes=Vec::new();let mut eof=false;let mut done=false;
    loop{
        check()?;
        if Instant::now()>=deadline{return Err(no());}
        if !eof{
            let mut b=[0u8;512];
            match rustix::io::read(&read,&mut b[..]){
                Ok(0)=>eof=true,
                Ok(n) if bytes.len()+n<=CAP=>bytes.extend_from_slice(&b[..n]),
                Ok(_)=>return Err(no()),
                Err(Errno::INTR|Errno::AGAIN)=>(),Err(e)=>return Err(e.into()),
            }
        }
        if let Some(c)=child.0.as_mut(){if let Some(s)=c.try_wait()?{
            child.0=None;if !s.success(){return Err(no());}done=true;
        }}
        if eof&&done{break;}
        std::thread::sleep(Duration::from_millis(15));
    }
    manager()?;tool.recheck()?;check()?;Ok(bytes)
}
pub(super) fn require_unresolved(check:impl Fn()->io::Result<()>)->io::Result<()>{
    // The executable is from the trusted, protected OS installation, not
    // downloaded/installed by Helper. The operator administers the real host.
    let tool=RuntimeFile::open(TOOL,true,16<<20)?;
    require_amd64_elf(&tool.file)?;
    unresolved(&call(&tool,check)?)
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn only_the_exact_current_unresolved_manager_shape_qualifies(){
        let data=FIELDS.iter().map(|(k,v)|format!("{k}={v}\n")).collect::<String>();
        unresolved(data.as_bytes()).unwrap();
        for wrong in [data.replace("not-found","loaded"),data.replace("inactive","active"),
            data.replace("DropInPaths=\n","DropInPaths=/etc/shadow.conf\n"),
            data.replace("Transient=no","Transient=yes"),data.clone()+"Id=zxs-management-http.service\n",
            data.replace("SourcePath=\n","")]{assert!(unresolved(wrong.as_bytes()).is_err());}
    }
}
