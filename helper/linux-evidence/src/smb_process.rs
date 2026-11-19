//! Stable Linux 6.12 evidence used by the dedicated SMB launcher. These two
//! calls inspect only qualified live kernel descriptors; no socket options,
//! process credentials or namespace membership are changed here.
use std::{fs::File, io, os::fd::{AsRawFd, FromRawFd}};

/// Linux UAPI SO_PASSPIDFD (76) is not exposed by the pinned rustix sockopt API.
#[allow(unsafe_code)]
pub fn initially_no_sender_pidfd(socket: &File) -> io::Result<()> {
    let mut value: libc::c_int = -1;
    let mut len = std::mem::size_of_val(&value) as libc::socklen_t;
    // SAFETY: the fixed option returns one int; both initialized outputs live through the call.
    if unsafe { libc::getsockopt(socket.as_raw_fd(), libc::SOL_SOCKET, 76,
        (&raw mut value).cast(), &raw mut len) } != 0 { return Err(io::Error::last_os_error()); }
    if len as usize != std::mem::size_of_val(&value) || value != 0 {
        return Err(io::Error::other("not an untouched launch channel"));
    }
    Ok(())
}

/// Linux NS_GET_USERNS returns a new CLOEXEC descriptor for the namespace that
/// owns the supplied PID namespace. The caller must not infer this from two names.
#[allow(unsafe_code)]
pub fn pid_namespace_owner(ns: &File) -> io::Result<File> {
    if rustix::fs::fstatfs(ns)?.f_type as u64 != 0x6e736673 {
        return Err(io::Error::other("not a real namespace descriptor"));
    }
    // SAFETY: Linux nsfs accepts exactly these no-argument _IO requests. It either
    // fails or supplies a new owned fd for GET_USERNS; never a borrowed reference.
    unsafe {
        if libc::ioctl(ns.as_raw_fd(), 0xb703, 0 as libc::c_ulong) != libc::CLONE_NEWPID {
            return Err(io::Error::other("not the original PID namespace"));
        }
        let fd = libc::ioctl(ns.as_raw_fd(), 0xb701, 0 as libc::c_ulong);
        if fd < 0 { return Err(io::Error::last_os_error()); }
        let owner = File::from_raw_fd(fd);
        if rustix::fs::fstatfs(&owner)?.f_type as u64 != 0x6e736673 ||
            libc::ioctl(owner.as_raw_fd(), 0xb703, 0 as libc::c_ulong) != libc::CLONE_NEWUSER {
            return Err(io::Error::other("PID namespace owner is unavailable"));
        }
        Ok(owner)
    }
}

/// Return just the active PID namespace, PID namespace for future children and
/// current user namespace of THIS process pidfd. The codes are Linux 6.12 pidfd.h.
#[allow(unsafe_code)]
pub fn process_namespaces(pidfd: &File) -> io::Result<[File; 3]> {
    if rustix::fs::fstatfs(pidfd)?.f_type as u64 != 0x50494446 {
        return Err(io::Error::other("not a pidfs process descriptor"));
    }
    let fixed = |request: libc::c_ulong| -> io::Result<File> {
        // SAFETY: all three call sites are fixed _IO requests with zero argument.
        // A successful call gives a new owned CLOEXEC nsfs file.
        let fd = unsafe { libc::ioctl(pidfd.as_raw_fd(), request, 0 as libc::c_ulong) };
        if fd < 0 { return Err(io::Error::last_os_error()); }
        let f = unsafe { File::from_raw_fd(fd) };
        if rustix::fs::fstatfs(&f)?.f_type as u64 != 0x6e736673 {
            return Err(io::Error::other("cannot identify the original process namespace"));
        }
        Ok(f)
    };
    Ok([fixed(0xff05)?, fixed(0xff06)?, fixed(0xff09)?])
}
