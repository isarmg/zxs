//! Only fixed distribution slots. An observed file is not a trusted release.
//! Optional verification compares the entire regular file with the operator's
//! independent expected hash. The hash is never used to grant execution.
use super::Profile;
use crate::pinned::{Entity, LocalDataFile, Mount};
use rustix::fs::{Mode, OFlags, open, openat};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::PathBuf,
};

const MAX_HASH: u64 = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Record {
    MetadataOnly,
    Equal,
    Different,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Evidence {
    Observed(Record),
    Missing,
    Refused,
    Changed,
    Denied,
    Unavailable,
    DigestUnavailable,
}
impl Evidence {
    pub(crate) fn slot(self) -> &'static str {
        match self {
            Self::Observed(_) | Self::DigestUnavailable => "protected-metadata",
            Self::Missing => "absent-at-fixed-slot",
            Self::Refused => "ineligible",
            Self::Changed => "changed",
            Self::Denied => "permission-denied",
            Self::Unavailable => "unknown",
        }
    }
    pub(super) fn digest(self) -> &'static str {
        match self {
            Self::Observed(Record::MetadataOnly) => "not-requested",
            Self::Observed(Record::Equal) => "matches-reference",
            Self::Observed(Record::Different) => "differs-from-reference",
            Self::DigestUnavailable => "unavailable",
            _ => "not-verified",
        }
    }
    fn from_io(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::PermissionDenied => Self::Denied,
            _ => Self::Unavailable,
        }
    }
}
type Result<T> = std::result::Result<T, Evidence>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mark {
    entity: Entity,
    mount: u64,
    mode: u32,
    owner: u32,
    links: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
impl Mark {
    fn on(fd: &File) -> Result<Self> {
        let m = fd.metadata().map_err(Evidence::from_io)?;
        Ok(Self {
            entity: Entity::of(&m).map_err(Evidence::from_io)?,
            mount: Mount::on(fd)
                .map_err(Evidence::from_io)?
                .ok_or(Evidence::Unavailable)?
                .id,
            mode: m.mode(),
            owner: m.uid(),
            links: m.nlink(),
            size: m.len(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
        })
    }
    fn accepts(self, directory: bool) -> bool {
        if self.owner != 0 || self.mode & 0o022 != 0 || self.links == 0 {
            return false;
        }
        if directory {
            self.mode & 0o170_000 == 0o040_000
        } else {
            self.regular() && self.mode & 0o111 != 0
        }
    }
    fn regular(self) -> bool {
        self.owner == 0
            && self.mode & 0o6022 == 0
            && self.links > 0
            && self.mode & 0o170_000 == 0o100_000
            && self.size > 0
    }
}

struct Slot {
    fds: Vec<File>,
    marks: Vec<Mark>,
}
impl Slot {
    fn unchanged(&self) -> bool {
        self.fds
            .iter()
            .zip(&self.marks)
            .all(|(f, m)| Mark::on(f).is_ok_and(|now| now == *m))
    }
}
enum Lookup {
    Present(Slot),
    Absent { parent: Slot, component: usize },
}
fn path_flags() -> OFlags {
    OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC
}
fn lookup(parts: &[&str], executable: bool) -> Result<Lookup> {
    let mut held = Slot {
        fds: Vec::with_capacity(4),
        marks: Vec::with_capacity(4),
    };
    let root = File::from(
        open(c"/", path_flags() | OFlags::DIRECTORY, Mode::empty())
            .map_err(|e| Evidence::from_io(e.into()))?,
    );
    let m = Mark::on(&root)?;
    if !m.accepts(true) {
        return Err(Evidence::Refused);
    }
    held.fds.push(root);
    held.marks.push(m);
    for (index, &name) in parts.iter().enumerate() {
        // Only an actual component lookup, never an fstat/statx error, can report absence.
        let f = match openat(
            held.fds.last().expect("root present"),
            name,
            path_flags(),
            Mode::empty(),
        ) {
            Ok(fd) => File::from(fd),
            Err(rustix::io::Errno::NOENT) => {
                return Ok(Lookup::Absent {
                    parent: held,
                    component: index,
                });
            }
            Err(e) => return Err(Evidence::from_io(e.into())),
        };
        let m = Mark::on(&f)?;
        if if index + 1 < parts.len() {
            !m.accepts(true)
        } else {
            !m.regular() || executable && !m.accepts(false)
        } {
            return Err(Evidence::Refused);
        }
        held.fds.push(f);
        held.marks.push(m);
    }
    Ok(Lookup::Present(held))
}

// The second walk must miss the SAME component beneath the SAME named and
// still-held parent chain. ENOENT for an unlinked or superseded /usr/sbin
// does not mean a program is missing from the host's current installation.
fn confirm_absence(
    original: &Slot,
    component: usize,
    again: Result<Lookup>,
    name: &str,
) -> Evidence {
    let Ok(Lookup::Absent {
        parent,
        component: again,
    }) = again
    else {
        return Evidence::Changed;
    };
    if again != component || original.marks != parent.marks {
        return Evidence::Changed;
    }
    let last = original
        .fds
        .last()
        .expect("absence always has a held parent");
    if !matches!(
        openat(last, name, path_flags(), Mode::empty()),
        Err(rustix::io::Errno::NOENT)
    ) || !original.unchanged()
        || !parent.unchanged()
    {
        return Evidence::Changed;
    }
    Evidence::Missing
}
fn full_hash(file: &mut impl Read, size: u64) -> io::Result<[u8; 32]> {
    hash_upto(file, size, MAX_HASH)
}
/// Pure bounded hashing. The diagnostic command keeps its fixed 64 MiB limit;
/// separately curated roles can choose a different fixed limit on a held image.
pub(crate) fn hash_upto(file: &mut impl Read, size: u64, max: u64) -> io::Result<[u8; 32]> {
    if size == 0 || size > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file outside hash budget",
        ));
    }
    let mut hash = Sha256::new();
    let mut left = size;
    let mut buf = vec![0; 32 * 1024];
    while left != 0 {
        let n = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
        file.read_exact(&mut buf[..n])?;
        hash.update(&buf[..n]);
        left -= n as u64;
    }
    loop {
        match file.read(&mut buf[..1]) {
            Ok(0) => return Ok(hash.finalize().into()),
            Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "size changed")),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

