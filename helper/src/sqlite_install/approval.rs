//! Actual host authority and permanent privilege selection for private workers.
//! No browser, environment, SQL row or saved grant can provide that authority.
pub(super) use crate::foreground::only_inherited_descriptors;
use crate::maintenance::diagnostic_worker;
pub(super) use crate::maintenance::host_admin;
use rustix::{
    process::{Gid, Uid},
    thread::{
        set_keep_capabilities, set_no_new_privs, set_thread_groups, set_thread_res_gid,
        set_thread_res_uid,
    },
};
use std::io;
// Fresh single-threaded host processes only. No escalation or recovery.
pub(super) fn drop_to_storage(uid: u32, gid: u32) -> io::Result<()> {
    if !(1..u32::MAX).contains(&uid) || !(1..u32::MAX).contains(&gid) {
        return Err(denied());
    }
    set_no_new_privs(true)?;
    set_keep_capabilities(false)?;
    set_thread_groups(&[])?;
    let g = Gid::from_raw(gid);
    let u = Uid::from_raw(uid);
    set_thread_res_gid(g, g, g)?;
    set_thread_res_uid(u, u, u)?;
    diagnostic_worker(uid, gid)
}

#[cfg(test)]
fn escaped(raw: &[u8]) -> String {
    raw.iter()
        .copied()
        .flat_map(std::ascii::escape_default)
        .map(char::from)
        .collect()
}
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "exact protected host administrator, explicit original scope and controlling terminal required",
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_escapes_opaque_or_terminal_control_bytes() {
        assert_eq!(escaped(b"/x/\xff\x1b"), r"/x/\xff\x1b");
    }
}
