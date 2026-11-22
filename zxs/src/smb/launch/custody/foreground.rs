//! One foreground's process-local stop signal. Installation/routing policy
//! is unrelated to these handlers. No process-global change in Web/Native.
use super::{Fault,Result};
use crate::smb::launch::signals;
use std::{ffi::c_int,sync::atomic::{AtomicBool,Ordering}};
static STOP:AtomicBool=AtomicBool::new(false);
extern "C" fn stop(_:c_int){STOP.store(true,Ordering::Release);}
pub(super) fn stopping()->bool{STOP.load(Ordering::Acquire)}
pub(super) fn require()->Result<()>{if stopping(){Err(Fault::Handoff)}else{Ok(())}}
pub(super) fn install()->Result<()>{
    // SAFETY: this is the dedicated single-threaded high role only, never
    // the Web/default entry; the handler touches only one lock-free atomic.
    if !unsafe{signals::foreground_stop(stop)}{return Err(Fault::Handoff);}
    require()
}
