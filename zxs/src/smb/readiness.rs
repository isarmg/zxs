//! Observe readiness of already owned descriptors. This conveys no I/O or authority.
use std::{ffi::c_int,io,marker::PhantomData,os::fd::{AsRawFd,BorrowedFd}};

// Private bits shared with wait_descriptor_readiness, not native poll values.
const READ:u8=1;
const WRITE:u8=2;
const PEER_END:u8=4;
const ERROR:u8=8;
const HANG:u8=16;
const INVALID:u8=32;

#[derive(Clone,Copy)]
#[repr(u8)]
pub(super) enum Interest { Read=READ, Write=WRITE, PeerEnd=PEER_END, Input=READ|PEER_END }

#[derive(Clone,Copy)]
pub(super) struct Events(u8);
impl Events{
    pub(super) const fn any(self)->bool{self.0!=0} // includes unfamiliar native bits
    pub(super) const fn readable(self)->bool{self.0&READ!=0}
    pub(super) const fn invalid_or_error(self)->bool{self.0&(INVALID|ERROR)!=0}
    pub(super) const fn peer_end(self)->bool{self.0&(PEER_END|HANG)!=0}
    pub(super) const fn broken(self)->bool{self.0&(INVALID|ERROR|HANG)!=0}
}

#[repr(C)]
struct Slot{fd:c_int,interest:u8,observed:u8}
#[repr(transparent)]
pub(super) struct Watch<'fd>{slot:Slot,_hold:PhantomData<BorrowedFd<'fd>>}
impl<'fd> Watch<'fd>{
    pub(super) fn new(fd:BorrowedFd<'fd>,interest:Interest)->Self{
        Self{slot:Slot{fd:fd.as_raw_fd(),interest:interest as u8,observed:0},_hold:PhantomData}
    }
    pub(super) const fn vacant()->Self{
        Self{slot:Slot{fd:-1,interest:0,observed:0},_hold:PhantomData}
    }
    pub(super) const fn events(&self)->Events{Events(self.slot.observed)}
}

unsafe extern "C"{
    fn wait_descriptor_readiness(watches:*mut Slot,count:usize,ms:c_int)->c_int;
}
fn observe(watches:&mut [Watch<'_>],ms:c_int)->c_int{
    // SAFETY: Watch is transparent over Slot. Live descriptor borrows and
    // writable slots cover the call; no pointer or FD is retained by C.
    unsafe{wait_descriptor_readiness(watches.as_mut_ptr().cast::<Slot>(),watches.len(),ms)}
}

/// The observed events belong to this call, including a failed wait.
pub(super) fn wait(watches:&mut [Watch<'_>],ms:c_int)->io::Result<usize>{
    usize::try_from(observe(watches,ms)).map_err(|_|io::Error::last_os_error())
}
/// One zero wait for a retained pidfd, including the allocation-free child.
pub(super) fn quiet_now(fd:BorrowedFd<'_>)->bool{
    observe(&mut [Watch::new(fd,Interest::Read)],0)==0
}

#[cfg(test)]
#[path="../../tests/unit/attached/smb/readiness/fixtures.rs"]
mod fixtures;
