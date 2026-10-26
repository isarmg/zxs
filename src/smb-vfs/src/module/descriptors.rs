//! Owned descriptor snapshots and close/save ordering.
use super::{Context, Open, authentication, bad, context, sdk};
use smb_protocol::{
    client::{Client, close_descriptor},
    protocol::{OpenResult, Request},
};
use std::{
    os::fd::{FromRawFd, OwnedFd},
    sync::{Arc, Mutex},
};

pub(super) struct Snapshot {
    pub(super) client: Client,
    pub(super) info: Arc<OpenResult>,
    pub(super) fd: OwnedFd,
    pub(super) writable: bool,
    pub(super) io: Arc<Mutex<()>>,
}
/// # Safety
/// handle obeys the module host contract and its Context is exclusively accessed
/// for this call. fsp is only used as an opaque lookup key, never dereferenced.
pub(super) unsafe fn snapshot(
    handle: *mut sdk::vfs_handle_struct,
    fsp: *mut sdk::files_struct,
) -> std::io::Result<Snapshot> {
    // SAFETY: The owning host callback supplies a live handle; Context access stays on this event-
    // loop thread and does not escape the callback.
    let context = unsafe { context(handle) }?;
    let opened = context.files.get(&(fsp as usize)).ok_or_else(bad)?;
    // SAFETY: Open owns a live descriptor; F_DUPFD_CLOEXEC returns a distinct owned descriptor or
    // an error, checked below.
    let descriptor = unsafe { libc::fcntl(opened.fd, libc::F_DUPFD_CLOEXEC, 0) };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Snapshot {
        client: context.client.clone(),
        info: opened.info.clone(),
        // SAFETY: fcntl succeeded above; this new descriptor has not been adopted by any other
        // owner.
        fd: unsafe { OwnedFd::from_raw_fd(descriptor) },
        writable: opened.writable,
        io: opened.io.clone(),
    })
}
pub(super) fn close_opened(
    opened: Open,
    finish: impl FnOnce(Request) -> std::io::Result<()>,
) -> std::io::Result<i32> {
    // SAFETY: removing Open from Context transfers its sole local descriptor
    // ownership here. Guard it before any fallible lock/save operation, because
    // it is no longer available to free_context on an early return or unwind.
    let descriptor = unsafe { OwnedFd::from_raw_fd(opened.fd) };
    close_descriptor(opened.info.handle, descriptor, &opened.io, finish)?;
    Ok(0)
}

pub(super) fn finish_open(
    context: &mut Context,
    opened: Open,
    authorized: bool,
) -> std::io::Result<i32> {
    let remote = opened.info.handle;
    close_opened(opened, |request| {
        if authorized {
            context.client.call(request).map(|_| ())
        } else {
            // Detach releases the handle without publishing after revocation.
            context.client.call(Request::Detach { handle: remote })?;
            Err(authentication::denied())
        }
    })
}