pub(super) fn inspect(p: &Profile, expected: Option<&[u8; 32]>) -> Evidence {
    let parts = p.slots();
    let first = match lookup(&parts, true) {
        Ok(Lookup::Present(s)) => s,
        Ok(Lookup::Absent { parent, component }) => {
            return confirm_absence(&parent, component, lookup(&parts, true), parts[component]);
        }
        Err(e) => return e,
    };
    let leaf = *first.marks.last().expect("fixed leaf");
    // Only an explicit reference requests content access. Select via the existing
    // local-regular-file boundary; never retry on a pseudo/unknown FS or without NOATIME.
    let result = expected.map_or(Evidence::Observed(Record::MetadataOnly), |h| {
        let read = (|| {
            if leaf.size > MAX_HASH {
                return Err(Evidence::DigestUnavailable);
            }
            let path = PathBuf::from("/").join(parts.join("/"));
            let selected = LocalDataFile::select(&path).map_err(|_| Evidence::DigestUnavailable)?;
            let pre = selected
                .metadata()
                .map_err(|_| Evidence::DigestUnavailable)?;
            if Entity::of(&pre).map_err(|_| Evidence::DigestUnavailable)? != leaf.entity {
                return Err(Evidence::Changed);
            }
            let mut f = selected.reader().map_err(|_| Evidence::DigestUnavailable)?;
            if Mark::on(&f).map_err(|_| Evidence::DigestUnavailable)? != leaf {
                return Err(Evidence::Changed);
            }
            let actual = full_hash(&mut f, leaf.size).map_err(|_| Evidence::DigestUnavailable)?;
            if Mark::on(&f).map_err(|_| Evidence::DigestUnavailable)? != leaf
                || selected.require_same_path().is_err()
            {
                return Err(Evidence::Changed);
            }
            Ok(Evidence::Observed(if actual.eq(h) {
                Record::Equal
            } else {
                Record::Different
            }))
        })();
        read.unwrap_or_else(|e| e)
    });
    let Ok(Lookup::Present(now)) = lookup(&parts, true) else {
        return Evidence::Changed;
    };
    if first.marks != now.marks || !first.unchanged() || !now.unchanged() {
        return Evidence::Changed;
    }
    result
}

