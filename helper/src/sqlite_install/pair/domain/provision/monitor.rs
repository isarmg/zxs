//! Private bwrap v0.12 status FD, distinct from the guest's one-shot SCM packet.
//! --as-pid-1 is essential: without it child-pid can identify a separate init.
use super::{Finished,cancelled};
use crate::{isolated_group::Group,sqlite_install::bad};
use rustix::{fd::AsFd,fs::{OFlags,fcntl_getfl,fcntl_setfl},process::Pid};
use std::{fs::File,io::{self,Read},thread,time::{Duration,Instant}};

fn positive(s:&str)->Option<u64>{
    if s.starts_with('0')||s.is_empty()||!s.bytes().all(|b|b.is_ascii_digit()){return None;}s.parse().ok()
}
fn child(status:&[u8])->Option<Pid>{
    let s=std::str::from_utf8(status).ok()?.strip_suffix("\n{ \"exit-code\": 0 }\n")?;
    let fields=s.strip_prefix("{ \"child-pid\": ")?.strip_suffix(" }")?;
    let mut parts=fields.split(", ");let pid=i32::try_from(positive(parts.next()?)?).ok()?;
    let mut remaining=["ipc","mnt","net","pid","uts"].as_slice();
    for p in parts{
        let (name,num)=p.strip_prefix('"')?.split_once("-namespace\": ")?;
        positive(num)?;let at=remaining.iter().position(|&k|k==name)?;
        remaining=&remaining[at+1..];
    }
    Pid::from_raw(pid)
}
fn nonblock(fd:&impl AsFd)->io::Result<()> {fcntl_setfl(fd,fcntl_getfl(fd)?|OFlags::NONBLOCK)?;Ok(())}
fn observe(group:&mut Group,mut status:File)->io::Result<Pid>{
    let mut err=group.child.stderr.take().ok_or_else(||bad("missing isolated launch error channel"))?;
    nonblock(&err)?;nonblock(&status)?;
    let start=Instant::now();let mut bytes=Vec::new();let mut buf=[0;512];
    loop{
        cancelled()?;
        if start.elapsed()>=Duration::from_secs(35){return Err(io::Error::new(io::ErrorKind::TimedOut,"dedicated Samba provision timed out"));}
        let eof=match status.read(&mut buf){
            Ok(0)=>true,Ok(n) if bytes.len()+n<=4096=>{bytes.extend_from_slice(&buf[..n]);false},
            Ok(_)=>return Err(bad("unexpected launcher status size")),
            Err(e) if matches!(e.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::Interrupted)=>false,Err(e)=>return Err(e),
        };
        // Never forward native errors from private Samba files. A non-quiet
        // or incomplete provisioning run cannot issue durable evidence.
        let quiet=match err.read(&mut buf){Ok(0)=>true,Ok(_)=>return Err(bad("isolated domain provision reported diagnostics; preserve its state")),
            Err(e) if matches!(e.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::Interrupted)=>false,Err(e)=>return Err(e)};
        match group.peek_exit()?{
            Some(0) if eof&&quiet=>return child(&bytes).ok_or_else(||bad("normal payload exec, identity and status not established")),
            Some(code) if code!=0=>return Err(bad("dedicated Samba provision failed; do not start a replacement")),
            _=>{},
        }
        thread::sleep(Duration::from_millis(5));
    }
}
pub(super) fn reap(mut group:Group,status:File)->io::Result<Finished>{
    let result=observe(&mut group,status);
    // Even a timeout, invalid report or signal must collect the original
    // process group before we allow any readback, synchronization or result.
    let reaped=group.finish()?;let pid=result?;
    if reaped.leader_code!=Some(0){return Err(bad("abnormal or indeterminate launcher exit"));}
    cancelled()?;Ok(Finished(pid))
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn only_one_canonical_host_payload_and_one_normal_final_status(){
        for a in ["{ \"child-pid\": 45 }","{ \"child-pid\": 45, \"ipc-namespace\": 91, \"pid-namespace\": 52 }"]{
            assert_eq!(child(format!("{a}\n{{ \"exit-code\": 0 }}\n").as_bytes()).unwrap().as_raw_nonzero().get(),45);}
        let good="{ \"child-pid\": 45, \"mnt-namespace\": 83, \"pid-namespace\": 52 }\n{ \"exit-code\": 0 }\n";
        for wrong in [good.replace("45","0"),good.replace("45","045"),good.replace("45","2147483648"),
            good.replace("pid-namespace","mnt-namespace"),good.replace("mnt-namespace","uts-namespace"),
            good.replace("pid-namespace","wire-pid"),good.replace("52","-1"),good.replace("exit-code\": 0","exit-code\": 1"),
            good.replace("\n{ \"exit-code\": 0 }",""),good.to_owned()+"\n",good.to_owned()+good]{
            assert!(child(wrong.as_bytes()).is_none());
        }
    }
}
