//! One internal start and one teardown. The only two children are the selected
//! ELFs, not a shell, service-manager wrapper, namespace proxy or looked-up PID.
//! Package/site policy for authorizing the actual private deployment is upstream
//! of Inputs. No automatic start, Web/CLI command, self-approval or retry.
use super::{Control,no,origins::{Inputs,RuntimeLease,Selected},process};
use helper_linux_evidence::{diagnostic_process as signals,smb_spawn::{self,Born}};
use std::{io,thread,time::{Duration,Instant}};

/// Distinguish a verified stop of the two originals from a claim about FILEs
/// or graceful exit of each Samba descendant. An ns init's ordinary exit may
/// still cause Linux to kill remaining members. Only report the former.
pub(super) struct OriginalRolesExited;
pub(super) struct Pair<'a>{site:Selected<'a>,daemon:Born,storage:Born,_lease:RuntimeLease}
fn cancelled()->io::Result<()>{if signals::cancelled(){Err(io::Error::new(io::ErrorKind::Interrupted,"dedicated service stop requested"))}else{Ok(())}}
impl<'a> Pair<'a>{
    pub(super) fn start(inputs:Inputs<'a>)->io::Result<Self>{
        process::host()?;signals::require_waitable_children()?;signals::watch_cancellation()?;
        let site=Selected::new(inputs)?;cancelled()?;
        let lease=RuntimeLease::acquire(site.inputs.system,site.inputs.site)?;site.require()?;lease.require(&site.inputs)?;
        let control=Control::fresh()?;let end=Instant::now()+Duration::from_secs(9);
        let mut daemon=smb_spawn::daemon(&site.daemon.0.file,site.inputs.daemon_root,control.first_endpoint()?)?;
        daemon.exec_barrier(end)?;
        // smbd can be in F_DUPFD_CLOEXEC/close(3) during the bounded scan.
        // Retry only this same living child/original, never respawn or wait on READY.
        loop{
            cancelled()?;process::waitable_live(daemon.original())?;
            if process::qualify(daemon.original(),&site.daemon.0.file,&control.daemon,&control.storage).is_ok(){break;}
            if Instant::now()>=end{return Err(no());}thread::sleep(Duration::from_millis(5));
        }
        site.require()?;cancelled()?;
        let handoff=control.for_storage(daemon.original(),&site.daemon)?;
        let (opposite,_original_master)=handoff.original_3_5()?;
        let storage=smb_spawn::storage(&site.storage.0.file,site.inputs.storage_root,opposite,site.inputs.bound_readonly,&daemon,&site.env)?;
        drop(handoff); // supervisor keeps NO extra endpoint
        let mut pair=Self{site,daemon,storage,_lease:lease};
        let checked=(||{
            pair.storage.exec_barrier(end)?;let v=&pair.site;
            process::storage_snapshot(pair.storage.original(),&v.storage.0.file,v.inputs.uid,v.inputs.gid)?;
            process::waitable_live(pair.daemon.original())?;v.require()?;cancelled()
        })();
        if let Err(e)=checked{pair.abort()?;return Err(e);}
        Ok(pair) // two observed execs, not a READY acknowledgement or a login
    }
    fn abort(&mut self)->io::Result<()>{
        // Signal BOTH anchored namespace inits before either blocking reap.
        let k1=self.storage.kill_original();let k2=self.daemon.kill_original();
        let r1=self.storage.abort();let r2=self.daemon.abort();k1.and(k2).and(r1).and(r2)
    }
    fn stop_inner(&mut self,end:Instant)->io::Result<OriginalRolesExited>{
        let mut requested=false;
        loop{
            let s=self.storage.peek()?;let d=self.daemon.peek()?;
            if !requested{
                // A namespace PID1 can ignore TERM until its handler is installed.
                if s.is_some()||d.is_some(){return Err(no());}
                let a=&self.site;
                if process::storage_snapshot(self.storage.original(),&a.storage.0.file,a.inputs.uid,a.inputs.gid)?{
                    self.storage.term()?;requested=true;
                }
            }else{
                if s.is_some_and(|n|n!=0)||d.is_some_and(|n|n!=0){return Err(no());}
                if s==Some(0)&&d==Some(0){
                    let sc=self.storage.finish()?;let dc=self.daemon.finish()?;
                    if sc==0&&dc==0{return Ok(OriginalRolesExited);}return Err(no());
                }
            }
            if Instant::now()>=end{return Err(io::Error::new(io::ErrorKind::TimedOut,"normal stop of the original service pair unconfirmed"));}
            thread::sleep(Duration::from_millis(20));
        }
    }
    pub(super) fn stop(mut self)->io::Result<OriginalRolesExited>{
        match self.stop_inner(Instant::now()+Duration::from_secs(30)){
            Ok(v)=>Ok(v),Err(e)=>{self.abort()?;Err(e)}
        }
    }
    /// This is only for a dedicated foreground launcher (never a Web process).
    /// Unexpected departure cannot become a normal stop or auto-restart.
    pub(super) fn supervise(mut self)->io::Result<OriginalRolesExited>{
        loop{
            if signals::cancelled(){return self.stop();}
            if self.daemon.peek()?.is_some()||self.storage.peek()?.is_some(){self.abort()?;return Err(no());}
            thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for Pair<'_>{
    fn drop(&mut self){let _=self.abort();}
}
