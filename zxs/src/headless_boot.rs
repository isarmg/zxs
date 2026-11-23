//! Foreground HTTP startup with the original, qualified storage selection.
use crate::{
    container_store::Store,
    headless_http::{Incoming, Outcome},
    headless_lifecycle::serve_selected,
    headless_start::SelectedStart,
    workspace_protocol::Request,
};
use std::{
    ffi::c_int,
    io,
    net::{SocketAddr, TcpListener},
    sync::atomic::{AtomicBool, Ordering},
};

unsafe extern "C" {
    fn zxs_startup_install_stop_handlers(handler: extern "C" fn(c_int)) -> c_int;
}

static STOP: AtomicBool = AtomicBool::new(false);

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

pub(crate) fn run(
    listen: SocketAddr,
    dispatch: impl Fn(&Store, Request, &mut Incoming<'_>, &dyn Fn() -> bool) -> Outcome + Sync,
) -> io::Result<()> {
    if listen.port() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an explicit nonzero listen port is required",
        ));
    }
    let stopping = || STOP.load(Ordering::Acquire);
    let selected = SelectedStart::open_default()?;
    install_signals()?;
    if stopping() {
        return Ok(());
    }
    let bound = selected.bind()?;
    if stopping() {
        return Ok(());
    }
    // A SocketAddr is one actual numeric bind, not DNS or address discovery.
    let listener = TcpListener::bind(listen)?;
    serve_selected(&bound, listener, stopping, dispatch)
}
