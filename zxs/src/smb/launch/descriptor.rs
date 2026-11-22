//! Read-only Linux descriptor facts. A successful read grants no ownership.
use super::{Fault,Result};
use std::{ffi::c_int,os::fd::RawFd};

pub(super) struct DescriptorFlags(c_int);
impl DescriptorFlags{
    pub(super) const fn close_on_exec(self)->bool{self.0&libc::FD_CLOEXEC!=0}
    /// Inheritance must carry every native bit into its existing F_SETFD.
    pub(super) const fn native_bits(self)->c_int{self.0}
}

#[derive(Clone,Copy,PartialEq,Eq)]
pub(super) enum Access {ReadOnly,WriteOnly,ReadWrite}
pub(super) struct OpenStatus(c_int);
impl OpenStatus{
    // This is the fixed observe_open_status encoding, not Linux flag values.
    pub(super) const fn access(&self)->Option<Access>{
        match self.0&3{
            1=>Some(Access::ReadOnly),2=>Some(Access::WriteOnly),3=>Some(Access::ReadWrite),_=>None,
        }
    }
    pub(super) const fn nonblocking(&self)->bool{self.0&4!=0}
    pub(super) const fn append(&self)->bool{self.0&8!=0}
}

unsafe extern "C"{
    fn observe_descriptor_flags(fd:c_int)->c_int;
    fn observe_open_status(fd:c_int)->c_int;
    fn observe_file_seals(fd:c_int)->c_int;
}
fn observed(raw:c_int)->Result<c_int>{if raw<0{Err(Fault::Handoff)}else{Ok(raw)}}
pub(super) fn flags(fd:RawFd)->Result<DescriptorFlags>{
    // SAFETY: fixed read-only command; the kernel checks the descriptor number.
    observed(unsafe{observe_descriptor_flags(fd)}).map(DescriptorFlags)
}
pub(super) fn status(fd:RawFd)->Result<OpenStatus>{
    // SAFETY: fixed read-only command; the kernel checks the descriptor number.
    observed(unsafe{observe_open_status(fd)}).map(OpenStatus)
}
pub(super) fn cloexec(fd:RawFd)->Result<()>{
    if flags(fd)?.close_on_exec(){Ok(())}else{Err(Fault::Handoff)}
}
pub(super) fn seal_bits(fd:RawFd)->Result<c_int>{
    // SAFETY: fixed read-only command. Keep all bits for the caller's exact or subset match.
    observed(unsafe{observe_file_seals(fd)})
}
