//! One foreground local administrator authorizes one already-pinned set. No
//! browser, environment, SQL row, plain UID argument or saved grant can issue it.
use super::{
    Purpose,
    originals::Originals,
    pair::{InitialLayout, PublicConfirmation},
    product_layout,
    request::{Id, Request},
};
pub(super) use crate::foreground::{
    no_inherited_descriptors, only_inherited_descriptors, same_terminal,
};
use crate::{
    maintenance::{diagnostic_worker, thread_file},
    pinned::{Entity, Mount},
};
use rustix::{
    fs::{Mode, OFlags, PROC_SUPER_MAGIC, ResolveFlags, fstatfs, open, openat, openat2},
    process::{Gid, Uid, getegid, geteuid, getgid, getuid},
    thread::{
        set_keep_capabilities, set_no_new_privs, set_thread_groups, set_thread_res_gid,
        set_thread_res_uid,
    },
};
use std::{
    ffi::CStr,
    fmt::Write as _,
    fs::File,
    io::{self, Read},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

pub(super) struct Approved {
    request: Request,
    originals: Originals,
    purpose: Purpose,
}

/// Pins the selected original names without opening directories for data or
/// acquiring a lease. A paired browser may ask to display the terminal
/// confirmation, but only this live root foreground can make it Approved.
pub(super) struct Proposal {
    request: Request,
    originals: Originals,
    purpose: Purpose,
    subject: (Id, u32, u32),
    explicit: super::pair::ExplicitSubject,
    private: bool,
    layout: Option<InitialLayout>,
}
impl Proposal {
    /// Non-secret op/reference may be recorded BEFORE any write; they are not authority.
    pub(super) fn select(
        purpose: Purpose,
        op: Id,
        reference: Id,
        paths: Vec<PathBuf>,
        explicit: super::pair::ExplicitSubject,
        private: bool,
    ) -> io::Result<Self> {
        super::pair::compiled_schema()?;
        if private {
            super::pair::require_private_install()?;
            if !matches!(purpose, Purpose::Fresh) {
                return Err(denied());
            }
        }
        // A privileged FD inherited across lowering credentials retains its
        // original rights. A fresh terminal tool accepts only stdio, no inherited
        // file/socket/cwd-like authority from a shell, web worker or supervisor.
        no_inherited_descriptors()?;
        let host = host_admin()?;
        let subject = explicit.tuple();
        let request = Request::new(op, subject.0, reference, subject.1, paths)?;
        let layout = if matches!(purpose, Purpose::Fresh) {
            // Outside-only spellings can still traverse the actual product
            // through a whole local alias. Only definite absence exempts that
            // independent INIT; it never needs an existing /zxs/state.
            let required = private
                || request
                    .paths()
                    .iter()
                    .any(|p| p.starts_with(Path::new(product_layout::TOP)));
            Some(InitialLayout::select(&host, required)?)
        } else {
            None
        };
        // Check paths/SQLite capabilities before even showing a proposal. No SQL
        // connection, data read, directory allocation or privilege is granted here.
        request.database(super::engine::path_limit()?)?;
        let originals = Originals::pin_passive(&request, host.try_clone()?)?;
        let layout = layout
            .map(|origin| origin.match_media(&host, &originals.selected))
            .transpose()?;
        let out = Self {
            request,
            originals,
            purpose,
            subject,
            explicit,
            private,
            layout,
        };
        out.require()?;
        Ok(out)
    }
    pub(super) fn require(&self) -> io::Result<()> {
        let current = host_admin()?;
        self.originals.require_host(&current)?;
        super::pair::compiled_schema()?;
        if self.private {
            super::pair::require_private_install()?;
        }
        if self.explicit.tuple() != self.subject {
            return Err(denied());
        }
        same_terminal(&io::stdin(), &io::stderr())?;
        for n in &self.originals.selected {
            n.require_same_named_mount()?;
        }
        if let Some(layout) = &self.layout {
            layout.require(&current, &self.originals.selected)?;
        }
        Ok(())
    }
    pub(super) fn browser_scope(&self) -> ([u8; 32], bool, u8, super::pair::ExplicitSubject) {
        (
            self.request.sha(),
            matches!(self.purpose, Purpose::Fresh),
            (self.request.paths().len() - 2) as u8,
            self.explicit,
        )
    }
    pub(super) fn authorize(self, tty: PublicConfirmation) -> io::Result<Approved> {
        self.require()?;
        tty.check()?;
        let (actor, uid, gid) = self.subject;
        let op = self.request.op();
        let reference = self.request.approval();
        let mut preview=match self.purpose{
            Purpose::Fresh=>"新建空池：将创建 SYSTEM.sqlite3、写入原卷标并提交初始卷组。不会格式化、清空或覆盖。\n".to_owned(),
            Purpose::Original=>"续原单：仅当下面仍是原先确认的原目录/介质时，才可重同步并补齐该原单的卷标；已提交则只读历史回执。不新建数据库。\n".to_owned(),
        };
        writeln!(preview,"原操作 {}，审批参考 {}，本机维护主体 {}，存储 UID/GID {uid}/{gid}，原申请 SHA-256 {}。请先保存这两个操作参考。",
            hex(&op),hex(&reference),hex(&actor),hex(&self.request.sha())).expect("writing to String");
        for (i, n) in self.originals.selected.iter().enumerate() {
            let label = match i {
                0 => "SYSTEM",
                1 => "RECYCLE",
                _ => "FILE",
            };
            writeln!(
                preview,
                "{label}  {}  [dev {}, inode {}, mount {}]",
                escaped(n.path().as_os_str().as_bytes()),
                n.entity().dev,
                n.entity().ino,
                n.mount()?.id
            )
            .expect("writing to String");
        }
        preview.push_str("请独立核实这些原件和本机目录、锁、同步条件，以及回收卷确是独立文件系统。不确定就取消。\n");
        // Also used after the browser worker has really closed and been
        // reaped. The same retained TTY issues a new challenge, clearing all
        // previously queued input; the repeatable request SHA is not an answer.
        if self.private {
            tty.confirm_fresh_pool(preview)?;
        } else {
            tty.confirm_init(matches!(self.purpose, Purpose::Fresh), preview)?;
        }
        self.require()?;
        tty.check()?;
        drop(tty);
        let Self {
            request,
            originals,
            purpose,
            layout,
            ..
        } = self;
        drop(layout);
        // End the privileged phase BEFORE opening any storage directory for data or
        // reserving a file. This code runs only in a fresh, single-threaded CLI.
        // Any partial transition terminates the operation; no recovery as root.
        drop_to_storage(uid, gid)?;
        Ok(Approved {
            request,
            originals,
            purpose,
        })
    }
}
impl Approved {
    pub(super) fn into_parts(self) -> (Purpose, Request, Originals) {
        (self.purpose, self.request, self.originals)
    }
}

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

fn hex(v: &[u8]) -> String {
    crate::fs_tools::lower_hex(v)
}
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
pub(super) fn host_admin() -> io::Result<File> {
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
        if fstatfs(&ns)?.f_type as u64 != 0x6e73_6673 {
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
    #[test]
    fn preview_escapes_opaque_or_terminal_control_bytes() {
        assert_eq!(escaped(b"/x/\xff\x1b"), r"/x/\xff\x1b");
    }
}
