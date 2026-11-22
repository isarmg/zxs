//! Open the original held name using one fixed read or pin recipe.
use std::{ffi::{CStr,c_char,c_int,c_long},fs::File,io,os::fd::{AsRawFd,FromRawFd,OwnedFd,RawFd}};

// Discriminants index the native held_read_recipes table.
#[repr(u8)]
pub(super) enum ProcOpen { Pin=1, Directory=2, Record=3 }
#[repr(u8)]
pub(super) enum ViewOpen { ProjectedPin=4, LocalPin=5 }
#[repr(u8)]
pub(super) enum SiteOpen { LocalPin=6, CachedCrossingPin=7, Directory=8 }

pub(super) enum Error { Native(io::Error), InvalidDescriptor }
pub(super) type Result=std::result::Result<OwnedFd,Error>;

unsafe extern "C"{
    fn open_held_read(parent:c_int,name:*const c_char,recipe:u8)->c_long;
}
fn open(parent:&File,name:&CStr,recipe:u8)->Result{
    // SAFETY: the held directory and C string stay live; only fixed read recipes reach this boundary.
    let opened=unsafe{open_held_read(parent.as_raw_fd(),name.as_ptr(),recipe)};
    if opened<0{return Err(Error::Native(io::Error::last_os_error()));}
    let fd=RawFd::try_from(opened).map_err(|_|Error::InvalidDescriptor)?;
    // SAFETY: successful openat2 returned this new descriptor, owned only here.
    Ok(unsafe{OwnedFd::from_raw_fd(fd)})
}
pub(super) fn proc(parent:&File,name:&CStr,recipe:ProcOpen)->Result{open(parent,name,recipe as u8)}
pub(super) fn view(parent:&File,name:&CStr,recipe:ViewOpen)->Result{open(parent,name,recipe as u8)}
pub(super) fn site(parent:&File,name:&CStr,recipe:SiteOpen)->Result{open(parent,name,recipe as u8)}
