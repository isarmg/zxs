//! Owned descriptor snapshots and close/save ordering.
use super::{Context, Open, bad, context, sdk};

type Request = RawRequest<storage_contract::OperationId>;
use smb_protocol::{
    client::{Client, WriteSequence, close_descriptor},
    protocol::{OpenResult, Request as RawRequest},
};
use std::{
    os::fd::OwnedFd,
    sync::{Arc, Mutex},
};

pub(super) struct Snapshot {
    pub(super) client: Client,
    pub(super) info: Arc<OpenResult>,
    pub(super) fd: OwnedFd,
    pub(super) writable: bool,
    pub(super) io: Arc<Mutex<WriteSequence>>,
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
    Ok(Snapshot {
        client: context.client.clone(),
        info: opened.info.clone(),
        fd: opened.fd.try_clone()?,
        writable: opened.writable,
        io: opened.io.clone(),
    })
}
pub(super) fn close_opened(
    opened: Open,
    finish: impl FnOnce(Request) -> std::io::Result<()>,
) -> std::io::Result<i32> {
    close_descriptor(opened.info.handle, opened.fd, &opened.io, finish)?;
    Ok(0)
}

pub(super) fn finish_open(
    context: &mut Context,
    opened: Open,
    authorization: std::io::Result<()>,
) -> std::io::Result<i32> {
    match authorization {
        Ok(()) => close_opened(opened, |request| context.client.call(request).map(|_| ())),
        Err(authentication) => {
            let remote = opened.info.handle;
            // Detach releases ownership without publishing. Keep authentication
            // primary even if Detach, the I/O lock or local close also fails.
            let cleanup = close_opened(opened, |_| {
                context
                    .client
                    .call(Request::Detach { handle: remote })
                    .map(|_| ())
            });
            match cleanup {
                Ok(_) => Err(authentication),
                Err(cleanup) => Err(cleanup_error(Err(authentication), cleanup)),
            }
        }
    }
}

pub(super) fn cleanup_error(
    authorization: std::io::Result<()>,
    cleanup: std::io::Error,
) -> std::io::Error {
    match authorization {
        Ok(()) => cleanup,
        Err(authentication) => std::io::Error::new(
            authentication.kind(),
            DeniedCleanup {
                authentication,
                cleanup,
            },
        ),
    }
}

pub(super) fn error_errno(error: &std::io::Error) -> i32 {
    match error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<DeniedCleanup>())
    {
        Some(failure) => smb_protocol::client::error_errno(&failure.authentication),
        None => smb_protocol::client::error_errno(error),
    }
}

/// The authentication failure remains primary; cleanup is additional evidence.
#[derive(Debug)]
struct DeniedCleanup {
    authentication: std::io::Error,
    cleanup: std::io::Error,
}

impl std::fmt::Display for DeniedCleanup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}; cleanup also failed: {}",
            self.authentication, self.cleanup
        )
    }
}

impl std::error::Error for DeniedCleanup {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.authentication)
    }
}

#[cfg(test)]
#[path = "../../../../test/smb-vfs/unit/module/descriptor_cleanup_tests.rs"]
mod tests;
