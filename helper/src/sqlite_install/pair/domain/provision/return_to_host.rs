//! Actual root-owned installation hierarchy and exact original directory locks.
mod deployed;
mod layout;
mod public_record;
mod rust_site;
#[cfg(test)]
mod rust_vault_io;
mod start;
use super::{bad, child, contents, live, mark};
use crate::{
    maintenance::HostMounts,
    pinned::{
        DiscoveryAdmission, Entity, Mount, discover_component, local_data_filesystem,
        require_no_automount,
    },
    sqlite_install::{approval, product_layout, request::Id},
};
pub(in crate::sqlite_install::pair) use deployed::deployed_worker_payload;
pub(in crate::sqlite_install::pair) use layout::FreshParent;
pub(in crate::sqlite_install::pair) use rust_site::{
    VaultStamp, checked_vault_stamp, verify_fresh_worker,
};
use rustix::{
    fs::{FlockOperation, OFlags, fcntl_getfl, flock},
    io::Errno,
};
pub(in crate::sqlite_install::pair) use start::child as request_live_approval;
use std::{
    ffi::{CStr, CString, OsStr},
    fs::File,
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
};

const PRODUCT: [&CStr; 2] = product_layout::STATE_COMPONENTS;
fn safe_dir(f: &File) -> io::Result<()> {
    let m = f.metadata()?;
    if !m.is_dir() || m.uid() != 0 || m.nlink() == 0 || m.mode() & 0o022 != 0 {
        return Err(bad("unprotected host instance ancestor"));
    }
    Ok(())
}
struct Site {
    host: File,
    chain: Vec<File>,
    leaf: CString,
}
// Fixed, bare leaf probes below the admitted product root never cross even
// a same-device bind and do not induce a terminal automount.
fn product_leaf(parent: &File, name: &CStr) -> io::Result<File> {
    discover_component(parent, OsStr::from_bytes(name.to_bytes()), false)
}
fn product_pin(parent: &File, name: &CStr) -> io::Result<File> {
    let f = product_leaf(parent, name)?;
    require_no_automount(&f)?;
    safe_dir(&f)?;
    local_data_filesystem(&f)?;
    let (before, from) = mark(parent)?;
    let (next, to) = mark(&f)?;
    if from != to || before.dev != next.dev || next == before {
        return Err(bad(
            "different protected product mount or aliased directory",
        ));
    }
    Ok(f)
}
// Only the final bare NO_XDEV lookup may report an absent optional top.
// Failure to obtain kernel evidence or a named expected crossing is not absence.
fn optional_top<T>(crossing: Option<u64>, leaf: io::Result<T>) -> io::Result<Option<T>> {
    match leaf {
        Ok(f) => Ok(Some(f)),
        Err(e) if crossing.is_none() && e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn observe_top(host: &File) -> io::Result<Option<File>> {
    let view = HostMounts::discovery_view(false)?;
    let root = view.root_mount()?;
    if view.admit(host)? != root {
        return Err(bad("the actual host root mount changed"));
    }
    safe_dir(host)?;
    let wanted = view.product_crossing(root)?;
    let leaf = discover_component(
        host,
        OsStr::from_bytes(PRODUCT[0].to_bytes()),
        wanted.is_some(),
    );
    let Some(f) = optional_top(wanted, leaf)? else {
        return Ok(None);
    };
    // The inherited proc descriptor's actual mount id precedes any metadata
    // or filesystem query on that opened leaf. No fallback on EXDEV/EAGAIN.
    if view.admit(&f)? != wanted.unwrap_or(root) {
        return Err(bad("unexpected /zxs mount transition"));
    }
    require_no_automount(&f)?;
    safe_dir(&f)?;
    let magic = local_data_filesystem(&f)?;
    let (entity, mount) = mark(&f)?;
    let original = mark(host)?;
    if mount != wanted.unwrap_or(root) || original.1 != root || entity == original.0 {
        return Err(bad("changed or aliased original /zxs directory"));
    }
    if wanted.is_some() {
        let at = Mount::on(&f)?
            .filter(|m| m.id == mount)
            .ok_or_else(|| bad("/zxs mount identity changed"))?;
        view.require_product_mount(at, root, entity, magic)?;
    } else if entity.dev != original.0.dev {
        return Err(bad("unattributed device change at /zxs"));
    }
    Ok(Some(f))
}
fn product_top(host: &File) -> io::Result<File> {
    observe_top(host)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "the required protected /zxs directory is absent",
        )
    })
}
fn state_pins(host: &File) -> io::Result<[File; 2]> {
    let top = product_top(host)?;
    let state = product_pin(&top, PRODUCT[1])?;
    layout::state_spelling(host, &top, &state)?;
    Ok([top, state])
}
#[cfg(test)]
#[test]
fn layout_v18_optional_top_allows_only_positive_final_same_mount_absence() {
    let absent = || io::Error::from(io::ErrorKind::NotFound);
    assert_eq!(
        optional_top(None, Ok("already held")).unwrap(),
        Some("already held")
    );
    assert_eq!(optional_top::<()>(None, Err(absent())).unwrap(), None);
    assert!(optional_top::<()>(Some(91), Err(absent())).is_err());
    for fail in [
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::WouldBlock,
        io::ErrorKind::Unsupported,
    ] {
        assert!(optional_top::<()>(None, Err(fail.into())).is_err());
    }
}
impl Site {
    fn product(host: File, instance: &Id, missing_final: bool) -> io::Result<Self> {
        crate::sqlite_install::pair::compiled_schema()?;
        let leaf = CString::new(crate::fs_tools::lower_hex(instance)).expect("lower hex");
        let mut chain = Vec::from(state_pins(&host)?);
        match product_pin(&chain[1], &leaf) {
            Ok(original) => chain.push(original),
            Err(e) if missing_final && e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let this = Self { host, chain, leaf };
        this.check()?;
        Ok(this)
    }
    fn component(parent: &File, name: &CStr, level: usize) -> io::Result<File> {
        if level == 0 && name == PRODUCT[0] {
            product_top(parent)
        } else {
            product_pin(parent, name)
        }
    }
    fn base(&self) -> &File {
        self.chain.last().expect("base retained")
    }
    fn check(&self) -> io::Result<()> {
        self.require_from(approval::host_admin()?)
    }
    fn require_from(&self, host: File) -> io::Result<()> {
        safe_dir(&self.host)?;
        safe_dir(&host)?;
        if mark(&self.host)? != mark(&host)? {
            return Err(bad("different current host root"));
        }
        let mut p = host;
        for (level, (original, n)) in self
            .chain
            .iter()
            .zip(
                PRODUCT
                    .iter()
                    .copied()
                    .chain(std::iter::once(self.leaf.as_c_str())),
            )
            .enumerate()
        {
            let next = Self::component(&p, n, level)?;
            safe_dir(original)?;
            if mark(original)? != mark(&next)? {
                return Err(bad("protected installation path changed"));
            }
            p = next;
        }
        Ok(())
    }
}
fn owns_flock(data: &[u8], inode: u64, mount: u64, mode: &str) -> bool {
    let Ok(text) = std::str::from_utf8(data) else {
        return false;
    };
    if text.len() > 4096 || !text.ends_with('\n') {
        return false;
    }
    let mut got = [false; 3];
    for row in text.lines() {
        let Some((key, value)) = row.split_once(':') else {
            return false;
        };
        let value = value.trim_ascii();
        let index = match key {
            "mnt_id" if value.parse::<u64>() == Ok(mount) => 0,
            "ino" if value.parse::<u64>() == Ok(inode) => 1,
            "lock" => {
                let fields: Vec<_> = value.split_ascii_whitespace().collect();
                let [
                    ordinal,
                    "FLOCK",
                    "ADVISORY",
                    got_mode,
                    pid,
                    device,
                    "0",
                    "EOF",
                ] = fields.as_slice()
                else {
                    return false;
                };
                if *ordinal != "1:" || *got_mode != mode || pid.parse::<i32>().is_err() {
                    return false;
                }
                let mut dev = device.split(':');
                let hex = |s: Option<&str>| {
                    s.is_some_and(|v| {
                        !v.is_empty() && v.len() <= 8 && v.bytes().all(|b| b.is_ascii_hexdigit())
                    })
                };
                if !hex(dev.next())
                    || !hex(dev.next())
                    || dev.next().and_then(|n| n.parse::<u64>().ok()) != Some(inode)
                    || dev.next().is_some()
                {
                    return false;
                }
                2
            }
            "mnt_id" | "ino" => return false,
            _ => continue,
        };
        if std::mem::replace(&mut got[index], true) {
            return false;
        }
    }
    got == [true; 3]
}
fn require_owned(fd: &File, id: (Entity, u64), mode: &str) -> io::Result<()> {
    let path =
        CString::new(format!("thread-self/fdinfo/{}", fd.as_raw_fd())).expect("decimal descriptor");
    let mut bytes = Vec::new();
    crate::maintenance::thread_file(&path)?
        .take(4097)
        .read_to_end(&mut bytes)?;
    if !owns_flock(&bytes, id.0.ino, id.1, mode) {
        return Err(bad(
            "no exact requested whole-file flock on this open description",
        ));
    }
    Ok(())
}
pub(in crate::sqlite_install) fn lease(fd: &File, id: (Entity, u64)) -> io::Result<()> {
    let flags = fcntl_getfl(fd)?;
    if flags.contains(OFlags::PATH)
        || !flags.contains(OFlags::DIRECTORY)
        || flags & OFlags::ACCMODE != OFlags::RDONLY
        || mark(fd)? != id
        || !fd.metadata()?.is_dir()
    {
        return Err(bad("not the expected original worker lease"));
    }
    require_owned(fd, id, "WRITE")?;
    // Probe a competing independent OFD; never convert or reacquire the
    // original. A READ loan can never satisfy this WRITE-lease check.
    let other = child(fd, c".")?;
    if mark(&other)? != id {
        return Err(bad("lease original changed"));
    }
    match flock(&other, FlockOperation::NonBlockingLockShared) {
        Err(Errno::WOULDBLOCK) => require_owned(fd, id, "WRITE"),
        Err(e) => Err(e.into()),
        Ok(()) => Err(bad("original lease no longer conflicts")),
    }
}

#[cfg(test)]
#[test]
fn a_shared_fdinfo_witness_cannot_authorize_an_exclusive_system_loan() {
    let read = b"mnt_id: 17\nino: 31\nlock: 1: FLOCK ADVISORY READ 705 08:06:31 0 EOF\n";
    let write = b"mnt_id: 17\nino: 31\nlock: 1: FLOCK ADVISORY WRITE 705 08:06:31 0 EOF\n";
    assert!(owns_flock(read, 31, 17, "READ"));
    assert!(!owns_flock(read, 31, 17, "WRITE"));
    assert!(owns_flock(write, 31, 17, "WRITE"));
    assert!(!owns_flock(write, 31, 18, "WRITE"));
}
