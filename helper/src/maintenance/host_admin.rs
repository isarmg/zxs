//! Prove the original root, credentials and host namespaces of a protected publisher.
use super::thread_file;
use crate::pinned::{Entity, Mount};
use rustix::{
    fs::{Mode, OFlags, PROC_SUPER_MAGIC, ResolveFlags, fstatfs, open, openat, openat2},
    process::{getegid, geteuid, getgid, getuid},
};
use std::{
    ffi::CStr,
    fs::File,
    io::{self, Read},
};

fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "exact protected host administrator, explicit original scope and controlling terminal required",
    )
}
fn fields(raw: &[u8]) -> impl Iterator<Item = &[u8]> {
    raw.split(u8::is_ascii_whitespace).filter(|s| !s.is_empty())
}
fn kernel(name: &CStr, max: u64) -> io::Result<Vec<u8>> {
    let mut v = Vec::new();
    thread_file(name)?.take(max + 1).read_to_end(&mut v)?;
    if v.len() as u64 > max || !v.ends_with(b"\n") {
        return Err(denied());
    }
    Ok(v)
}
// Linux 6.12 reserves these actual nsfs inode numbers for the initial
// PID and USER namespaces. A self-consistent isolated namespace is not host
// authority. For MNT we compare to the now independently identified host PID 1.
fn require_host_namespaces(current: [Entity; 3], pid1: [Entity; 3]) -> io::Result<()> {
    const INITIAL_PID: u64 = 0xefff_fffc;
    const INITIAL_USER: u64 = 0xefff_fffd;
    if current != pid1 || current[0].ino != INITIAL_PID || current[1].ino != INITIAL_USER {
        return Err(denied());
    }
    Ok(())
}
pub(crate) fn host_admin() -> io::Result<File> {
    if !(getuid().is_root() && geteuid().is_root() && getgid().is_root() && getegid().is_root()) {
        return Err(denied());
    }
    let s = kernel(c"thread-self/status", 65536)?;
    for (key, value) in [("Threads:", "1"), ("Uid:", "0 0 0 0"), ("Gid:", "0 0 0 0")] {
        let mut rows = s
            .split(|&v| v == b'\n')
            .filter_map(|l| l.strip_prefix(key.as_bytes()));
        if !rows
            .next()
            .is_some_and(|line| fields(line).eq(fields(value.as_bytes())))
            || rows.next().is_some()
        {
            return Err(denied());
        }
    }
    // Tiny subordinate user namespaces are not the host administrative identity.
    for n in [c"thread-self/uid_map", c"thread-self/gid_map"] {
        if !fields(&kernel(n, 4096)?).eq([b"0".as_slice(), b"0", b"4294967295"]) {
            return Err(denied());
        }
    }
    let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let proc = File::from(open(c"/proc", flags, Mode::empty())?);
    let mount = Mount::on(&proc)?
        .filter(|m| m.at_root == Some(true))
        .ok_or_else(denied)?;
    if fstatfs(&proc)?.f_type != PROC_SUPER_MAGIC {
        return Err(denied());
    }
    // Comparing just "/" is insufficient: two mount namespaces can expose
    // the same root but different mounts below it. Verify the genuine nsfs
    // objects from this procfs' PID 1 and from the current thread.
    let namespace = |who: &CStr, name: &CStr| -> io::Result<Entity> {
        let dir = File::from(openat2(
            &proc,
            who,
            flags,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_XDEV | ResolveFlags::NO_MAGICLINKS,
        )?);
        if Mount::on(&dir)?.is_none_or(|m| m.id != mount.id) {
            return Err(denied());
        }
        let link = File::from(openat(
            &dir,
            name,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        if !link.metadata()?.file_type().is_symlink()
            || Mount::on(&link)?.is_none_or(|m| m.id != mount.id)
        {
            return Err(denied());
        }
        let ns = File::from(openat(
            &dir,
            name,
            OFlags::PATH | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        if fstatfs(&ns)?.f_type.cast_unsigned() != 0x6e73_6673 {
            return Err(denied());
        } // Linux NSFS_MAGIC
        Entity::of(&ns.metadata()?)
    };
    let triple = |who: &CStr| -> io::Result<[Entity; 3]> {
        Ok([
            namespace(who, c"pid")?,
            namespace(who, c"user")?,
            namespace(who, c"mnt")?,
        ])
    };
    require_host_namespaces(triple(c"thread-self/ns")?, triple(c"1/ns")?)?;
    let pid = File::from(openat(&proc, c"1", flags, Mode::empty())?);
    if Mount::on(&pid)?.is_none_or(|m| m.id != mount.id) {
        return Err(denied());
    }
    // Verify the actual procfs magic link before following it, including protection
    // against an ordinary replacement mount. The resulting O_PATH is retained.
    let link = File::from(openat(
        &pid,
        c"root",
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    if !link.metadata()?.file_type().is_symlink()
        || Mount::on(&link)?.is_none_or(|m| m.id != mount.id)
    {
        return Err(denied());
    }
    let host = File::from(openat(
        &pid,
        c"root",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let here = File::from(open(c"/", flags, Mode::empty())?);
    if Entity::of(&host.metadata()?)? != Entity::of(&here.metadata()?)?
        || Mount::on(&host)? != Mount::on(&here)?
    {
        return Err(denied());
    }
    Ok(host)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_consistent_private_pid_or_user_view_does_not_establish_the_host() {
        let id = |ino| Entity { dev: 9, ino };
        let host = [id(0xefff_fffc), id(0xefff_fffd), id(4_026_531_841)];
        assert!(require_host_namespaces(host, host).is_ok());
        for i in [0, 1] {
            let mut private = host;
            private[i] = id(4_026_680_001);
            assert!(require_host_namespaces(private, private).is_err());
        }
        let mut other = host;
        other[2].ino += 1;
        assert!(require_host_namespaces(other, host).is_err());
        let mut foreign = host;
        foreign[2].dev += 1;
        assert!(require_host_namespaces(foreign, host).is_err());
    }
}
