//! Host evidence for the fixed /zxs and /zxs/state hierarchy. No deployment or database authority.
use crate::{
    maintenance::HostMounts,
    pinned::{
        DiscoveryAdmission, Entity, Mount, discover_component, local_data_filesystem,
        require_no_automount,
    },
    product_layout::{self, STATE_COMPONENTS as PRODUCT},
};
use rustix::{
    fs::{FlockOperation, Mode, OFlags, fcntl_getfl, fgetxattr, flock, openat},
    io::Errno,
};
use std::{
    ffi::{CStr, CString, OsStr},
    fs::File,
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
};

fn bad(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

pub(crate) fn mark(fd: &File) -> io::Result<(Entity, u64)> {
    Ok((
        Entity::of(&fd.metadata()?)?,
        Mount::on(fd)?
            .ok_or_else(|| bad("missing original mount identity"))?
            .id,
    ))
}
pub(crate) fn child(parent: &File, name: &std::ffi::CStr) -> io::Result<File> {
    Ok(File::from(openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?))
}
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct Times {
    pub(crate) links: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
impl Times {
    pub(crate) fn on(f: &File) -> io::Result<Self> {
        let m = f.metadata()?;
        Ok(Self {
            links: m.nlink(),
            len: m.len(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
        })
    }
}
pub(crate) fn safe_dir(f: &File) -> io::Result<()> {
    let m = f.metadata()?;
    if !m.is_dir() || m.uid() != 0 || m.nlink() == 0 || m.mode() & 0o022 != 0 {
        return Err(bad("unprotected host instance ancestor"));
    }
    Ok(())
}
// Fixed, bare leaf probes below the admitted product root never cross even
// a same-device bind and do not induce a terminal automount.
fn product_leaf(parent: &File, name: &CStr) -> io::Result<File> {
    discover_component(parent, OsStr::from_bytes(name.to_bytes()), false)
}
pub(crate) fn product_pin(parent: &File, name: &CStr) -> io::Result<File> {
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
pub(crate) fn product_top(host: &File) -> io::Result<File> {
    observe_top(host)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "the required protected /zxs directory is absent",
        )
    })
}
pub(crate) fn state_pins(host: &File) -> io::Result<[File; 2]> {
    let top = product_top(host)?;
    let state = product_pin(&top, PRODUCT[1])?;
    state_spelling(host, &top, &state)?;
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
// Do not infer write/search rights from group bits while a named Linux POSIX
// ACL or inherited default ACL may alter them. Unknown on this host is not
// permission to chown/chmod an existing object.
pub(crate) fn bare_acl(f: &File, key: &CStr) -> io::Result<()> {
    match fgetxattr(f, key, &mut [0u8; 0][..]) {
        Err(Errno::NODATA) => Ok(()),
        Ok(_) | Err(Errno::RANGE) => Err(bad(
            "extended POSIX ACL is outside the first-install layout profile",
        )),
        Err(e) => Err(e.into()),
    }
}
// Opening a fixed spelling can reach another spelling on a casefolded FS.
// Read only directory entries on the already-selected parent's own stream;
// never open or return an unrelated entry. The independent lookup must still
// resolve to the held directory (a mount point need not have its dirent ino).
pub(crate) fn exact_with(
    parent: &File,
    name: &CStr,
    selected: &File,
    reopen: impl FnOnce() -> io::Result<File>,
) -> io::Result<()> {
    product_layout::raw_member(parent, OsStr::from_bytes(name.to_bytes()), || {
        if mark(&reopen()?)? != mark(selected)? {
            return Err(bad("changed child while checking exact layout spelling"));
        }
        Ok(())
    })
}
fn check_directory(pin: &File, check: impl FnOnce(&File) -> io::Result<()>) -> io::Result<()> {
    let dir = child(pin, c".")?;
    if mark(&dir)? != mark(pin)? {
        return Err(bad("changed original protected directory"));
    }
    check(&dir)
}
pub(crate) fn top_spelling(host: &File, top: &File) -> io::Result<()> {
    // The actual host root allows no access ACL; the fixed product root
    // must match the exact same profile as the cold consumer.
    check_directory(host, |f| bare_acl(f, c"system.posix_acl_access"))?;
    exact_with(host, PRODUCT[0], top, || product_top(host))?;
    check_directory(top, protected_control)
}
pub(crate) fn state_spelling(host: &File, top: &File, state: &File) -> io::Result<()> {
    top_spelling(host, top)?;
    exact_with(top, PRODUCT[1], state, || product_pin(top, PRODUCT[1]))?;
    check_directory(state, protected_control)
}
pub(crate) fn protected_control(dir: &File) -> io::Result<()> {
    attrs(
        dir,
        0,
        0,
        Mode::from_bits_retain(product_layout::CONTROL_MODE),
        true,
    )?;
    bare_acl(dir, c"system.posix_acl_access")?;
    bare_acl(dir, c"system.posix_acl_default")
}
// A whole already mounted /zxs is the sole possible OS mount boundary.
// Keep every original descriptor; no transition at state or an instance.
fn one_mount(host: (crate::pinned::Entity, u64), d: (crate::pinned::Entity, u64)) -> bool {
    host.0.dev == d.0.dev && host.1 == d.1
}
pub(crate) fn host_mount(host: &File, chain: &[File]) -> io::Result<()> {
    let mut parent = mark(host)?;
    let mut seen = vec![parent.0];
    for (level, d) in chain.iter().enumerate() {
        let current = mark(d)?;
        if seen.contains(&current.0) {
            return Err(bad("aliased original instance ancestors"));
        }
        if !one_mount(parent, current) {
            if level != 0 || parent.1 == current.1 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "a separate mount or device change below /zxs is not supported for protected state",
                ));
            }
            let mount = Mount::on(d)?
                .filter(|m| m.id == current.1)
                .ok_or_else(|| bad("layout mount identity changed"))?;
            HostMounts::current()?.require_product_mount(
                mount,
                parent.1,
                current.0,
                crate::pinned::local_data_filesystem(d)?,
            )?;
        }
        seen.push(current.0);
        parent = current;
    }
    Ok(())
}
fn attrs(f: &File, u: u32, g: u32, mode: Mode, directory: bool) -> io::Result<()> {
    let md = f.metadata()?;
    if (if directory {
        !md.is_dir() || md.nlink() == 0
    } else {
        !md.is_file() || md.nlink() != 1
    }) || md.uid() != u
        || md.gid() != g
        || md.mode() & 0o7777 != mode.bits()
    {
        return Err(bad(
            "original layout object does not have its exact owner, group or mode",
        ));
    }
    Ok(())
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
pub(crate) fn lease(fd: &File, id: (Entity, u64)) -> io::Result<()> {
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
