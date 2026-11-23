//! setsid+exec occur before bubblewrap starts. All trusted launch stages stay
//! in this group; the fixed sandbox payload cannot fork or switch session/group.
use rustix::{
    io::Errno,
    process::{
        Pid, Signal, WaitId, WaitIdOptions, WaitOptions, kill_process_group, set_child_subreaper,
        waitid, waitpgid,
    },
};
use std::{io, process::Child};

pub(crate) fn prepare() -> io::Result<()> {
    helper_linux_evidence::diagnostic_process::require_waitable_children()?;
    helper_linux_evidence::diagnostic_process::watch_cancellation()?;
    // The kernel PR_SET_CHILD_SUBREAPER argument is boolean (nonzero).
    set_child_subreaper(Pid::from_raw(1))?;
    if rustix::process::child_subreaper()?.is_none() {
        return Err(io::Error::other(
            "cannot supervise orphaned launcher stages",
        ));
    }
    Ok(())
}
pub(crate) struct Reaped {
    pub(crate) leader_code: Option<i32>,
    #[cfg(test)]
    pub(crate) members: usize,
}
pub(crate) struct Group {
    pub(crate) child: Child,
    id: Pid,
    signaled: bool,
}
impl Group {
    pub(crate) fn new(child: Child) -> Self {
        // Linux PID is a positive pid_t; successful exec also proves setsid
        // was performed by this very child. No untrusted process can join this new session.
        let id = Pid::from_raw(child.id() as i32).expect("fresh Linux child");
        Self {
            child,
            id,
            signaled: false,
        }
    }
    pub(crate) fn peek_exit(&self) -> io::Result<Option<i32>> {
        match waitid(
            WaitId::Pid(self.id),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ) {
            Ok(None) | Err(Errno::INTR) => Ok(None),
            Ok(Some(s)) => s
                .exit_status()
                .map(Some)
                .ok_or_else(|| io::Error::other("launcher terminated by signal")),
            Err(e) => Err(e.into()),
        }
    }
    // We never try_wait/reap the monitor before killing the group: even an
    // already-dead leader reserves its numeric identity against reuse.
    pub(crate) fn finish(mut self) -> io::Result<Reaped> {
        match kill_process_group(self.id, Signal::KILL) {
            Ok(()) | Err(Errno::SRCH) => self.signaled = true,
            Err(e) => return Err(e.into()),
        }
        // Reap only children in the session's original group, including
        // descendants adopted by our subreaper when their parents exited.
        let mut leader = None;
        #[cfg(test)]
        let mut members = 0;
        loop {
            match waitpgid(self.id, WaitOptions::empty()) {
                Ok(Some((id, status))) => {
                    #[cfg(test)]
                    {
                        members += 1;
                    }
                    if id == self.id {
                        leader = Some(status.exit_status());
                    }
                }
                Err(Errno::INTR) => continue,
                Err(Errno::CHILD) => {
                    return leader
                        .map(|leader_code| Reaped {
                            leader_code,
                            #[cfg(test)]
                            members,
                        })
                        .ok_or_else(|| {
                            io::Error::other("original sandbox launcher wait status unavailable")
                        });
                }
                Ok(None) => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }
}
impl Drop for Group {
    fn drop(&mut self) {
        // Best effort only in unwinding. Once finish started reaping, this
        // numeric identity must NEVER be signaled again.
        if !self.signaled {
            let _ = kill_process_group(self.id, Signal::KILL);
        }
    }
}
