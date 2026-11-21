//! Actual reaped private workers for offline provisioning and interactive diagnostics.
use super::{bad,transport::Channel};
use crate::maintenance::thread_file;
use helper_linux_evidence::diagnostic_process::{OfflineSql,prepare_account_sql_exec,require_waitable_children};
use rustix::{io::Errno,process::{Pid,PidfdFlags,Signal,pidfd_open,pidfd_send_signal,waitid,WaitId,WaitIdOptions}};
use std::{fs::File,io::{self,Read},os::fd::AsFd,process::{Child,Command,Stdio},thread,time::{Duration,Instant}};

pub(super) struct Worker { child:Child, fd:File, pub(super) pid:Pid, done:bool }
impl Worker {
    pub(super) fn spawn(exe:&File,peer:&File,order:&File,mode:OfflineSql)->io::Result<Self>{
        require_waitable_children()?;
        let mut cmd=Command::new("/not-an-account-program"); // only pinned execveat in pre_exec
        cmd.env_clear();
        if matches!(mode,OfflineSql::FirstBrowser|OfflineSql::CopyRepairBrowser){
            let null=helper_linux_evidence::diagnostic_process::null_device()?;
            cmd.stdin(Stdio::from(null.try_clone()?)).stdout(Stdio::from(null.try_clone()?)).stderr(Stdio::from(null));
        }else if matches!(mode,OfflineSql::FirstLayout){cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());}
        prepare_account_sql_exec(&mut cmd,exe,peer,order,mode)?;
        let mut child=cmd.spawn()?;
        let actual=i32::try_from(child.id()).ok().and_then(Pid::from_raw);
        let Some(pid)=actual else{let _=child.kill();let _=child.wait();return Err(bad("invalid exact SQLite child PID"));};
        // This Child alone owns the still-waitable child PID; it cannot be
        // reused before wait. Hard failure here never transfers an FD.
        match pidfd_open(pid,PidfdFlags::empty()){
            Ok(fd)=>Ok(Self{child,fd:File::from(fd),pid,done:false}),
            Err(e)=>{let _=child.kill();let _=child.wait();Err(e.into())}
        }
    }
    fn peek(&self)->io::Result<Option<i32>>{
        loop{match waitid(WaitId::PidFd(self.fd.as_fd()),WaitIdOptions::EXITED|WaitIdOptions::NOHANG|WaitIdOptions::NOWAIT){
            Err(Errno::INTR)=>continue,Ok(s)=>return Ok(s.map(|s|s.exit_status().unwrap_or(-1))),Err(e)=>return Err(e.into()),
        }}
    }
    pub(super) fn live(&self)->io::Result<()>{
        if !self.done&&self.peek()?.is_none(){Ok(())}else{Err(bad("the original SQLite worker ended before the next step"))}
    }
    pub(super) fn finish(&mut self,channel:&Channel,until:Instant)->io::Result<()>{
        loop {
            if Instant::now()>=until{return Err(bad("original credential worker completion timed out"));}
            if let Some(status)=self.peek()?{
                let read=self.child.wait()?;
                self.done=true;
                // A clean result and real independent exit are both
                // required; empty or additional control packets are errors.
                return if status==0&&read.code()==Some(0){channel.eof()}else{Err(bad("original credential worker did not exit normally"))};
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Worker {
    pub(super) fn cancel(&mut self){
        if self.done{return;}
        // Restore Web's tty guard via TERM before considering force. If the
        // kernel is stalled even a kill-and-wait can be delayed. Forced exit
        // never supplies success, and this process keeps the two originals.
        let _=pidfd_send_signal(&self.fd,Signal::TERM);
        let end=Instant::now()+Duration::from_secs(3);
        while Instant::now()<end&&self.peek().ok().flatten().is_none(){thread::sleep(Duration::from_millis(20));}
        if self.peek().ok().flatten().is_none(){let _=pidfd_send_signal(&self.fd,Signal::KILL);}
        loop{match self.child.wait(){
            Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,
            Ok(_)=>{self.done=true;break},Err(_)=>break,
        }}
    }
}
impl Drop for Worker { fn drop(&mut self){self.cancel();} }
fn current_status()->io::Result<String>{
    let mut s=String::new();thread_file(c"thread-self/status")?.take(65537).read_to_string(&mut s)?;
    if s.len()>65536||!s.ends_with('\n'){return Err(bad("invalid current credential process status"));}
    Ok(s)
}
pub(super) fn prerequisite()->io::Result<()>{
    let s=current_status()?;
    let mut e=s.lines().filter_map(|l|l.strip_prefix("CapEff:"));
    let cap=e.next().and_then(|v|u64::from_str_radix(v.trim_ascii(),16).ok());
    // DAC override/read plus root process-control rights; inadequate signal
    // authority cannot start a worker it would be unable to reap on failure.
    if e.next().is_some()||!cap.is_some_and(|v|v&(1<<5)!=0&&v&6!=0){
        return Err(bad("the site cannot verify or collect this original credential process"));
    }Ok(())
}
