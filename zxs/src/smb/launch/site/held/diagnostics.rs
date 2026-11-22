//! Optional foreground diagnostics. This is not a business log, a new
//! authority, or a writable descriptor made available to a lowered child.
use super::{
    OriginalAccess, PathPin, Place, Role, Stamp, denied, dir_reader, layout, leaf,
    no_xattr, place, regular, reopen_original, require_spelling,
};
use super::super::{SiteLease, protected_dir};
use crate::smb::launch::custody::Parent;
use std::{
    io::{self, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy)]
enum Event {
    ForegroundSelected,
    PairDrained,
    RunUnconfirmed,
}

impl Event {
    fn line(self, seconds: Option<u64>) -> String {
        let outcome = match self {
            Self::ForegroundSelected => "original-selected-not-started",
            Self::PairDrained => "web-smb-drained",
            Self::RunUnconfirmed => "start-or-drain-unconfirmed",
        };
        let at = seconds.map_or_else(|| "unknown".into(), |t| t.to_string());
        format!("zxs time_unix={at} lifecycle={outcome}\n")
    }
}

// regular() already proves a still-linked ordinary file with exactly one
// link and a nonzero inode. A named file on another mount is never opened.
fn eligible(stamp: Stamp, parent: Place) -> bool {
    stamp.uid == 0
        && stamp.gid == 0
        && stamp.mode == 0o100600
        && stamp.place.mount == parent.mount
        && stamp.place.entity.device == parent.entity.device
}

impl<'p> SiteLease<'p> {
    pub(in crate::smb::launch) fn record_selected(&self) {
        Event::ForegroundSelected.record(self.parent, self.deployment.state_pin());
    }

    pub(in crate::smb::launch) fn record_after_run(self, result: &io::Result<()>) {
        let (parent, state) = self.release_for_diagnostics();
        let event = if result.is_ok() { Event::PairDrained } else { Event::RunUnconfirmed };
        event.record(parent, &state);
    }

    // Consume all business custody. Only the original state O_PATH (not its
    // locked directory OFD) and the supervisor identity survive this return.
    fn release_for_diagnostics(self) -> (&'p Parent, PathPin) {
        (self.parent, self.deployment.into_state_pin())
    }
}

impl Event {
    fn record(self, parent: &Parent, state: &PathPin) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs());
        let line = self.line(now);
        if Self::append(parent, state, line.as_bytes()).is_err() {
            // A short write may have appended a prefix. Emit the full event
            // to stderr, never retry it against another file or roll it back.
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(b"zxs: /zxs/log/zxs.log unavailable; ");
            let _ = stderr.write_all(line.as_bytes());
        }
    }

    fn append(parent: &Parent, state: &PathPin, bytes: &[u8]) -> io::Result<()> {
        const NAME: &std::ffi::CStr = c"zxs.log";
        let host = parent.host().map_err(|_| denied())?;
        let task = parent.own_task().map_err(|_| denied())?;
        state.require(host, &task)?;
        let dir = PathPin::select(host, &task, Path::new(layout::LOG), Role::FixedDir)?;
        if state.parent_place() != dir.parent_place() || state.overlaps(&dir) {
            return Err(denied());
        }
        let reader = dir_reader(dir.held())?;
        protected_dir(&reader, 0, 0o040700)?;
        // Exact on-disk spelling even on casefold-capable filesystems.
        require_spelling(&reader, NAME.to_bytes())?;
        let pin = leaf(&reader, NAME)?;
        let stamp = regular(&pin)?;
        if !eligible(stamp, dir.location()) {
            return Err(denied());
        }
        // Only this retained, previously checked regular inode is activated;
        // no writable lookup of an untrusted name or special device.
        let mut out = reopen_original(&task, &pin, stamp, OriginalAccess::AppendLog)?;
        no_xattr(&out, c"system.posix_acl_access")?;
        state.require(host, &task)?;
        dir.require(host, &task)?;
        protected_dir(&reader, 0, 0o040700)?;
        require_spelling(&reader, NAME.to_bytes())?;
        if place(&reader)? != dir.location() || regular(&leaf(&reader, NAME)?)? != stamp
            || regular(&out)? != stamp
        {
            return Err(denied());
        }
        out.write_all(bytes)
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/attached/smb/launch/site/held/diagnostics/tests.rs"]
mod tests;
