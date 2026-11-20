//! A one-use, kernel-attested post-bind receipt, separate from storage control.
//! Only the v9 supervisor may use it with its original two Born objects.
//! Receiving bytes is not permission, a login, or evidence of business I/O.
use crate::{diagnostic_process, smb_spawn::Born};
use std::{fs::File, io, mem, os::{fd::{AsRawFd, FromRawFd, RawFd}, unix::fs::MetadataExt},
    ptr, thread, time::{Duration, Instant}};

// Linux 6.12 x86_64 LP64: include/{linux/socket.h,uapi/asm-generic/socket.h}.
// Keep the normal storage control on a separate socket with its own options.
const SO_PASSPIDFD: libc::c_int = 76;
const SCM_PIDFD: libc::c_int = 4;
const PIDFS_MAGIC: u64 = 0x5049_4446;
const CONTROL_MAX: usize = 128;
const CMSG_HEAD: usize = 16;
const CMSG_ALIGN: usize = 8;
const _: () = {
    assert!(mem::size_of::<libc::cmsghdr>() == CMSG_HEAD);
    assert!(mem::align_of::<libc::cmsghdr>() == CMSG_ALIGN);
    assert!(mem::size_of::<libc::c_int>() == 4);
};
const MAGIC: &[u8; 16] = b"ZXS-SMB/LISTEN1\0";
pub const FRAME_BYTES: usize = 176;

fn wrong() -> io::Error { io::Error::other("no exact live receipt from the original dedicated SMB listener") }

pub fn expected(bound: &[u8; 128], plan: &[u8; 32]) -> [u8; FRAME_BYTES] {
    let mut out = [0; FRAME_BYTES];
    out[..16].copy_from_slice(MAGIC);
    out[16..144].copy_from_slice(bound);
    out[144..].copy_from_slice(plan);
    out
}

/// A receiving endpoint generated before either child. Not a generic
/// receiver for paths, arbitrary sockets or unknown programs.
pub struct Receipt(File);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attachment { Pid, Rights }

/// Accept only the native kernel's completed control layout. Retain a ledger
/// of all delivered FD integers even for a rejected packet, so the caller
/// owns/closes both unexpected rights and the genuine ancillary PID fd.
fn scan_control(mut control: &[u8]) -> (bool, Vec<(Attachment, RawFd)>) {
    let mut valid = true;
    let mut received = Vec::new();
    while !control.is_empty() {
        if control.len() < CMSG_HEAD { return (false, received); }
        let len = usize::from_ne_bytes(control[..8].try_into().expect("8 bytes"));
        let level = i32::from_ne_bytes(control[8..12].try_into().expect("4 bytes"));
        let kind = i32::from_ne_bytes(control[12..16].try_into().expect("4 bytes"));
        if !(CMSG_HEAD..=control.len()).contains(&len) { return (false, received); }
        let data = &control[CMSG_HEAD..len];
        if level == libc::SOL_SOCKET && matches!(kind, SCM_PIDFD | libc::SCM_RIGHTS) {
            let role = if kind == SCM_PIDFD { Attachment::Pid } else { Attachment::Rights };
            valid &= role == Attachment::Pid && data.len() == 4;
            for word in data.chunks_exact(4) {
                let fd = i32::from_ne_bytes(word.try_into().expect("4 bytes"));
                // Linux may put a negative pidfd_prepare error in SCM_PIDFD.
                if fd >= 0 { received.push((role, fd)); } else { valid = false; }
            }
        } else { valid = false; }
        let stride = (len + CMSG_ALIGN - 1) & !(CMSG_ALIGN - 1);
        if stride > control.len() && len != control.len() { return (false, received); }
        control = &control[stride.min(control.len())..];
    }
    (valid, received)
}

#[repr(align(8))]
struct Control([u8; CONTROL_MAX]);
enum Packet { Waiting, End, Frame([u8; FRAME_BYTES], File) }

impl Receipt {
    /// The parent must drop its copy of the second endpoint immediately
    /// after giving it to the original smbd. No other child gets that end.
    #[allow(unsafe_code)]
    pub fn fresh() -> io::Result<(Self, File)> {
        let mut f = [-1; 2];
        // SAFETY: the fixed socketpair call writes two new owned descriptors.
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, f.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: this is the only adoption of these fresh descriptor numbers.
        let [rx, tx] = f.map(|fd| unsafe { File::from_raw_fd(fd) });
        let on = 1i32;
        let mut value = 0i32;
        let mut len = mem::size_of::<i32>() as libc::socklen_t;
        // SAFETY: both option calls operate on this new local receive end only,
        // and use the initialized exact int/length slots.
        if unsafe { libc::setsockopt(rx.as_raw_fd(), libc::SOL_SOCKET, SO_PASSPIDFD, (&raw const on).cast(), len) } != 0
            || unsafe { libc::getsockopt(rx.as_raw_fd(), libc::SOL_SOCKET, SO_PASSPIDFD, (&raw mut value).cast(), &raw mut len) } != 0
        { return Err(io::Error::last_os_error()); }
        if value != on || len as usize != mem::size_of::<i32>() { return Err(wrong()); }
        Ok((Self(rx), tx))
    }

