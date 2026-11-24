//! Two fixed kernel observations of this very thread. No credentials are changed.
use crate::pinned::{self, Entity, Mount};
use rustix::fs::{Mode, OFlags, PROC_SUPER_MAGIC, ResolveFlags, fstatfs, open, openat2};
use std::{
    ffi::{CStr, OsStr},
    fs::File,
    io::{self, Read},
};

fn unsupported(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, reason)
}

pub(crate) fn thread_file(name: &CStr) -> io::Result<File> {
    let proc = File::from(open(
        "/proc",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let mount = Mount::on(&proc)?.ok_or_else(|| unsupported("proc mount identity unavailable"))?;
    if fstatfs(&proc)?.f_type != PROC_SUPER_MAGIC || mount.at_root != Some(true) {
        return Err(unsupported("need the root of a genuine procfs"));
    }
    // Accept the kernel's relative thread-self link, never a magic link, escape,
    // final symlink or overmount (including a bind over just the chosen proc file).
    let fd = File::from(openat2(
        &proc,
        name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_XDEV | ResolveFlags::NO_MAGICLINKS,
    )?);
    if !fd.metadata()?.is_file()
        || fstatfs(&fd)?.f_type != PROC_SUPER_MAGIC
        || Mount::on(&fd)?.is_none_or(|m| m.id != mount.id)
    {
        return Err(unsupported(
            "cannot attribute kernel data to this thread's procfs",
        ));
    }
    Ok(fd)
}

#[cfg(any(test, feature = "embedded-zxs"))]
pub(super) fn mountinfo() -> io::Result<File> {
    thread_file(c"thread-self/mountinfo")
}

/// Only discovery's current mount table. Walk actual PID/task/TID on the
/// held proc mount without following a directory lookup into an automount.
pub(super) fn passive_mountinfo() -> io::Result<File> {
    let (root, mount) = pinned::passive_task_proc()?;
    let name = OsStr::new("mountinfo");
    let prior = pinned::passive_proc_child(&root, name, mount, false)?;
    let original = Entity::of(&prior.metadata()?)?;
    let rd = File::from(openat2(
        &root,
        name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_XDEV | ResolveFlags::NO_SYMLINKS,
    )?);
    if !rd.metadata()?.is_file()
        || Entity::of(&rd.metadata()?)? != original
        || fstatfs(&rd)?.f_type != PROC_SUPER_MAGIC
        || Mount::on(&rd)?.is_none_or(|m| m.id != mount)
        || Entity::of(&pinned::passive_proc_child(&root, name, mount, false)?.metadata()?)?
            != original
    {
        return Err(unsupported(
            "mount table name or genuine procfs identity changed",
        ));
    }
    Ok(rd)
}

const STATUS_MAX: usize = 64 * 1024;

/// Process-local sandbox eligibility only. Does not consult a pool or issue
/// maintenance authority. A fresh CLI sets `NoNewPrivs` before taking maintenance custody.
pub(crate) fn diagnostic_worker(uid: u32, gid: u32) -> io::Result<()> {
    let mut data = Vec::new();
    thread_file(c"thread-self/status")?
        .take(STATUS_MAX as u64 + 1)
        .read_to_end(&mut data)?;
    check_status(&data, uid, gid)?;
    let mut fields = data
        .split(|&b| b == b'\n')
        .filter_map(|line| line.strip_prefix(b"Threads:"))
        .flat_map(|v| v.split(u8::is_ascii_whitespace).filter(|f| !f.is_empty()));
    if fields.next() != Some(&b"1"[..]) || fields.next().is_some() {
        return Err(unsupported(
            "diagnostics require an independent single-threaded CLI process",
        ));
    }
    Ok(())
}

fn check_status(data: &[u8], uid: u32, service_gid: u32) -> io::Result<()> {
    const KEYS: [&[u8]; 8] = [
        b"Uid:",
        b"Gid:",
        b"Groups:",
        b"CapInh:",
        b"CapPrm:",
        b"CapEff:",
        b"CapAmb:",
        b"NoNewPrivs:",
    ];
    let denied = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "need the unprivileged, no-new-privileges storage worker",
        )
    };
    if [uid, service_gid].iter().any(|v| *v == 0 || *v == u32::MAX)
        || data.len() > STATUS_MAX
        || !data.ends_with(b"\n")
    {
        return Err(denied());
    }
    let uid = uid.to_string();
    let gid = service_gid.to_string();
    let mut seen = [false; KEYS.len()];
    for line in data.split(|b| *b == b'\n') {
        let mut fields = line
            .split(u8::is_ascii_whitespace)
            .filter(|x| !x.is_empty());
        let Some(key) = fields.next() else { continue };
        let Some(i) = KEYS.iter().position(|&k| k == key) else {
            continue;
        };
        if seen[i] {
            return Err(denied());
        }
        seen[i] = true;
        let first = fields.next();
        let valid = match i {
            0 | 1 => {
                let expected = if i == 0 {
                    uid.as_bytes()
                } else {
                    gid.as_bytes()
                };
                first == Some(expected) && (0..3).all(|_| fields.next() == Some(expected))
            }
            // No inheritable host-group access; some launchers also list the one
            // permitted primary group as supplementary. A second entry is rejected below.
            2 => first.is_none() || first == Some(gid.as_bytes()),
            3..=6 => first == Some(&b"0000000000000000"[..]),
            _ => first == Some(&b"1"[..]),
        };
        if !valid || fields.next().is_some() {
            return Err(denied());
        }
    }
    if seen.into_iter().all(|v| v) {
        Ok(())
    } else {
        Err(denied())
    }
}

#[cfg(test)]
#[path = "../../tests/maintenance/storage_worker.rs"]
mod tests;
