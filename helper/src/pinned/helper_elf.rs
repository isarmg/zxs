//! The already executing Helper, not an installed program or an alternate
//! command. Kernel identity, the owning ELF and any independent expected
//! digest are different checks. Never derive a credential grant here.
use super::{Entity, Mount, invalid, local_data_filesystem, passive_proc_child, passive_proc_root};
use rustix::{
    fs::{
        Mode, OFlags, PROC_SUPER_MAGIC, ResolveFlags, StatVfsMountFlags, fgetxattr, fstatfs,
        fstatvfs, makedev, openat, openat2,
    },
    io::Errno,
    process::{Pid, getegid, geteuid, getgid, getpid, getppid, getuid},
};
#[cfg(any(test, feature = "embedded-zxs"))]
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, MetadataExt},
    },
};

const MAX_IMAGE: u64 = 64 << 20;
const MAX_MAPS: usize = 256 << 10;
const MAX_AUXV: usize = 4096;
const PROC_WITHIN: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_XDEV)
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS);
const PIN: OFlags = OFlags::PATH.union(OFlags::CLOEXEC).union(OFlags::NOFOLLOW);
fn no() -> io::Error {
    invalid("cannot bind the original foreground Helper ELF to this offline worker")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    #[cfg(any(test, feature = "embedded-zxs"))]
    Privileged,
    Local {
        uid: u32,
        gid: u32,
    },
}
impl Role {
    fn require(self) -> io::Result<()> {
        let (uid, gid) = (getuid().as_raw(), getgid().as_raw());
        let want = match self {
            #[cfg(any(test, feature = "embedded-zxs"))]
            Self::Privileged => (0, 0),
            Self::Local { uid, gid } if uid != 0 && gid != 0 => (uid, gid),
            Self::Local { .. } => return Err(no()),
        };
        if (uid, gid) != want || geteuid().as_raw() != uid || getegid().as_raw() != gid {
            return Err(no());
        }
        Ok(())
    }
    fn accepts(self, s: Stamp) -> bool {
        if !(1..=MAX_IMAGE).contains(&s.len) || s.links != 1 {
            return false;
        }
        match self {
            #[cfg(any(test, feature = "embedded-zxs"))]
            Self::Privileged => (s.uid, s.gid, s.mode) == (0, 0, 0o100_755),
            Self::Local { uid, .. } => match s.uid {
                0 => s.gid == 0 && s.mode == 0o100_755,
                own if own == uid => {
                    matches!(s.mode, 0o100_500 | 0o100_555 | 0o100_700 | 0o100_755)
                }
                _ => false,
            },
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Stamp {
    entity: Entity,
    mount: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    links: u64,
    len: u64,
    ctime: [i64; 2],
    mtime: [i64; 2],
}
impl Stamp {
    fn at(f: &File) -> io::Result<Self> {
        let m = f.metadata()?;
        Ok(Self {
            entity: Entity::of(&m)?,
            mount: Mount::on(f)?.ok_or_else(no)?.id,
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
            links: m.nlink(),
            len: m.len(),
            ctime: [m.ctime(), m.ctime_nsec()],
            mtime: [m.mtime(), m.mtime_nsec()],
        })
    }
}
struct ProcessFiles {
    proc: File,
    mount: u64,
}
impl ProcessFiles {
    fn select() -> io::Result<Self> {
        let (proc, mount) = passive_proc_root()?;
        Ok(Self { proc, mount })
    }
    fn directory(&self, id: Pid) -> io::Result<File> {
        let pid = id.as_raw_nonzero().get().to_string();
        passive_proc_child(&self.proc, OsStr::new(&pid), self.mount, true)
    }
    fn kernel_link(&self, dir: &File, leaf: &OsStr, flags: OFlags) -> io::Result<File> {
        // First obtain the actual untraversed same-procfs magic link. The
        // only follow sites are fixed numeric owned descriptors and exe.
        let link = || -> io::Result<Entity> {
            let fd = File::from(openat2(dir, leaf, PIN, Mode::empty(), PROC_WITHIN)?);
            let m = fd.metadata()?;
            if !m.file_type().is_symlink()
                || fstatfs(&fd)?.f_type != PROC_SUPER_MAGIC
                || Mount::on(&fd)?.is_none_or(|x| x.id != self.mount)
            {
                return Err(no());
            }
            Entity::of(&m)
        };
        let before = link()?;
        let actual = File::from(openat(dir, leaf, flags | OFlags::CLOEXEC, Mode::empty())?);
        if link()? != before {
            return Err(no());
        }
        Ok(actual)
    }
    fn exe(&self, id: Pid) -> io::Result<File> {
        self.kernel_link(&self.directory(id)?, OsStr::new("exe"), OFlags::PATH)
    }
    fn open_pinned(&self, fd: &File) -> io::Result<File> {
        let me = self.directory(getpid())?;
        let files = passive_proc_child(&me, OsStr::new("fd"), self.mount, true)?;
        self.kernel_link(
            &files,
            OsStr::new(&fd.as_raw_fd().to_string()),
            OFlags::RDONLY | OFlags::NONBLOCK,
        )
    }
    fn own_table(&self, name: &'static str, max: usize) -> io::Result<Vec<u8>> {
        let me = self.directory(getpid())?;
        let name = OsStr::new(name);
        let control = passive_proc_child(&me, name, self.mount, false)?;
        let read = File::from(openat2(
            &me,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
            PROC_WITHIN,
        )?);
        let same = || -> io::Result<bool> {
            Ok(read.metadata()?.is_file()
                && fstatfs(&read)?.f_type == PROC_SUPER_MAGIC
                && Mount::on(&read)?.is_some_and(|x| x.id == self.mount)
                && Entity::of(&read.metadata()?)? == Entity::of(&control.metadata()?)?)
        };
        if !same()? {
            return Err(no());
        }
        let mut out = Vec::new();
        (&read).take(max as u64 + 1).read_to_end(&mut out)?;
        if out.len() > max || !same()? {
            return Err(no());
        }
        Ok(out)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CodeMap {
    kernel_id: Entity,
    offset: u64,
}
fn code_map(bytes: &[u8], address: u64) -> io::Result<CodeMap> {
    if bytes.len() > MAX_MAPS || !bytes.ends_with(b"\n") {
        return Err(no());
    }
    let dec = |text: &[u8]| -> Option<u64> {
        let s = std::str::from_utf8(text).ok()?;
        let n = s.parse::<u64>().ok()?;
        (n.to_string().as_bytes() == text).then_some(n)
    };
    let hex = |text: &[u8]| -> Option<u64> {
        let s = std::str::from_utf8(text).ok()?;
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| u64::from_str_radix(s, 16).ok())
            .flatten()
    };
    let range = |b: &[u8]| -> Option<(u64, u64)> {
        let i = b.iter().position(|&x| x == b'-')?;
        Some((hex(&b[..i])?, hex(&b[i + 1..])?))
    };
    let id = |device: &[u8], inode: &[u8]| -> Option<Entity> {
        let i = device.iter().position(|&x| x == b':')?;
        let major = u32::try_from(hex(&device[..i])?).ok()?;
        let minor = u32::try_from(hex(&device[i + 1..])?).ok()?;
        let ino = dec(inode)?;
        (ino != 0).then_some(Entity {
            dev: makedev(major, minor),
            ino,
        })
    };
    let mut selected = None;
    for (row, line) in bytes[..bytes.len() - 1].split(|&x| x == b'\n').enumerate() {
        if row >= 4096 {
            return Err(no());
        }
        let mut fields = line
            .split(u8::is_ascii_whitespace)
            .filter(|b| !b.is_empty());
        let (start, end) = range(fields.next().ok_or_else(no)?).ok_or_else(no)?;
        if start >= end {
            return Err(no());
        }
        if !(start..end).contains(&address) {
            continue;
        }
        if fields.next() != Some(b"r-xp") {
            return Err(no());
        }
        let base = hex(fields.next().ok_or_else(no)?).ok_or_else(no)?;
        let kernel_id =
            id(fields.next().ok_or_else(no)?, fields.next().ok_or_else(no)?).ok_or_else(no)?;
        let offset = base.checked_add(address - start).ok_or_else(no)?;
        if selected.replace(CodeMap { kernel_id, offset }).is_some() {
            return Err(no());
        }
    }
    selected.ok_or_else(no)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExecAux {
    phdr: u64,
    phent: u64,
    phnum: u64,
    entry: u64,
    page: u64,
}
fn exec_aux(raw: &[u8]) -> io::Result<ExecAux> {
    // Native x86-64 ELF, not the supplied argv or a userspace copy of auxv.
    let (words, tail) = raw.as_chunks::<16>();
    if words.is_empty() || raw.len() > MAX_AUXV || !tail.is_empty() {
        return Err(no());
    }
    let mut wanted = [None; 5];
    let mut ended = false;
    for (i, e) in words.iter().enumerate() {
        let tag = u64::from_le_bytes(e[..8].try_into().expect("word"));
        let value = u64::from_le_bytes(e[8..].try_into().expect("word"));
        let j = match tag {
            0 if value == 0 && i + 1 == words.len() => {
                ended = true;
                break;
            }
            0 => return Err(no()),
            3 => 0,
            4 => 1,
            5 => 2,
            9 => 3,
            6 => 4,
            _ => continue,
        };
        if value == 0 || wanted[j].replace(value).is_some() {
            return Err(no());
        }
    }
    let [
        Some(phdr),
        Some(phent),
        Some(phnum),
        Some(entry),
        Some(page),
    ] = wanted
    else {
        return Err(no());
    };
    if !ended
        || phent != 56
        || !(1..=128).contains(&phnum)
        || !(4096..=(1 << 20)).contains(&page)
        || !page.is_power_of_two()
    {
        return Err(no());
    }
    Ok(ExecAux {
        phdr,
        phent,
        phnum,
        entry,
        page,
    })
}
fn load_offsets(
    mut read: impl FnMut(u64, &mut [u8]) -> io::Result<()>,
    size: u64,
    aux: ExecAux,
    here: u64,
) -> io::Result<(u64, u64)> {
    let mut elf = [0; 64];
    read(0, &mut elf)?;
    let half = |b: &[u8], o: usize| u16::from_le_bytes(b[o..o + 2].try_into().expect("half"));
    let word = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().expect("word"));
    let wide = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().expect("wide"));
    let kind = half(&elf, 16);
    let entry = wide(&elf, 24);
    let phoff = wide(&elf, 32);
    let width = aux.phent.checked_mul(aux.phnum).ok_or_else(no)?;
    if !elf.starts_with(b"\x7fELF\x02\x01\x01")
        || !matches!(kind, 2 | 3)
        || half(&elf, 18) != 62
        || word(&elf, 20) != 1
        || half(&elf, 52) != 64
        || u64::from(half(&elf, 54)) != aux.phent
        || u64::from(half(&elf, 56)) != aux.phnum
        || entry == 0
        || phoff < 64
        || phoff.checked_add(width).is_none_or(|end| end > size)
    {
        return Err(no());
    }
    let mut table_vaddr = None;
    let mut rx = Vec::new();
    for index in 0..aux.phnum {
        let mut row = [0; 56];
        read(phoff + 56 * index, &mut row)?;
        if word(&row, 0) != 1 {
            continue;
        }
        let (off, start, files, mem) = (
            wide(&row, 8),
            wide(&row, 16),
            wide(&row, 32),
            wide(&row, 40),
        );
        let file_end = off
            .checked_add(files)
            .filter(|&end| end <= size)
            .ok_or_else(no)?;
        let virtual_end = start.checked_add(files).ok_or_else(no)?;
        if files > mem || start.checked_add(mem).is_none() || off % aux.page != start % aux.page {
            return Err(no());
        }
        if phoff >= off && phoff + width <= file_end {
            let v = start.checked_add(phoff - off).ok_or_else(no)?;
            if table_vaddr.replace(v).is_some() {
                return Err(no());
            }
        }
        if word(&row, 4) & 7 == 5 {
            rx.push((start..virtual_end, off));
        } // file-backed PF_R|PF_X, never PF_W
    }
    let bias = aux
        .phdr
        .checked_sub(table_vaddr.ok_or_else(no)?)
        .ok_or_else(no)?;
    if bias % aux.page != 0
        || kind == 2 && bias != 0
        || kind == 3 && bias == 0
        || entry.checked_add(bias) != Some(aux.entry)
    {
        return Err(no());
    }
    let offset = |address: u64| -> io::Result<u64> {
        let virtual_address = address.checked_sub(bias).ok_or_else(no)?;
        let mut matches = rx.iter().filter(|(r, _)| r.contains(&virtual_address));
        let (range, off) = matches.next().ok_or_else(no)?;
        if matches.next().is_some() {
            return Err(no());
        }
        off.checked_add(virtual_address - range.start)
            .ok_or_else(no)
    };
    Ok((offset(aux.entry)?, offset(here)?))
}
fn require_code_layout(
    aux: ExecAux,
    bytes: &[u8],
    entry_off: u64,
    code_off: u64,
    here: u64,
    ino: u64,
) -> io::Result<()> {
    let entry = code_map(bytes, aux.entry)?;
    let code = code_map(bytes, here)?;
    if entry.kernel_id != code.kernel_id
        || code.kernel_id.ino != ino
        || entry.offset != entry_off
        || code.offset != code_off
    {
        return Err(no());
    }
    Ok(())
}

#[inline(never)]
fn require_application_text(proc: &ProcessFiles, reader: &File, stamp: Stamp) -> io::Result<()> {
    // The kernel reports the program header/base and entry of the original
    // exe, not a loader's later application. Bind the Helper code pointer
    // and entry to that actual ELF's executable segments and VMA offsets.
    // maps uses superblock device IDs (not the fstat subvolume device on
    // every local filesystem); only compare like kernel VMA identities.
    let aux = exec_aux(&proc.own_table("auxv", MAX_AUXV)?)?;
    let here = require_application_text as *const () as usize as u64;
    let (entry, code) = load_offsets(|o, b| reader.read_exact_at(b, o), stamp.len, aux, here)?;
    require_code_layout(
        aux,
        &proc.own_table("maps", MAX_MAPS)?,
        entry,
        code,
        here,
        stamp.entity.ino,
    )
}
fn check_image_file(reader: &File, stamp: Stamp) -> io::Result<()> {
    local_data_filesystem(reader)?;
    let mut h = [0; 20];
    reader.read_exact_at(&mut h, 0)?;
    if !h.starts_with(b"\x7fELF\x02\x01\x01")
        || !matches!(u16::from_le_bytes([h[16], h[17]]), 2 | 3)
        || h[18..] != [62, 0]
        || fstatvfs(reader)?.f_flag.contains(StatVfsMountFlags::NOEXEC)
    {
        return Err(no());
    }
    for attr in [c"system.posix_acl_access", c"security.capability"] {
        if !matches!(
            fgetxattr(reader, attr, &mut [] as &mut [u8]),
            Err(Errno::NODATA)
        ) {
            return Err(no());
        }
    }
    if Stamp::at(reader)? != stamp {
        return Err(no());
    }
    Ok(())
}

/// Keep this exact selected descriptor for the existing private execveat.
/// No on-disk file is created and no stored Helper authority is consulted.
pub(crate) struct ForegroundElf {
    original: File,
    reader: File,
    stamp: Stamp,
    process: ProcessFiles,
    role: Role,
}
impl ForegroundElf {
    #[cfg(any(test, feature = "embedded-zxs"))]
    pub(crate) fn host() -> io::Result<Self> {
        Self::select(Role::Privileged)
    }
    pub(crate) fn local() -> io::Result<Self> {
        Self::select(Role::Local {
            uid: getuid().as_raw(),
            gid: getgid().as_raw(),
        })
    }
    fn select(role: Role) -> io::Result<Self> {
        role.require()?;
        let process = ProcessFiles::select()?;
        let original = process.exe(getpid())?;
        let stamp = Stamp::at(&original)?;
        if !role.accepts(stamp) {
            return Err(no());
        }
        local_data_filesystem(&original)?;
        let reader = process.open_pinned(&original)?;
        if Stamp::at(&reader)? != stamp {
            return Err(no());
        }
        check_image_file(&reader, stamp)?;
        require_application_text(&process, &reader, stamp)?;
        let result = Self {
            original,
            reader,
            stamp,
            process,
            role,
        };
        result.require()?;
        Ok(result)
    }
    pub(crate) fn descriptor(&self) -> &File {
        &self.original
    }
    pub(crate) fn require(&self) -> io::Result<()> {
        self.role.require()?;
        if Stamp::at(&self.original)? != self.stamp
            || Stamp::at(&self.reader)? != self.stamp
            || Stamp::at(&self.process.exe(getpid())?)? != self.stamp
            || fstatvfs(&self.reader)?
                .f_flag
                .contains(StatVfsMountFlags::NOEXEC)
        {
            return Err(no());
        }
        Ok(())
    }
    /// A later privileged cut still independently attests the *current*
    /// proc, file policy and actual executable mapping. Its original data
    /// reader is already owned; never reopen it or accept another inode.
    #[cfg(any(test, feature = "embedded-zxs"))]
    pub(crate) fn requalify_host_image(&self) -> io::Result<()> {
        if self.role != Role::Privileged {
            return Err(no());
        }
        self.role.require()?;
        let current = ProcessFiles::select()?;
        if current.mount != self.process.mount
            || Entity::of(&current.proc.metadata()?)? != Entity::of(&self.process.proc.metadata()?)?
        {
            return Err(no());
        }
        let same = || -> io::Result<()> {
            self.require()?;
            if Stamp::at(&current.exe(getpid())?)? != self.stamp {
                return Err(no());
            }
            Ok(())
        };
        same()?;
        local_data_filesystem(&self.original)?;
        check_image_file(&self.reader, self.stamp)?;
        require_application_text(&current, &self.reader, self.stamp)?;
        same()
    }
    pub(crate) fn parent_is_same_image(&self, expected: Pid) -> io::Result<()> {
        if getppid() != Some(expected) {
            return Err(no());
        }
        self.require()?;
        if Stamp::at(&self.process.exe(expected)?)? != self.stamp || getppid() != Some(expected) {
            return Err(no());
        }
        self.require()
    }
    #[cfg(any(test, feature = "embedded-zxs"))]
    pub(crate) fn require_independent_digest(&self, expected: &[u8; 32]) -> io::Result<()> {
        if expected == &[0; 32] {
            return Err(no());
        }
        self.require()?;
        let mut hash = Sha256::new();
        let mut buf = vec![0_u8; 32 << 10];
        let mut offset = 0;
        while offset < self.stamp.len {
            let n = usize::try_from(self.stamp.len - offset)
                .unwrap_or(usize::MAX)
                .min(buf.len());
            self.reader.read_exact_at(&mut buf[..n], offset)?;
            hash.update(&buf[..n]);
            offset += n as u64;
        }
        if self.reader.read_at(&mut [0], offset)? != 0
            || <[u8; 32]>::from(hash.finalize()) != *expected
        {
            return Err(no());
        }
        self.require()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn self_elf_kernel_map_uses_exact_exec_mapping_and_not_textual_path() {
        let a=b"0001-0002 r--p 00000000 00:00 0\n2000-3000 r-xp 00001000 08:15 41 /does-not-become-a-path\n";
        assert_eq!(
            code_map(a, 0x2250).unwrap(),
            CodeMap {
                kernel_id: Entity {
                    dev: makedev(8, 21),
                    ino: 41
                },
                offset: 0x1250
            }
        );
        assert!(code_map(a, 0x3000).is_err());
        let mut forged = a.to_vec();
        forged.extend(b"2000-3000 r-xp 00001000 08:15 42 /helper\n");
        assert!(code_map(&forged, 0x2250).is_err());
        assert!(code_map(b"2000-3000 rw-p 0 08:15 41 /helper\n", 0x2250).is_err());
        assert!(code_map(b"2000-3000 r-xp 0 08:15 0 [kernel-vdso]\n", 0x2250).is_err());
    }

    #[test]
    fn self_elf_kernel_main_header_binds_the_code_anchor_without_comparing_filesystem_device_namespaces()
     {
        let base = 0x20_0000u64;
        let aux = ExecAux {
            phdr: base + 64,
            phent: 56,
            phnum: 2,
            entry: base + 4096 + 16,
            page: 4096,
        };
        let synthetic = [
            [3, aux.phdr],
            [4, aux.phent],
            [5, aux.phnum],
            [6, aux.page],
            [9, aux.entry],
            [0, 0],
        ]
        .into_iter()
        .flatten()
        .flat_map(u64::to_le_bytes)
        .collect::<Vec<_>>();
        assert_eq!(exec_aux(&synthetic).unwrap(), aux);
        let mut tail = synthetic.clone();
        tail.extend([0; 16]);
        assert!(exec_aux(&tail).is_err());
        let mut repeated = synthetic.clone();
        repeated[16..24].copy_from_slice(&3u64.to_le_bytes());
        assert!(exec_aux(&repeated).is_err());
        let mut data = vec![0u8; 8192];
        let short = |b: &mut [u8], o: usize, n: u16| {
            b[o..o + 2].copy_from_slice(&n.to_le_bytes());
        };
        let word = |b: &mut [u8], o: usize, n: u32| {
            b[o..o + 4].copy_from_slice(&n.to_le_bytes());
        };
        let wide = |b: &mut [u8], o: usize, n: u64| {
            b[o..o + 8].copy_from_slice(&n.to_le_bytes());
        };
        data[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        short(&mut data, 16, 3);
        short(&mut data, 18, 62);
        word(&mut data, 20, 1);
        wide(&mut data, 24, 4112);
        wide(&mut data, 32, 64);
        short(&mut data, 52, 64);
        short(&mut data, 54, 56);
        short(&mut data, 56, 2);
        for (o, p, flags) in [(64, 0, 4), (120, 4096, 5)] {
            word(&mut data, o, 1);
            word(&mut data, o + 4, flags);
            for at in [8, 16] {
                wide(&mut data, o + at, p);
            }
            for at in [32, 40] {
                wide(&mut data, o + at, 4096);
            }
        }
        let read = |o: u64, b: &mut [u8]| -> io::Result<()> {
            let start = usize::try_from(o).unwrap();
            b.copy_from_slice(&data[start..start + b.len()]);
            Ok(())
        };
        let here = base + 4160;
        let (ep, pc) = load_offsets(read, data.len() as u64, aux, here).unwrap();
        assert_eq!((ep, pc), (4112, 4160));
        assert!(
            load_offsets(
                read,
                data.len() as u64,
                ExecAux {
                    phdr: aux.phdr + 4096,
                    ..aux
                },
                here
            )
            .is_err()
        );
        assert!(load_offsets(read, data.len() as u64, aux, here + 0x8000).is_err());
        let maps=b"201000-201020 r-xp 00001000 00:37 45 /selected\n201020-202000 r-xp 00001020 00:37 45 /name-is-not-used\n";
        // A subvolume fstat device need not be that superblock device.
        require_code_layout(aux, maps, ep, pc, here, 45).unwrap();
        assert!(require_code_layout(aux, maps, ep, pc + 1, here, 45).is_err());
        assert!(require_code_layout(aux, maps, ep, pc, here, 46).is_err());
    }

    #[test]
    fn self_elf_owned_test_process_exe_is_the_mapping_of_this_compiled_code() {
        // The unit-test process only. No launch, protected path or live data.
        let proc = ProcessFiles::select().unwrap();
        let pin = proc.exe(getpid()).unwrap();
        let stamp = Stamp::at(&pin).unwrap();
        let read = proc.open_pinned(&pin).unwrap();
        require_application_text(&proc, &read, stamp).unwrap();
        let foreign = Stamp {
            entity: Entity {
                ino: stamp.entity.ino ^ 1,
                ..stamp.entity
            },
            ..stamp
        };
        assert!(require_application_text(&proc, &read, foreign).is_err());
    }

    #[test]
    fn self_elf_ownership_profiles_reject_unlinked_privileged_or_writable_inputs() {
        let trusted = Stamp {
            entity: Entity { dev: 1, ino: 2 },
            mount: 3,
            mode: 0o100_755,
            uid: 0,
            gid: 0,
            links: 1,
            len: 100,
            ctime: [1, 2],
            mtime: [3, 4],
        };
        let root = Role::Privileged;
        let local = Role::Local {
            uid: 1001,
            gid: 1002,
        };
        assert!(root.accepts(trusted) && local.accepts(trusted));
        let own = Stamp {
            uid: 1001,
            gid: 1002,
            mode: 0o100_700,
            ..trusted
        };
        assert!(local.accepts(own) && !root.accepts(own));
        for bad in [
            Stamp {
                links: 0,
                ..trusted
            },
            Stamp {
                links: 2,
                ..trusted
            },
            Stamp {
                mode: 0o104_755,
                ..trusted
            },
            Stamp {
                mode: 0o100_775,
                ..trusted
            },
            Stamp { uid: 1002, ..own },
            Stamp {
                len: MAX_IMAGE + 1,
                ..trusted
            },
        ] {
            assert!(!root.accepts(bad) && !local.accepts(bad));
        }
    }
}
