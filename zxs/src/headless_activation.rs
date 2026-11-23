//! Privileged foreground supervision of an already qualified host selection.
use super::{
    ApprovedNginx, ApprovedService, LaunchFds, bad, launch_approved, launch_service, live,
    protected_dir, protected_file,
};
use crate::fs::local_uid;
use std::{
    ffi::{c_int, c_long},
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    process::Child,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

unsafe extern "C" {
    fn zxs_startup_quit_gateway(pidfd: c_int) -> c_long;
    fn zxs_startup_term_service(pidfd: c_int) -> c_long;
    fn zxs_startup_install_stop_handlers(handler: extern "C" fn(c_int)) -> c_int;
    fn zxs_gateway_open_pidfd(owned_child: c_int) -> c_long;
}

const ENROLLMENT: &str = "/etc/zxs-gateway/activation.v1";
const LIMIT: u64 = 256;
const POLL: Duration = Duration::from_millis(25);
const STARTUP_LIMIT: Duration = Duration::from_secs(60);
static STOP: AtomicBool = AtomicBool::new(false);

fn sha(value: &str) -> io::Result<[u8; 32]> {
    fn digit(byte: u8) -> io::Result<u8> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(bad()),
        }
    }
    if value.len() != 64 {
        return Err(bad());
    }
    let mut answer = [0; 32];
    for (out, pair) in answer.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *out = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    if answer == [0; 32] {
        return Err(bad());
    }
    Ok(answer)
}

fn identity(value: &str) -> io::Result<u32> {
    if value.is_empty() || value.starts_with('0') || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let value = value.parse::<u32>().map_err(|_| bad())?;
    if matches!(value, 0 | 65534 | u32::MAX) {
        return Err(bad());
    }
    Ok(value)
}

fn enrollment() -> io::Result<(ApprovedNginx, ApprovedService)> {
    use std::os::unix::fs::MetadataExt;
    for path in ["/etc", "/etc/zxs-gateway"] {
        protected_dir(path)?;
    }
    let file = protected_file(ENROLLMENT, LIMIT)?;
    let first = file.metadata()?;
    if first.mode() & 0o7777 != 0o400 {
        return Err(bad());
    }
    let mut bytes = Vec::new();
    (&file).take(LIMIT + 1).read_to_end(&mut bytes)?;
    let named = protected_file(ENROLLMENT, LIMIT)?.metadata()?;
    let stamp = |m: &std::fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mode(),
            m.ctime(),
            m.ctime_nsec(),
            m.mtime(),
            m.mtime_nsec(),
        )
    };
    if bytes.len() as u64 != first.len()
        || stamp(&first) != stamp(&file.metadata()?)
        || stamp(&first) != stamp(&named)
    {
        return Err(bad());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| bad())?;
    let mut fields = text.strip_suffix('\n').ok_or_else(bad)?.split('\n');
    if fields.next() != Some("ZXS-HOST-ACTIVATION-1") {
        return Err(bad());
    }
    let mut next = || fields.next().ok_or_else(bad);
    let nginx = ApprovedNginx {
        executable_sha: sha(next()?)?,
        configuration_sha: sha(next()?)?,
        worker_uid: identity(next()?)?,
    };
    let service = ApprovedService {
        image_sha: sha(next()?)?,
        uid: identity(next()?)?,
        ipc_gid: identity(next()?)?,
    };
    if fields.next().is_some() || service.uid == nginx.worker_uid {
        return Err(bad());
    }
    Ok((nginx, service))
}

extern "C" fn request_stop(_: c_int) {
    STOP.store(true, Ordering::Release);
}

fn install_stop_handlers() -> io::Result<()> {
    // SAFETY: called before launching children. The static C-ABI handler
    // only stores an atomic flag and does not handle descriptors or allocate.
    if unsafe { zxs_startup_install_stop_handlers(request_stop) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn open_service_pidfd(child: &Child) -> io::Result<File> {
    let pid = c_int::try_from(child.id()).map_err(|_| bad())?;
    // SAFETY: the unreaped direct child still owns the numeric PID.
    let fd = unsafe { zxs_gateway_open_pidfd(pid) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful syscall created a fresh owned Linux descriptor.
    Ok(unsafe { File::from_raw_fd(c_int::try_from(fd).map_err(|_| bad())?) })
}

fn stop_original(child: &mut Child, signal: impl FnOnce() -> c_long) -> io::Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    if signal() != 0 {
        let error = io::Error::last_os_error();
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        return Err(error);
    }
    Ok(())
}

fn wait_original(child: &mut Child, name: &str) -> io::Result<()> {
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "original {name} stopped with {status}"
        )))
    }
}

