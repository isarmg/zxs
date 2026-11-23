//! One private existing Unix listener; no TCP or TLS listener, bind, or legacy roles.
use crate::{
    container_store::Store,
    fs::{HeldDir, local_uid},
    headless_http::{Incoming, Outcome, serve_one},
    headless_start::Bound,
    workspace_protocol::Request,
};
use std::{
    io,
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixListener,
    },
    path::Path,
    thread,
    time::Duration,
};

pub(crate) const SOCKET: &str = "/zxs/run/headless.sock";
const PARENT: &str = "/zxs/run";
const LEAF: &std::ffi::CStr = c"headless.sock";
const WORKERS: usize = 32;
const POLL: Duration = Duration::from_millis(20);
const IDLE: Duration = Duration::from_secs(60);

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "the qualified private gateway or listener was not proved",
    )
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Original {
    dev: u64,
    ino: u64,
    group: u32,
}

struct Selected {
    listener: UnixListener,
    parent: HeldDir,
    socket: Original,
}

impl Selected {
    fn open(listener: UnixListener) -> io::Result<Self> {
        let uid = local_uid();
        if matches!(uid, 0 | 65534 | u32::MAX)
            || listener.local_addr()?.as_pathname() != Some(Path::new(SOCKET))
        {
            return Err(refused());
        }
        let parent = HeldDir::open(Path::new(PARENT))?;
        let socket = selected_name(&parent, uid)?;
        let selected = Self {
            listener,
            parent,
            socket,
        };
        selected.require()?;
        selected.listener.set_nonblocking(true)?;
        Ok(selected)
    }

    fn require(&self) -> io::Result<()> {
        if self.listener.local_addr()?.as_pathname() != Some(Path::new(SOCKET))
            || selected_name(&self.parent, local_uid())? != self.socket
        {
            return Err(refused());
        }
        Ok(())
    }
}

fn selected_name(parent: &HeldDir, uid: u32) -> io::Result<Original> {
    // Fixed, protected parent path, never a value from the incoming request.
    let ancestor = std::fs::symlink_metadata("/zxs")?;
    if !ancestor.is_dir() || ![0, uid].contains(&ancestor.uid()) || ancestor.mode() & 0o022 != 0 {
        return Err(refused());
    }
    parent.require_named()?;
    let directory = parent.file.metadata()?;
    if !directory.is_dir()
        || directory.uid() != uid
        || directory.mode() & 0o7777 != 0o710
        || matches!(directory.gid(), 0 | 65534 | u32::MAX)
    {
        return Err(refused());
    }
    let pinned = parent.pin_record(LEAF)?.ok_or_else(refused)?;
    let socket = pinned.metadata()?;
    if !socket.file_type().is_socket()
        || socket.uid() != uid
        || socket.gid() != directory.gid()
        || socket.dev() != directory.dev()
        || socket.ino() == 0
        || socket.nlink() != 1
        || socket.mode() & 0o7777 != 0o660
    {
        return Err(refused());
    }
    parent.require_named()?;
    Ok(Original {
        dev: socket.dev(),
        ino: socket.ino(),
        group: socket.gid(),
    })
}

/// The native startup owner supplies an already bound exact private socket
/// and independent positive evidence that today's effective gateway edition
/// and configuration enforce the single-complete-request contract. Merely
/// having a peer UID, TLS or Content-Length does not furnish that evidence.
/// The documented gateway audit remains outside this transport module. The
/// native launcher receives readiness only after the Store and listener are
/// proved locally; this does not attest to external HTTPS reachability.
pub(crate) fn serve_selected(
    bound: &Bound,
    listener: UnixListener,
    require_gateway: impl Fn() -> io::Result<()>,
    stopping: impl Fn() -> bool,
    report_ready: impl FnOnce() -> io::Result<()>,
    dispatch: impl Fn(&Store, Request, &mut Incoming<'_>, &dyn Fn() -> bool) -> Outcome + Sync,
) -> io::Result<()> {
    require_gateway()?;
    let selected = Selected::open(listener)?;
    require_gateway()?;
    if stopping() {
        bound.account().stop_admission();
        return Ok(());
    }
    report_ready()?;
    thread::scope(|scope| {
        let mut workers: Vec<thread::ScopedJoinHandle<'_, ()>> = Vec::new();
        let mut result = loop {
            let mut next = 0;
            let mut panicked = false;
            while next < workers.len() {
                if workers[next].is_finished() {
                    panicked |= workers.swap_remove(next).join().is_err();
                } else {
                    next += 1;
                }
            }
            if panicked {
                break Err(io::Error::other("headless worker stopped unexpectedly"));
            }
            if stopping() {
                break Ok(());
            }
            if workers.len() >= WORKERS {
                thread::sleep(POLL);
                continue;
            }
            match selected.listener.accept() {
                Ok((stream, _)) => {
                    if stopping() {
                        break Ok(());
                    }
                    if let Err(error) = selected.require().and_then(|()| require_gateway()) {
                        break Err(error);
                    }
                    if stream.set_nonblocking(false).is_err()
                        || stream.set_read_timeout(Some(IDLE)).is_err()
                        || stream.set_write_timeout(Some(IDLE)).is_err()
                    {
                        continue;
                    }
                    let dispatch = &dispatch;
                    match thread::Builder::new().spawn_scoped(scope, move || {
                        // An unauthorized/short/disconnected request is not a
                        // host failure. This is the original adopted socket.
                        let _ = serve_one(bound.account(), stream, |request, body, live| {
                            dispatch(bound.store(), request, body, live)
                        });
                    }) {
                        Ok(worker) => workers.push(worker),
                        Err(error) => break Err(error),
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => break Err(error),
            }
        };
        // No more accept; already admitted calls may send exact receipts
        // and complete their already started durable business operations.
        // The standard proxy ceases new upstream work on service shutdown.
        drop(selected);
        if result.is_err() {
            bound.account().stop_admission();
        }
        for worker in workers {
            if worker.join().is_err() && result.is_ok() {
                result = Err(io::Error::other("headless worker stopped during drain"));
            }
        }
        bound.account().stop_admission();
        result
    })
}
