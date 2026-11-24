//! Fixed child lifetime primitives for a one-shot CLI, never a generic shell.
//! The installed ZXS offline entrypoints, plus foreground cancellation.
use linux_raw_sys::general as uapi;
use std::{
    fs::File,
    io,
    mem::MaybeUninit,
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::Command,
};

use std::sync::atomic::{AtomicBool, Ordering};
static CANCELLED: AtomicBool = AtomicBool::new(false);
extern "C" fn cancel(_: libc::c_int) {
    CANCELLED.store(true, Ordering::Relaxed);
}
pub fn cancelled() -> bool {
    CANCELLED.load(Ordering::Relaxed)
}

/// Only for the dedicated CLI process. Cancellation is recorded without
/// allocation or non-signal-safe I/O. exec resets caught signals in the child.
///
/// # Errors
/// Returns an error if a cancellation signal is blocked or its handler
/// cannot be installed.
pub fn watch_cancellation() -> io::Result<()> {
    watch(&[libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT])
}
#[allow(unsafe_code)]
fn watch(signals: &[libc::c_int]) -> io::Result<()> {
    // SAFETY: fully initialized Linux sigaction with a C-ABI handler that
    // only sets a lock-free x86-64 atomic bool. This does not send a signal.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = cancel as *const () as usize;
        let mut current = MaybeUninit::<libc::sigset_t>::uninit();
        if libc::sigprocmask(libc::SIG_SETMASK, std::ptr::null(), current.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        let current = current.assume_init();
        if libc::sigemptyset(&raw mut action.sa_mask) != 0 {
            return Err(io::Error::last_os_error());
        }
        for &sig in signals {
            // A blocked inherited cancellation signal is not observable by our handler.
            if libc::sigismember(&raw const current, sig) != 0 {
                return Err(io::Error::other(
                    "diagnostic cancellation signals must be unblocked",
                ));
            }
            if libc::sigaction(sig, &raw const action, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

/// The fixed maintenance child must remain waitable until the parent reaps it.
/// Reject inherited dispositions that could auto-reap the child.
///
/// # Errors
/// Returns an error if `SIGCHLD` is not verifiably in its default waitable state.
#[allow(unsafe_code)]
pub fn require_waitable_children() -> io::Result<()> {
    let mut a = MaybeUninit::<libc::sigaction>::uninit();
    // SAFETY: no new handler is installed. The kernel initializes a on success.
    if unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), a.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: sigaction succeeded above.
    let a = unsafe { a.assume_init() };
    if a.sa_sigaction != libc::SIG_DFL || a.sa_flags & libc::SA_NOCLDWAIT != 0 {
        return Err(io::Error::other(
            "diagnostic monitor needs default, waitable child disposition",
        ));
    }
    Ok(())
}

/// Select the fixed local file import entrypoint on the exact installed executable.
///
/// # Errors
/// Errors if its selected executable or descriptor capabilities cannot be retained.
pub fn prepare_import_one_exec(
    cmd: &mut Command,
    original: &File,
    channel: &File,
    order: &File,
) -> io::Result<()> {
    prepare_fixed_peer_exec(
        cmd,
        original,
        channel,
        order,
        c"zxs",
        c"__offline-import-one-v1",
    )
}

/// Select the fixed whole-batch import entrypoint on the same pinned executable.
///
/// # Errors
/// Errors if its original executable, channel or sealed order cannot be retained.
pub fn prepare_import_batch_exec(
    cmd: &mut Command,
    original: &File,
    channel: &File,
    order: &File,
) -> io::Result<()> {
    prepare_fixed_peer_exec(
        cmd,
        original,
        channel,
        order,
        c"zxs",
        c"__offline-import-batch-v1",
    )
}

/// Select the dedicated enrolled-DATA initialization worker on the pinned binary.
///
/// # Errors
/// Fails if the exact executable, order or channel cannot be retained.
pub fn prepare_init_data_exec(
    cmd: &mut Command,
    original: &File,
    channel: &File,
    order: &File,
) -> io::Result<()> {
    prepare_fixed_peer_exec(
        cmd,
        original,
        channel,
        order,
        c"zxs",
        c"__offline-init-data-v1",
    )
}

#[allow(unsafe_code)]
fn prepare_fixed_peer_exec(
    cmd: &mut Command,
    original: &File,
    channel: &File,
    order: &File,
    program: &'static std::ffi::CStr,
    worker_argument: &'static std::ffi::CStr,
) -> io::Result<()> {
    use std::os::fd::AsFd;
    let high = |f: &File| {
        rustix::io::fcntl_dupfd_cloexec(f.as_fd(), 16)
            .map(File::from)
            .map_err(io::Error::from)
    };
    let executable = high(original)?;
    let channel = high(channel)?;
    let order = high(order)?;
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: Owned, distinct high fds prevent dup3 clobbering the selected ELF.
    // The child calls only async-safe syscalls on fixed memory before exec,
    // never enables shell fallback, reallocates or uses a caller-supplied arg.
    unsafe {
        cmd.pre_exec(move || {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = libc::SIG_DFL;
            let mut term: libc::sigset_t = std::mem::zeroed();
            if libc::sigemptyset(&raw mut action.sa_mask) != 0
                || libc::sigemptyset(&raw mut term) != 0
                || libc::sigaddset(&raw mut term, libc::SIGTERM) != 0
                || libc::sigaction(libc::SIGTERM, &raw const action, std::ptr::null_mut()) != 0
                || libc::sigprocmask(libc::SIG_UNBLOCK, &raw const term, std::ptr::null_mut()) != 0
                || libc::syscall(
                    libc::SYS_close_range,
                    3u32,
                    u32::MAX,
                    libc::CLOSE_RANGE_CLOEXEC,
                ) != 0
                || libc::prctl(
                    libc::PR_SET_PDEATHSIG,
                    libc::SIGTERM as libc::c_ulong,
                    0 as libc::c_ulong,
                    0 as libc::c_ulong,
                    0 as libc::c_ulong,
                ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(io::Error::from_raw_os_error(libc::ECANCELED));
            }
            if libc::dup3(channel.as_raw_fd(), 3, 0) != 3
                || libc::dup3(order.as_raw_fd(), 4, 0) != 4
            {
                return Err(io::Error::last_os_error());
            }
            let argv = [program.as_ptr(), worker_argument.as_ptr(), std::ptr::null()];
            let env = [c"LC_ALL=C".as_ptr(), std::ptr::null()];
            libc::syscall(
                libc::c_long::from(uapi::__NR_execveat),
                executable.as_raw_fd(),
                c"".as_ptr(),
                argv.as_ptr(),
                env.as_ptr(),
                libc::AT_EMPTY_PATH,
            );
            Err(io::Error::last_os_error())
        });
    }
    Ok(())
}

/// Execute only the fixed installed ZXS read-only catalog worker.
///
/// # Errors
/// Fails if the protected executable cannot be duplicated for exec.
#[allow(unsafe_code)]
pub fn prepare_offline_catalog_exec(cmd: &mut Command, original: &File) -> io::Result<()> {
    let executable = original.try_clone()?;
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: the closure uses fixed storage and async-safe Linux calls.
    // The selected executable is already open, with no PATH or shell fallback.
    unsafe {
        cmd.pre_exec(move || {
            // SIG_IGN and the signal mask can survive exec. Make termination
            // effective *before* arming PDEATHSIG; a lost parent before arming
            // is detected by the getppid check before executing ZXS.
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = libc::SIG_DFL;
            let mut term: libc::sigset_t = std::mem::zeroed();
            if libc::sigemptyset(&raw mut action.sa_mask) != 0
                || libc::sigemptyset(&raw mut term) != 0
                || libc::sigaddset(&raw mut term, libc::SIGTERM) != 0
                || libc::sigaction(libc::SIGTERM, &raw const action, std::ptr::null_mut()) != 0
                || libc::sigprocmask(libc::SIG_UNBLOCK, &raw const term, std::ptr::null_mut()) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::syscall(
                libc::SYS_close_range,
                3u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            ) != 0
                || libc::prctl(
                    libc::PR_SET_PDEATHSIG,
                    libc::SIGTERM as libc::c_ulong,
                    0 as libc::c_ulong,
                    0 as libc::c_ulong,
                    0 as libc::c_ulong,
                ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(io::Error::from_raw_os_error(libc::ECANCELED));
            }
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            let argv = [
                c"zxs".as_ptr(),
                c"__offline-live-names-v1".as_ptr(),
                std::ptr::null(),
            ];
            let env = [c"LC_ALL=C".as_ptr(), std::ptr::null()];
            libc::syscall(
                libc::c_long::from(uapi::__NR_execveat),
                executable.as_raw_fd(),
                c"".as_ptr(),
                argv.as_ptr(),
                env.as_ptr(),
                libc::AT_EMPTY_PATH,
            );
            Err(io::Error::last_os_error())
        });
    }
    Ok(())
}
