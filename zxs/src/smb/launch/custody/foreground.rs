//! One foreground's process-local stop signal. Installation/routing policy
//! is unrelated to these handlers. No process-global change in Web/Native.
use super::{Fault,Result};
use std::{mem::zeroed,sync::atomic::{AtomicBool,Ordering}};
static STOP:AtomicBool=AtomicBool::new(false);
extern "C" fn stop(_:libc::c_int){STOP.store(true,Ordering::Release);}
pub(super) fn stopping()->bool{STOP.load(Ordering::Acquire)}
pub(super) fn require()->Result<()>{if stopping(){Err(Fault::Handoff)}else{Ok(())}}
pub(super) fn install()->Result<()>{
    // SAFETY: this is the dedicated single-threaded high role only, never
    // the Web/default entry; the handler touches only one lock-free atomic.
    unsafe{
        let mut act:libc::sigaction=zeroed();
        act.sa_sigaction=stop as *const () as libc::sighandler_t;
        let mut mask:libc::sigset_t=zeroed();
        if libc::sigemptyset(&mut act.sa_mask)!=0||libc::sigprocmask(libc::SIG_SETMASK,std::ptr::null(),&raw mut mask)!=0{
            return Err(Fault::Handoff);
        }
        for sig in [libc::SIGINT,libc::SIGTERM,libc::SIGHUP,libc::SIGQUIT,libc::SIGTSTP]{
            if libc::sigismember(&mask,sig)!=0||libc::sigaction(sig,&act,std::ptr::null_mut())!=0{return Err(Fault::Handoff);}
        }
    }
    require()
}
