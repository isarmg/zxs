//! Native representation for one original child birth and its exit sample.
use std::{ffi::{c_int,c_long},io,os::fd::{AsRawFd,BorrowedFd}};

#[repr(C)]
#[derive(Default)]
pub(super) struct ExitSample{
    pub(super) pid:i32,
    pub(super) signal:i32,
    pub(super) code:i32,
    pub(super) status:i32,
}
unsafe extern "C"{
    fn clone_child_pidfd(pidfd:*mut i32)->c_long;
    fn peek_child_exit(pidfd:c_int,out:*mut ExitSample)->c_int;
    fn reap_child_exit(pidfd:c_int,out:*mut ExitSample)->c_int;
}

/// # Safety
/// Only the prepared single-thread parent may call this. A zero return
/// must immediately enter the non-returning child setup.
pub(super) unsafe fn clone_child(pidfd:&mut i32)->c_long{
    // SAFETY: the caller owns this output slot; the fixed native clone uses
    // independent VM and descriptor tables, so the child has its own stack.
    unsafe{clone_child_pidfd(pidfd)}
}

#[derive(Clone,Copy)]
pub(super) enum ExitWait{Peek,Reap}
impl ExitWait{
    pub(super) fn sample(self,pidfd:BorrowedFd<'_>)->io::Result<ExitSample>{
        let mut out=ExitSample::default();
        // SAFETY: the original descriptor and plain output live through one
        // waitid. Only Reap consumes the original child exit.
        let rc=unsafe{match self{
            Self::Peek=>peek_child_exit(pidfd.as_raw_fd(),&raw mut out),
            Self::Reap=>reap_child_exit(pidfd.as_raw_fd(),&raw mut out),
        }};
        if rc!=0{return Err(io::Error::last_os_error());}
        Ok(out)
    }
}
