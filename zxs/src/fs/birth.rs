//! Read-only, filesystem-scoped incarnation proof for reopening after close.
//! The kernel's opaque object handle is compared, never used to open by handle.
use super::{File,invalid};
use std::{io,os::fd::AsRawFd};
pub(crate) const MAX_BIRTH:usize=16+4+128;
unsafe extern "C" { fn zxs_native_birth(fd:std::os::raw::c_int,out:*mut u8,cap:usize)->isize; }

#[derive(Clone,Debug,Eq,PartialEq)]
pub(crate) struct Birth(Vec<u8>);
impl Birth {
    pub(crate) fn read(file:&File)->io::Result<Self>{
        let mut buf=[0u8;MAX_BIRTH];
        // SAFETY: output covers cap writable bytes; input FD remains held.
        let n=unsafe{zxs_native_birth(file.as_raw_fd(),buf.as_mut_ptr(),buf.len())};
        if n<0{return Err(io::Error::last_os_error());}
        Self::decode(buf.get(..n as usize).ok_or_else(||invalid("native handle exceeds bound"))?)
    }
    pub(crate) fn decode(bytes:&[u8])->io::Result<Self>{
        if !(21..=MAX_BIRTH).contains(&bytes.len()) || bytes[..16].iter().all(|b|*b==0){
            return Err(invalid("missing or unsupported filesystem UUID or original object handle"));
        }
        Ok(Self(bytes.to_vec()))
    }
    pub(crate) fn bytes(&self)->&[u8]{&self.0}
    pub(crate) fn filesystem(&self)->&[u8]{&self.0[..16]}
    pub(crate) fn require(&self,observed:&Self)->io::Result<()>{
        if self!=observed {Err(invalid("filesystem or original object incarnation differs"))}else{Ok(())}
    }
}
