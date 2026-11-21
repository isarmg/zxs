//! Ask the exact host manager whether one fixed native-HTTP service is
//! currently unresolved. Without a main fragment, systemd can stop before
//! reading otherwise applicable drop-ins. This checks only the current
//! lookup; it never proves what a new fragment would load or launch.
use super::{Id,bad,inactive_web_unit::{UNIT,require_amd64_elf},web_systemd_state::{self,State}};
use crate::fs_tools::RuntimeFile;
use rustix::{io::Errno,net::{AddressFamily,SocketAddrUnix,SocketFlags,SocketType,connect,socket_with,sockopt},
    pipe::{PipeFlags,pipe_with}};
use std::{collections::BTreeMap,fs::File,io,os::fd::AsRawFd,
    path::Path,process::{Child,Command,Stdio},time::{Duration,Instant}};

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
    let peer=socket_with(AddressFamily::UNIX,SocketType::STREAM,SocketFlags::CLOEXEC|SocketFlags::NONBLOCK,None)?;
    connect(&peer,&SocketAddrUnix::new("/run/systemd/private")?)?;
    let who=sockopt::socket_peercred(&peer)?;
    if who.pid.as_raw_pid()!=1||!who.uid.is_root(){return Err(no());}
    Ok(())
}
struct Running(Option<Child>);
impl Drop for Running{
    fn drop(&mut self){if let Some(mut c)=self.0.take(){let _=c.kill();let _=c.wait();}}
}
#[derive(Clone,Copy)]
enum UnitName{Web,Sysinit,SystemSlice,RootSlice,Shutdown}
impl UnitName{
    fn as_str(self)->&'static str{match self{
        Self::Web=>UNIT,Self::Sysinit=>"sysinit.target",Self::SystemSlice=>"system.slice",
        Self::RootSlice=>"-.slice",Self::Shutdown=>"shutdown.target",
    }}
}
enum Verb<'a>{Read(&'a str,usize,UnitName),Start}
fn call(tool:&RuntimeFile,verb:Verb<'_>,check:impl Fn()->io::Result<()>)->io::Result<Vec<u8>>{
    helper_linux_evidence::diagnostic_process::require_waitable_children()?;
    check()?;manager()?;tool.recheck()?;
    let (read,write)=pipe_with(PipeFlags::CLOEXEC|PipeFlags::NONBLOCK)?;
    // Exec only the already verified executable descriptor. No command, unit,
    // options or connection originate in the browser or caller argv.
    let path=format!("/proc/thread-self/fd/{}",tool.file.as_raw_fd());
    let mut cmd=Command::new(path);
    cmd.args(["--system","--no-ask-password","--no-pager"]);
    let (cap,seconds)=match verb{
        Verb::Read(fields,cap,name)=>{cmd.args(["--all","--property",fields,"--","show",name.as_str()]);(cap,10)}
        Verb::Start=>{
            // No --wait (that waits for termination as well), no --no-block,
            // and never replace a conflicting existing manager job.
            cmd.args(["--quiet","--job-mode=fail","start",UNIT]);(0,60)
        }
    };
    check()?;manager()?;tool.recheck()?;
    let child=cmd.env_clear().env("LC_ALL","C").env("TZ","UTC").env("PATH","/usr/bin")
        .stdin(Stdio::null()).stdout(Stdio::from(File::from(write))).stderr(Stdio::null()).spawn()?;
    drop(cmd);
    let mut child=Running(Some(child));let deadline=Instant::now()+Duration::from_secs(seconds);
    let mut bytes=Vec::new();let mut eof=false;let mut done=false;
    loop{
        check()?;
        if Instant::now()>=deadline{return Err(no());}
        if !eof{
            let mut b=[0u8;512];
            match rustix::io::read(&read,&mut b[..]){
                Ok(0)=>eof=true,
                Ok(n) if bytes.len()+n<=cap=>bytes.extend_from_slice(&b[..n]),
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
pub(super) struct Manager(RuntimeFile);
impl Manager{
    pub(super) fn select()->io::Result<Self>{
        // Use only the protected native system tool, not PATH or a downloader.
        let tool=RuntimeFile::open(TOOL,true,16<<20)?;
        require_amd64_elf(&tool.file)?;Ok(Self(tool))
    }
    pub(super) fn read(&self,system:&Path,pool:&Id,uid:u32,gid:u32,check:impl Fn()->io::Result<()>)->io::Result<State>{
        let bytes=call(&self.0,Verb::Read(&web_systemd_state::selectors(),web_systemd_state::CAP,UnitName::Web),check)?;
        web_systemd_state::read(&bytes,system,pool,uid,gid)
    }
    pub(super) fn start(&self,check:impl Fn()->io::Result<()>)->io::Result<()>{
        // Even DefaultDependencies=no has the real system.slice dependency.
        // Require stable already-active slices, sysinit already reached,
        // and no shutdown. Do not activate a boot/maintenance dependency.
        for name in [UnitName::Sysinit,UnitName::SystemSlice,UnitName::RootSlice,UnitName::Shutdown]{
            const STATUS:&str="Id,LoadState,ActiveState,Job";
            const SLICE:&str="Id,LoadState,ActiveState,Job,Requires,Wants,Requisite,BindsTo,Upholds,Conflicts,ConflictedBy,OnFailure,OnSuccess,FailureAction,SuccessAction";
            let fields=if matches!(name,UnitName::SystemSlice|UnitName::RootSlice){SLICE}else{STATUS};
            stable_prerequisite(name,&call(&self.0,Verb::Read(fields,CAP,name),&check)?)?;
        }
        let output=call(&self.0,Verb::Start,check).map_err(|e|io::Error::new(e.kind(),
            "manager job result unconfirmed; the manager may already have accepted it; check status before another action"))?;
        if !output.is_empty(){return Err(no());}Ok(())
    }
}
fn stable_prerequisite(name:UnitName,data:&[u8])->io::Result<()>{
    let no=||bad("the fixed normal systemd boot prerequisites are not stable; no Web start was requested");
    let text=std::str::from_utf8(data).map_err(|_|no())?.strip_suffix('\n').ok_or_else(no)?;
    let mut rows=BTreeMap::new();
    for line in text.split('\n'){
        let (k,v)=line.split_once('=').ok_or_else(no)?;
        if rows.insert(k,v).is_some(){return Err(no());}
    }
    let active=if matches!(name,UnitName::Shutdown){"inactive"}else{"active"};
    for (key,value) in [("Id",name.as_str()),("LoadState","loaded"),("ActiveState",active),("Job","")]{
        if rows.remove(key)!=Some(value){return Err(no());}
    }
    if matches!(name,UnitName::SystemSlice|UnitName::RootSlice){
        let requires=if matches!(name,UnitName::SystemSlice){"-.slice"}else{""};
        if rows.remove("Requires")!=Some(requires)||!matches!(rows.remove("Conflicts"),Some(""|"shutdown.target")){
            return Err(no());
        }
        for k in ["Wants","Requisite","BindsTo","Upholds","ConflictedBy","OnFailure","OnSuccess"]{
            if rows.remove(k)!=Some(""){return Err(no());}
        }
        for k in ["FailureAction","SuccessAction"]{
            if rows.remove(k)!=Some("none"){return Err(no());}
        }
    }
    if !rows.is_empty(){return Err(no());}Ok(())
}
pub(super) fn require_unresolved(check:impl Fn()->io::Result<()>)->io::Result<()>{
    let manager=Manager::select()?;
    let fields=FIELDS.iter().map(|(k,_)|*k).collect::<Vec<_>>().join(",");
    unresolved(&call(&manager.0,Verb::Read(&fields,CAP,UnitName::Web),check)?)
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