fn drain(
    gateway: &mut Child,
    fds: &LaunchFds,
    service: Option<(&mut Child, &File)>,
) -> io::Result<()> {
    // Let the original ZXS stop admission and drain accepted requests.
    // Request graceful shutdown of both originals before waiting for either.
    let service = service.map(|(child, pidfd)| {
        let signal = stop_original(child, || {
            // SAFETY: retained PIDFD names the directly owned original service.
            unsafe { zxs_startup_term_service(pidfd.as_raw_fd()) }
        });
        (child, signal)
    });
    let gateway_signal = stop_original(gateway, || {
        // SAFETY: a borrowed PIDFD names the directly owned original gateway.
        unsafe { zxs_startup_quit_gateway(fds.original_pidfd.as_raw_fd()) }
    });
    let service_result = service.map_or(Ok(()), |(child, signal)| {
        signal.and_then(|()| wait_original(child, "ZXS service"))
    });
    let gateway_result = gateway_signal.and_then(|()| wait_original(gateway, "gateway"));
    combine(service_result, gateway_result)
}

fn stop_failed_service(child: &mut Child) -> io::Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    match child.kill() {
        Ok(()) => child.wait().map(|_| ()),
        Err(error) => {
            if child.try_wait()?.is_some() {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}

fn combine(result: io::Result<()>, cleanup: io::Result<()>) -> io::Result<()> {
    match (result, cleanup) {
        (Err(error), Err(stop)) => Err(io::Error::other(format!(
            "{error}; original process shutdown also reported ({stop})"
        ))),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn require_running(gateway: &mut Child, service: &mut Child) -> io::Result<()> {
    if let Some(status) = gateway.try_wait()? {
        return Err(io::Error::other(format!(
            "original gateway exited: {status}"
        )));
    }
    if let Some(status) = service.try_wait()? {
        return Err(io::Error::other(format!(
            "original ZXS service exited: {status}"
        )));
    }
    Ok(())
}

fn monitor(
    gateway: &mut Child,
    service: &mut Child,
    mut try_ready: impl FnMut() -> io::Result<bool>,
) -> io::Result<()> {
    let start = Instant::now();
    let mut ready = false;
    loop {
        if STOP.load(Ordering::Acquire) {
            return Ok(());
        }
        require_running(gateway, service)?;
        if !ready {
            ready = try_ready()?;
            if STOP.load(Ordering::Acquire) {
                return Ok(());
            }
            if start.elapsed() >= STARTUP_LIMIT {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the original ZXS service did not acknowledge startup in time",
                ));
            }
            if ready {
                require_running(gateway, service)?;
                writeln!(io::stderr().lock(), "activate-headless: ZXS backend ready")?;
            }
        }
        thread::sleep(POLL);
    }
}

pub(crate) fn activate() -> io::Result<()> {
    if local_uid() != 0 {
        return Err(bad());
    }
    let (nginx, service) = enrollment()?;
    install_stop_handlers()?;
    if STOP.load(Ordering::Acquire) {
        return Ok(());
    }
    let (mut gateway_child, fds) = launch_approved(&nginx)?;
    if STOP.load(Ordering::Acquire) {
        return drain(&mut gateway_child, &fds, None);
    }
    let mut service = match live(&fds.original_pidfd).and_then(|()| launch_service(&service, &fds))
    {
        Ok(service) => service,
        Err(error) => return combine(Err(error), drain(&mut gateway_child, &fds, None)),
    };
    let service_pidfd = match open_service_pidfd(&service.child) {
        Ok(pidfd) => pidfd,
        Err(error) => {
            // No service readiness was announced. The Child handle still
            // owns the unreaped PID; kill and reap this failed startup only.
            let service_stop = stop_failed_service(&mut service.child);
            return combine(
                Err(error),
                combine(service_stop, drain(&mut gateway_child, &fds, None)),
            );
        }
    };
    let result = monitor(&mut gateway_child, &mut service.child, || {
        service.readiness.try_ready()
    });
    combine(
        result,
        drain(
            &mut gateway_child,
            &fds,
            Some((&mut service.child, &service_pidfd)),
        ),
    )
}