/// A fixed host installation input, not an executable selector from the request.
/// These bytes are public runtime components: ordinary readonly access may
/// update their atime. User images still require the original NOATIME boundary.
pub(crate) struct RuntimeFile {
    chain: Slot,
    pub(crate) file: File,
    parts: &'static [&'static str],
    executable: bool,
}
impl RuntimeFile {
    pub(crate) fn open(
        parts: &'static [&'static str],
        executable: bool,
        max: u64,
    ) -> io::Result<Self> {
        let unavailable =
            || io::Error::other("fixed diagnostic installation missing, unprotected or changed");
        let Lookup::Present(chain) = lookup(parts, executable).map_err(|_| unavailable())? else {
            return Err(unavailable());
        };
        let last = chain.fds.last().ok_or_else(unavailable)?;
        let mark = chain.marks.last().ok_or_else(unavailable)?;
        if mark.size > max {
            return Err(unavailable());
        }
        let file = crate::pinned::installed_regular_reader(last)?;
        if Mark::on(&file).ok().as_ref() != Some(mark) {
            return Err(unavailable());
        }
        let result = Self {
            chain,
            file,
            parts,
            executable,
        };
        result.recheck()?;
        Ok(result)
    }
    pub(crate) fn recheck(&self) -> io::Result<()> {
        let ok = match lookup(self.parts, self.executable) {
            Ok(Lookup::Present(now)) => {
                self.chain.marks == now.marks
                    && self.chain.unchanged()
                    && now.unchanged()
                    && Mark::on(&self.file).ok().as_ref() == self.chain.marks.last()
            }
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(io::Error::other(
                "protected diagnostic installation changed",
            ))
        }
    }
    pub(crate) fn digest(&mut self) -> io::Result<[u8; 32]> {
        use std::io::Seek;
        self.file.rewind()?;
        let expected_len = self.file.metadata()?.len();
        let hash = full_hash(&mut self.file, expected_len)?;
        self.recheck()?;
        self.file.rewind()?;
        Ok(hash)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_from_a_replaced_parent_is_not_confirmed_absence() {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let named = tmp.path().join("bin");
        fs::create_dir(&named).unwrap();
        let hold = || {
            let f =
                File::from(open(&named, path_flags() | OFlags::DIRECTORY, Mode::empty()).unwrap());
            let stamp = Mark::on(&f).unwrap();
            Slot {
                fds: vec![f],
                marks: vec![stamp],
            }
        };
        let original = hold();
        let miss = || {
            Ok(Lookup::Absent {
                parent: hold(),
                component: 2,
            })
        };
        assert_eq!(
            confirm_absence(&original, 2, miss(), "checker"),
            Evidence::Missing
        );
        fs::rename(&named, tmp.path().join("old")).unwrap();
        fs::create_dir(&named).unwrap();
        assert_eq!(
            confirm_absence(&original, 2, miss(), "checker"),
            Evidence::Changed
        );
        fs::write(
            named.join("checker"),
            b"new file in the named parent, not the old one",
        )
        .unwrap();
        let replacement = hold();
        assert_eq!(
            confirm_absence(&replacement, 2, miss(), "checker"),
            Evidence::Changed
        ); // even two reported misses need a real final lookup
    }
    #[test]
    fn use_complete_bytes_and_eof_only() {
        let expected =
            super::super::hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
                .unwrap();
        assert_eq!(full_hash(&mut &b"abc"[..], 3).unwrap(), expected);
        assert!(full_hash(&mut &b"ab"[..], 3).is_err());
        assert!(full_hash(&mut &b"abcd"[..], 3).is_err());
        for size in [0, MAX_HASH + 1] {
            assert!(full_hash(&mut &b""[..], size).is_err());
        }
    }
    #[test]
    fn protected_stage_never_classifies_a_symlink_fifo_device_or_privileged_image_as_the_tool() {
        let m = Mark {
            entity: Entity { dev: 1, ino: 9 },
            mount: 12,
            mode: 0o100_755,
            owner: 0,
            links: 1,
            size: 8,
            mtime: (2, 1),
            ctime: (2, 1),
        };
        assert!(m.accepts(false));
        for mode in [
            0o120_777, 0o010_755, 0o060_755, 0o100_775, 0o104_755, 0o102_755, 0o100_644,
        ] {
            assert!(!Mark { mode, ..m }.accepts(false));
        }
        assert!(!Mark { owner: 1000, ..m }.accepts(false));
        assert!(
            Mark {
                mode: 0o040_755,
                ..m
            }
            .accepts(true)
        );
        assert!(
            !Mark {
                mode: 0o040_777,
                ..m
            }
            .accepts(true)
        );
    }
}
