//! Waitable children for the exact installed ZXS offline workers.
use super::{bad, transport::Channel};
use helper_linux_evidence::diagnostic_process::{
    prepare_import_one_exec, prepare_init_data_exec, require_waitable_children,
};
use rustix::{
    io::Errno,
    process::{
        Pid, PidfdFlags, Signal, WaitId, WaitIdOptions, pidfd_open, pidfd_send_signal, waitid,
    },
};
use std::{
    fs::File,
    io,
    os::fd::AsFd,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(crate) struct Worker {
    child: Child,
    fd: File,
    pub(crate) pid: Pid,
    done: bool,
}
impl Worker {
    pub(crate) fn spawn_import(
        exe: &File,
        peer: &File,
        order: &File,
        lease: &File,
    ) -> io::Result<Self> {
        Self::spawn_storage(exe, peer, order, lease, prepare_import_one_exec)
    }
    pub(crate) fn spawn_data_init(
        exe: &File,
        peer: &File,
        order: &File,
        lease: &File,
    ) -> io::Result<Self> {
        Self::spawn_storage(exe, peer, order, lease, prepare_init_data_exec)
    }
    fn spawn_storage(
        exe: &File,
        peer: &File,
        order: &File,
        lease: &File,
        prepare: fn(&mut Command, &File, &File, &File) -> io::Result<()>,
    ) -> io::Result<Self> {
        require_waitable_children()?;
        let mut cmd = Command::new("/not-an-offline-storage-program");
        cmd.env_clear()
            .stdin(Stdio::from(lease.try_clone()?))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        prepare(&mut cmd, exe, peer, order)?;
        Self::spawn_prepared(&mut cmd)
    }
    fn spawn_prepared(cmd: &mut Command) -> io::Result<Self> {
        let mut child = cmd.spawn()?;
        let actual = i32::try_from(child.id()).ok().and_then(Pid::from_raw);
        let Some(pid) = actual else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(bad("invalid exact maintenance child PID"));
        };
        // This Child alone owns the still-waitable child PID; it cannot be
        // reused before wait. Hard failure here never transfers an FD.
        match pidfd_open(pid, PidfdFlags::empty()) {
            Ok(fd) => Ok(Self {
                child,
                fd: File::from(fd),
                pid,
                done: false,
            }),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(e.into())
            }
        }
    }
    fn peek(&self) -> io::Result<Option<i32>> {
        loop {
            match waitid(
                WaitId::PidFd(self.fd.as_fd()),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            ) {
                Err(Errno::INTR) => {}
                Ok(s) => return Ok(s.map(|s| s.exit_status().unwrap_or(-1))),
                Err(e) => return Err(e.into()),
            }
        }
    }
    pub(crate) fn live(&self) -> io::Result<()> {
        if !self.done && self.peek()?.is_none() {
            Ok(())
        } else {
            Err(bad(
                "the original maintenance worker ended before the next step",
            ))
        }
    }
    pub(crate) fn finish_import(&mut self, channel: &Channel) -> io::Result<()> {
        let status = self.child.wait()?;
        self.done = true;
        if !status.success() {
            return Err(bad("the original import worker did not exit normally"));
        }
        channel.eof()
    }
}
impl Worker {
    pub(crate) fn cancel(&mut self) {
        if self.done {
            return;
        }
        // Ask the exact child to stop before considering force. If the kernel
        // is stalled even a kill-and-wait can be delayed. Forced exit never
        // supplies success; original inputs remain under Helper custody.
        let _ = pidfd_send_signal(&self.fd, Signal::TERM);
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end && self.peek().ok().flatten().is_none() {
            thread::sleep(Duration::from_millis(20));
        }
        if self.peek().ok().flatten().is_none() {
            let _ = pidfd_send_signal(&self.fd, Signal::KILL);
        }
        loop {
            match self.child.wait() {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Ok(_) => {
                    self.done = true;
                    break;
                }
                Err(_) => break,
            }
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
    }
}
