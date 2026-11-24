//! Private foreground-terminal and inherited-descriptor evidence. No storage mutation.
pub(crate) mod channel;
pub(crate) use channel::PublicConfirmation;

use crate::pinned::Mount;
use rustix::fs::{
    self, Mode, OFlags, PROC_SUPER_MAGIC, RawDir, ResolveFlags, fstatfs, open, openat2,
};
use std::{
    fs::File,
    io,
    mem::MaybeUninit,
    os::fd::{AsFd, AsRawFd},
};

fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "exact protected host administrator, explicit original scope and controlling terminal required",
    )
}
fn supported_terminal(mode: u32, dev: u64, magic: u64) -> bool {
    let (major, minor) = (rustix::fs::major(dev), rustix::fs::minor(dev));
    mode & 0o170_000 == 0o020_000
        && ((major, minor) == (5, 0)
            || (major == 4 && (1..=63).contains(&minor))
            || ((136..=143).contains(&major) && magic == 0x1cd1)) // Linux devpts slave
}
fn foreground(tty_sid: i32, our_sid: i32, tty_group: i32, our_group: i32) -> bool {
    tty_sid > 0 && tty_group > 0 && tty_sid == our_sid && tty_group == our_group
}
fn require_terminal(fd: impl AsFd) -> io::Result<()> {
    use rustix::{
        fs::{fstat, fstatfs},
        process::{getpgrp, getsid},
        termios::{tcgetpgrp, tcgetsid},
    };
    let deny = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "requires the operator's supported foreground controlling terminal",
        )
    };
    let stat = fstat(&fd)?;
    if !supported_terminal(
        stat.st_mode,
        stat.st_rdev,
        fstatfs(&fd)?.f_type.cast_unsigned(),
    ) {
        return Err(deny());
    }
    if !foreground(
        tcgetsid(&fd)?.as_raw_pid(),
        getsid(None)?.as_raw_pid(),
        tcgetpgrp(&fd)?.as_raw_pid(),
        getpgrp().as_raw_pid(),
    ) {
        return Err(deny());
    }
    Ok(())
}

pub(crate) fn same_terminal(input: &io::Stdin, output: &io::Stderr) -> io::Result<()> {
    let stdout = io::stdout();
    require_terminal(input.as_fd())?;
    require_terminal(output.as_fd())?;
    require_terminal(stdout.as_fd())?;
    let a = fs::fstat(input)?;
    for b in [fs::fstat(output)?, fs::fstat(stdout)?] {
        if a.st_dev != b.st_dev || a.st_ino != b.st_ino || a.st_rdev != b.st_rdev {
            return Err(denied());
        }
    }
    Ok(())
}
fn own_descriptor_table() -> io::Result<(File, File)> {
    let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let proc = File::from(open(c"/proc", flags, Mode::empty())?);
    let mount = Mount::on(&proc)?
        .filter(|m| m.at_root == Some(true))
        .ok_or_else(denied)?;
    if fstatfs(&proc)?.f_type != PROC_SUPER_MAGIC {
        return Err(denied());
    }
    let dir = File::from(openat2(
        &proc,
        c"thread-self/fd",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_XDEV | ResolveFlags::NO_MAGICLINKS,
    )?);
    if Mount::on(&dir)?.is_none_or(|m| m.id != mount.id)
        || fstatfs(&dir)?.f_type != PROC_SUPER_MAGIC
    {
        return Err(denied());
    }
    Ok((proc, dir))
}
pub(crate) fn no_inherited_descriptors() -> io::Result<()> {
    let (proc, dir) = own_descriptor_table()?;
    let mut buf = [MaybeUninit::uninit(); 4096];
    let mut entries = RawDir::new(&dir, &mut buf);
    while let Some(e) = entries.next() {
        let e = e?;
        let bytes = e.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        let fd = std::str::from_utf8(bytes)
            .ok()
            .and_then(|s| s.parse::<i32>().ok())
            .ok_or_else(denied)?;
        if !matches!(fd, 0..=2) && fd != proc.as_raw_fd() && fd != dir.as_raw_fd() {
            return Err(denied());
        }
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn foreground_publish_requires_a_supported_terminal_and_the_callers_session() {
    use rustix::fs::makedev;
    assert!(supported_terminal(0o020_620, makedev(136, 4), 0x1cd1));
    assert!(supported_terminal(0o020_620, makedev(4, 1), 0));
    assert!(supported_terminal(0o020_666, makedev(5, 0), 0));
    for (mode, dev, fs) in [
        (0o020_666, makedev(5, 2), 0x1cd1),
        (0o100_600, makedev(136, 4), 0x1cd1),
        (0o020_620, makedev(136, 4), 0),
        (0o010_600, 0, 0),
    ] {
        assert!(!supported_terminal(mode, dev, fs));
    }
    assert!(foreground(31, 31, 47, 47));
    assert!(!foreground(30, 31, 47, 47));
    assert!(!foreground(31, 31, 42, 47));
    assert!(!foreground(0, 0, 47, 47));
    assert!(!foreground(31, 31, 0, 0));
}