    #[allow(unsafe_code)]
    fn packet(&self) -> io::Result<Packet> {
        let mut frame = [0; FRAME_BYTES];
        let mut iov = libc::iovec { iov_base: frame.as_mut_ptr().cast(), iov_len: frame.len() };
        let mut buffer = Control([0; CONTROL_MAX]);
        let mut msg = libc::msghdr {
            msg_name: ptr::null_mut(), msg_namelen: 0, msg_iov: &raw mut iov, msg_iovlen: 1,
            msg_control: buffer.0.as_mut_ptr().cast(), msg_controllen: CONTROL_MAX, msg_flags: 0,
        };
        // SAFETY: message buffers are live, fully initialized and bounded;
        // ancillary files installed by the kernel are taken into RAII below.
        let n = unsafe { libc::recvmsg(self.0.as_raw_fd(), &raw mut msg, libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC) };
        if n < 0 {
            let e = io::Error::last_os_error();
            return if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) {
                Ok(Packet::Waiting)
            } else { Err(e) };
        }
        let (valid, raw) = scan_control(&buffer.0[..msg.msg_controllen.min(CONTROL_MAX)]);
        // SAFETY: these integers came only from SCM_PIDFD and SCM_RIGHTS in
        // this successful kernel recvmsg. Each installed fd is uniquely owned.
        let mut files: Vec<_> = raw.into_iter().map(|(role, fd)| (role, unsafe { File::from_raw_fd(fd) })).collect();
        let allowed = libc::MSG_CMSG_CLOEXEC | libc::MSG_EOR;
        if !valid || msg.msg_controllen > CONTROL_MAX || msg.msg_flags & !allowed != 0 { return Err(wrong()); }
        // With SO_PASSPIDFD set before any send, a zero-length data record
        // still carries its sender (or failure/truncation), unlike real EOF.
        if n == 0 && msg.msg_controllen == 0 && msg.msg_flags & libc::MSG_EOR == 0 { return Ok(Packet::End); }
        if n as usize != FRAME_BYTES || files.len() != 1 { return Err(wrong()); }
        let (role, sender) = files.pop().expect("single checked descriptor");
        if role != Attachment::Pid { return Err(wrong()); }
        Ok(Packet::Frame(frame, sender))
    }

    /// Original Borns come only from the fixed supervisor's clone3. This
    /// also relies on that launcher confining all fd4 holders to the trusted
    /// master/fork family with no CAP_SYS_ADMIN or alternate user namespace:
    /// a privileged SCM_CREDENTIALS sender must not be able to choose a PID.
    pub fn wait(self, want: &[u8; FRAME_BYTES], daemon: &Born, storage: &Born, deadline: Instant) -> io::Result<()> {
        let live = || -> io::Result<()> {
            if diagnostic_process::cancelled() || Instant::now() >= deadline
                || daemon.peek()?.is_some() || storage.peek()?.is_some() { return Err(wrong()); }
            Ok(())
        };
        let mut first = None;
        loop {
            live()?;
            match self.packet()? {
                Packet::Waiting => thread::sleep(Duration::from_millis(5)),
                Packet::Frame(frame, sender) if first.is_none() && frame == *want => {
                    original(daemon.original(), &sender)?;
                    first = Some(sender);
                }
                Packet::End => {
                    original(daemon.original(), first.as_ref().ok_or_else(wrong)?)?;
                    return live(); // EOF is the end of this write channel, not a process exit.
                }
                Packet::Frame(..) => return Err(wrong()),
            }
        }
    }
}

#[allow(unsafe_code)]
fn identity(pidfd: &File) -> io::Result<(u64, u64)> {
    if rustix::fs::fstatfs(pidfd)?.f_type as u64 != PIDFS_MAGIC { return Err(wrong()); }
    let mut p = libc::pollfd { fd: pidfd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: the original owned PID fd is alive as a descriptor and the
    // fixed one-element pollfd buffer is writable. Do not wait here.
    if unsafe { libc::poll(&raw mut p, 1, 0) } != 0 { return Err(wrong()); }
    let m = pidfd.metadata()?;
    if m.ino() == 0 { return Err(wrong()); }
    Ok((m.dev(), m.ino()))
}
fn original(daemon: &File, sender: &File) -> io::Result<()> {
    let sent = identity(sender)?;
    if sent != identity(daemon)? || identity(sender)? != sent { return Err(wrong()); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    // Synthetic control bytes: never call FromRawFd on these fixture values.
    fn cmsg(role: i32, body: &[i32]) -> Vec<u8> {
        let n = CMSG_HEAD + body.len() * 4;
        let mut m = n.to_ne_bytes().to_vec();
        m.extend(libc::SOL_SOCKET.to_ne_bytes());
        m.extend(role.to_ne_bytes());
        for fd in body { m.extend(fd.to_ne_bytes()); }
        m.resize((n + CMSG_ALIGN - 1) & !(CMSG_ALIGN - 1), 0);
        m
    }
    #[test]
    fn the_exact_once_receipt_has_no_text_pid_or_extra_tail() {
        let frame = expected(&[0x71; 128], &[0x53; 32]);
        assert_eq!(&frame[..16], b"ZXS-SMB/LISTEN1\0");
        assert_eq!(&frame[16..144], &[0x71; 128]);
        assert_eq!(&frame[144..], &[0x53; 32]);
        assert_eq!(frame.len(), 176);
        assert_eq!(scan_control(&cmsg(SCM_PIDFD, &[71])), (true, vec![(Attachment::Pid, 71)]));
        assert_eq!(scan_control(&cmsg(SCM_PIDFD, &[-libc::EMFILE])), (false, Vec::new()));
        let attachments = [cmsg(libc::SCM_RIGHTS, &[27, 28]), cmsg(SCM_PIDFD, &[71])].concat();
        assert_eq!(scan_control(&attachments), (false,
            vec![(Attachment::Rights, 27), (Attachment::Rights, 28), (Attachment::Pid, 71)]));
        assert!(!scan_control(&cmsg(SCM_PIDFD, &[27, 28])).0);
        assert!(!scan_control(&[0; CMSG_HEAD - 1]).0);
    }
}
