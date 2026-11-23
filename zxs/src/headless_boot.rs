//! Fresh native boot. One protected Unix socket, no TCP, TLS or legacy role.
use crate::{
    container_store::Store,
    fs::{Entity, HeldDir, local_uid},
    headless_gateway::{GatewayRun, LaunchFds},
    headless_http::{Incoming, Outcome},
    headless_lifecycle::{SOCKET, serve_selected},
    headless_start::{SelectedStart, effective_group},
    workspace_protocol::Request,
};
use std::{
    ffi::{CStr, c_char, c_int, c_long},
    fs::File,
    io,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt},
            net::UnixListener,
        },
    },
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

unsafe extern "C" {
    fn zxs_startup_socket_mode(fd: c_int) -> c_long;
    fn zxs_startup_install_stop_handlers(handler: extern "C" fn(c_int)) -> c_int;
    fn zxs_remove_name(fd: c_int, name: *const c_char) -> c_int;
}

const RUN: &str = "/zxs/run";
const LEAF: &CStr = c"headless.sock";
static STOP: AtomicBool = AtomicBool::new(false);

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "the original private headless socket was not proved",
    )
}

fn protected_parent() -> io::Result<HeldDir> {
    let uid = local_uid();
    let group = effective_group();
    if matches!(uid, 0 | 65534 | u32::MAX) || matches!(group, 0 | 65534 | u32::MAX) {
        return Err(refused());
    }
    let base = std::fs::symlink_metadata("/zxs")?;
    if !base.is_dir() || ![0, uid].contains(&base.uid()) || base.mode() & 0o022 != 0 {
        return Err(refused());
    }
    let parent = HeldDir::open(Path::new(RUN))?;
    let info = parent.file.metadata()?;
    if !info.is_dir()
        || info.uid() != uid
        || info.gid() != group
        || info.mode() & 0o7777 != 0o710
        || info.nlink() == 0
    {
        return Err(refused());
    }
    parent.require_named()?;
    Ok(parent)
}

struct PrivateName {
    parent: HeldDir,
    pin: File,
    entity: Entity,
    group: u32,
}

impl PrivateName {
    fn bind() -> io::Result<(UnixListener, Self)> {
        let parent = protected_parent()?;
        // No fallback, cleanup or takeover of a preexisting/stale socket,
        // symlink, device or file. bind itself rechecks name absence.
        if parent.pin_record(LEAF)?.is_some() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let listener = UnixListener::bind(SOCKET)?;
        parent.require_named()?;
        let pin = parent.pin_record(LEAF)?.ok_or_else(refused)?;
        let info = pin.metadata()?;
        let group = parent.file.metadata()?.gid();
        if !info.file_type().is_socket()
            || info.uid() != local_uid()
            || info.gid() != group
            || info.dev() != parent.entity.device
            || info.ino() == 0
            || info.nlink() != 1
        {
            return Err(refused());
        }
        let entity = Entity {
            device: info.dev(),
            inode: info.ino(),
        };
        let name = Self {
            parent,
            pin,
            entity,
            group,
        };
        let finish = (|| {
            name.require_named()?;
            // SAFETY: Linux fchmodat2 with AT_EMPTY_PATH uses the retained
            // O_PATH socket inode, never another name or a symlink target.
            let rc = unsafe { zxs_startup_socket_mode(name.pin.as_raw_fd()) };
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
            if name.pin.metadata()?.mode() & 0o7777 != 0o660 {
                return Err(refused());
            }
            name.require_named()
        })();
        if let Err(error) = finish {
            drop(listener);
            let _ = name.retire();
            return Err(error);
        }
        Ok((listener, name))
    }

    fn require_named(&self) -> io::Result<()> {
        self.parent.require_named()?;
        let named = self.parent.pin_record(LEAF)?.ok_or_else(refused)?;
        for file in [&self.pin, &named] {
            let info = file.metadata()?;
            if !info.file_type().is_socket()
                || info.uid() != local_uid()
                || info.gid() != self.group
                || info.nlink() != 1
                || info.dev() != self.entity.device
                || info.ino() != self.entity.inode
            {
                return Err(refused());
            }
        }
        Ok(())
    }

    fn retire(self) -> io::Result<()> {
        self.require_named()?;
        // SAFETY: only this invocation's retained original inode matched,
        // under the protected parent. Never remove a previous or foreign name.
        let rc = unsafe { zxs_remove_name(self.parent.file.as_raw_fd(), LEAF.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

extern "C" fn request_stop(_: c_int) {
    STOP.store(true, Ordering::Release);
}

fn install_signals() -> io::Result<()> {
    // SAFETY: only the dedicated single-threaded initial entry calls this.
    // The static C-ABI handler performs one lock-free AtomicBool store.
    if unsafe { zxs_startup_install_stop_handlers(request_stop) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Native one-shot entry after independent audit of the real gateway.
/// There is no permissive default proof producer or demonstration Store.
pub(crate) fn run(
    gateway: Option<LaunchFds>,
    dispatch: impl Fn(&Store, Request, &mut Incoming<'_>, &dyn Fn() -> bool) -> Outcome + Sync,
) -> io::Result<()> {
    let gateway = gateway.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "no original approved gateway launch was selected",
        )
    })?;
    let selected = SelectedStart::open_default()?;
    let uid = selected.gateway_uid();
    let gateway = GatewayRun::retain(gateway, uid, &selected.service_epoch())?;
    let require_gateway = || gateway.require_same_original(uid);
    require_gateway()?;
    install_signals()?;
    let bound = selected.bind()?;
    if STOP.load(Ordering::Acquire) {
        return Ok(());
    }
    require_gateway()?;
    let (listener, original) = PrivateName::bind()?;
    let result = serve_selected(
        &bound,
        listener,
        require_gateway,
        || STOP.load(Ordering::Acquire),
        dispatch,
    );
    // Bound owns the shared deployment lease through socket drain/retirement.
    let retired = original.retire();
    result.and(retired)
}
