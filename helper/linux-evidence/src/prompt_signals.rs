//! Scoped signal mask for local input on a *single-threaded* dedicated worker.
//! Never reads a credential, installs a handler, re-signals a PID, or changes
//! signal dispositions. Drop the terminal guard before restoring this mask.
use std::{io, marker::PhantomData, mem::zeroed, ptr, rc::Rc};

const INTERRUPTS: [libc::c_int; 16] = [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT,
    libc::SIGTERM, libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU, libc::SIGALRM, libc::SIGPIPE,
    libc::SIGCONT, libc::SIGUSR1, libc::SIGUSR2, libc::SIGXCPU, libc::SIGXFSZ, libc::SIGVTALRM, libc::SIGPROF];

pub struct PromptSignals {
    old: libc::sigset_t,
    active: bool,
    // pthread_sigmask affects only the current thread.
    _this_thread: PhantomData<Rc<()>>,
}

impl PromptSignals {
    #[allow(unsafe_code)]
    pub fn begin() -> io::Result<Self> {
        // SAFETY: initialized, process-local POSIX sets; no signal handler or
        // foreign pointer retained. pthread_sigmask returns errno directly.
        unsafe {
            let mut set: libc::sigset_t = zeroed();
            if libc::sigemptyset(&mut set) != 0 { return Err(io::Error::last_os_error()); }
            for s in INTERRUPTS {
                if libc::sigaddset(&mut set, s) != 0 { return Err(io::Error::last_os_error()); }
            }
            let mut old: libc::sigset_t = zeroed();
            let rc = libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old);
            if rc != 0 { return Err(io::Error::from_raw_os_error(rc)); }
            let this = Self { old, active: true, _this_thread: PhantomData };
            for sig in INTERRUPTS {
                if libc::sigismember(&this.old, sig) != 0 {
                    return Err(io::Error::other("local secret entry cannot inherit blocked cancellation signals"));
                }
                let mut action: libc::sigaction = zeroed();
                if libc::sigaction(sig, ptr::null(), &mut action) != 0 {
                    return Err(io::Error::last_os_error());
                }
                // The Rust runtime may ignore broken pipes; terminal output
                // errors cancel separately. Do not change an inherited policy.
                let handler = action.sa_sigaction;
                if handler != libc::SIG_DFL && !(sig == libc::SIGPIPE && handler == libc::SIG_IGN) {
                    return Err(io::Error::other("unexpected disposition for a terminal interruption"));
                }
            }
            this.check()?;
            Ok(this)
        }
    }

    #[allow(unsafe_code)]
    pub fn check(&self) -> io::Result<()> {
        // SAFETY: sigpending initializes the local result on success. Leave
        // pending signals pending: the original disposition must apply AFTER
        // restoring the terminal, rather than being suppressed or auto-resumed.
        unsafe {
            let mut pending: libc::sigset_t = zeroed();
            if libc::sigpending(&mut pending) != 0 { return Err(io::Error::last_os_error()); }
            for sig in INTERRUPTS {
                match libc::sigismember(&pending, sig) {
                    0 => (), 1 => return Err(io::Error::new(io::ErrorKind::Interrupted,"local secret entry cancelled")),
                    _ => return Err(io::Error::last_os_error()),
                }
            }
        }
        Ok(())
    }

    #[allow(unsafe_code)]
    fn restore(&mut self) -> io::Result<()> {
        if self.active {
            // SAFETY: self cannot be sent to another thread. Restore the exact
            // mask returned by this thread's successful pthread_sigmask.
            let rc = unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &self.old, ptr::null_mut()) };
            if rc != 0 { return Err(io::Error::from_raw_os_error(rc)); }
            self.active = false;
        }
        Ok(())
    }
    pub fn finish(mut self) -> io::Result<()> { self.restore() }
}

impl Drop for PromptSignals {
    fn drop(&mut self) { let _ = self.restore(); }
}
