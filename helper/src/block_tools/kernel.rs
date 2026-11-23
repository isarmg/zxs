//! Read a fixed set of cached block attributes from the kernel's sysfs. Index
//! symlinks are normal; they must resolve inside this one original sysfs mount.
mod attachment;
use super::*;
use crate::pinned::Mount;
pub(super) use attachment::NvmeOwner;
use rustix::fs::{RawDir, ResolveFlags, fstatfs, openat2, readlinkat};
use std::{ffi::CStr, io::Read, mem::MaybeUninit};

const SYSFS_MAGIC: u64 = 0x6265_6572; // Linux UAPI linux/magic.h
fn path_dir() -> OFlags {
    OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC
}
fn confined() -> ResolveFlags {
    ResolveFlags::BENEATH | ResolveFlags::NO_XDEV | ResolveFlags::NO_MAGICLINKS
}
fn obj(f: &File) -> io::Result<Entity> {
    Entity::of(&f.metadata()?)
}
// Scoped resolution may report EAGAIN while the kernel cannot prove a
// link containing ".." stayed beneath the root. Retry the same lookup once.
fn retry_index_lookup<T>(mut open: impl FnMut() -> rustix::io::Result<T>) -> rustix::io::Result<T> {
    match open() {
        Err(rustix::io::Errno::AGAIN) => open(),
        result => result,
    }
}

fn attr(dir: &File, field: &CStr) -> io::Result<Vec<u8>> {
    let source = File::from(openat2(
        dir,
        field,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
        confined() | ResolveFlags::NO_SYMLINKS,
    )?);
    if !source.metadata()?.is_file() {
        return Err(invalid("unexpected sysfs attribute type"));
    }
    let mut out = Vec::new();
    source.take(4097).read_to_end(&mut out)?;
    if out.len() > 4096 || !out.ends_with(b"\n") {
        return Err(invalid("incomplete or oversized sysfs block attribute"));
    }
    out.pop();
    Ok(out)
}
fn scalar(dir: &File, key: &CStr) -> io::Result<u64> {
    number(&attr(dir, key)?).ok_or_else(|| invalid("unrecognized numeric block attribute"))
}
fn diskseq(dir: &File) -> Field<u64> {
    let n = scalar(dir, c"diskseq").map_err(Reason::io)?;
    if n == 0 { Err(Reason::Invalid) } else { Ok(n) }
}
fn label(raw: &[u8]) -> Field<String> {
    use fmt::Write;
    if raw.is_empty() || raw.len() > 96 {
        return Err(Reason::Invalid);
    }
    let mut s = String::new();
    for &b in raw {
        if b.is_ascii_alphanumeric() || b"._-+:".contains(&b) {
            s.push(char::from(b));
        } else {
            write!(&mut s, "%{b:02X}").expect("String formatting is infallible");
        }
    }
    Ok(s)
}
fn related(dir: &File, relative: &CStr) -> io::Result<File> {
    // Genuine kernel links may traverse ".." within sysfs. Do not cross the
    // mount or a magic link. No arbitrary operator-provided paths enter here.
    Ok(File::from(openat2(
        dir,
        relative,
        path_dir(),
        Mode::empty(),
        ResolveFlags::NO_XDEV | ResolveFlags::NO_MAGICLINKS,
    )?))
}

pub(super) struct Sys {
    root: File,
    index: File,
}
impl Sys {
    pub(super) fn open() -> io::Result<Self> {
        let root = Self::fixed_root()?;
        let index = Self::open_index(&root)?;
        Ok(Self { root, index })
    }

    fn fixed_root() -> io::Result<File> {
        let root = BlockRoot::Sysfs.pin()?;
        if fstatfs(&root)?.f_type as u64 != SYSFS_MAGIC
            || Mount::on(&root)?.is_none_or(|m| m.at_root != Some(true))
        {
            return Err(unsupported(
                "a genuine sysfs mount root with mount identity is required",
            ));
        }
        Ok(root)
    }

    fn open_index(root: &File) -> io::Result<File> {
        Ok(File::from(openat2(
            root,
            c"dev/block",
            path_dir() | OFlags::NOFOLLOW,
            Mode::empty(),
            confined() | ResolveFlags::NO_SYMLINKS,
        )?))
    }

    pub(super) fn require_named(&self) -> io::Result<()> {
        self.require_source(&Self::fixed_root()?)
    }

    fn require_source(&self, named: &File) -> io::Result<()> {
        let held_mount = Mount::on(&self.root)?;
        if held_mount.is_none()
            || Mount::on(named)? != held_mount
            || obj(named)? != obj(&self.root)?
        {
            return Err(invalid("original sysfs mount changed during observation"));
        }
        if obj(&Self::open_index(named)?)? != obj(&self.index)? {
            return Err(invalid(
                "original sysfs block index changed during observation",
            ));
        }
        Ok(())
    }

    pub(super) fn devices(&self) -> io::Result<Vec<Dev>> {
        let dir = File::from(openat2(
            &self.index,
            c".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            confined(),
        )?);
        let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
        let mut scan = RawDir::new(dir, &mut buffer);
        let mut ids = std::collections::BTreeSet::new();
        while let Some(next) = scan.next() {
            let entry = next?;
            let b = entry.file_name().to_bytes();
            if matches!(b, b"." | b"..") {
                continue;
            }
            let d = Dev::parse(b).ok_or_else(|| invalid("unexpected sysfs block index entry"))?;
            if !ids.insert(d) {
                return Err(invalid("repeated block index entry"));
            }
            if ids.len() > MAX_DEVICES {
                return Err(unsupported(
                    "more than 256 block entries; select --device MAJOR:MINOR",
                ));
            }
        }
        Ok(ids.into_iter().collect())
    }

    fn at(&self, dev: Dev) -> io::Result<File> {
        let path = format!("dev/block/{dev}");
        let selected = retry_index_lookup(|| {
            openat2(
                &self.root,
                path.as_str(),
                path_dir(),
                Mode::empty(),
                confined(),
            )
        })?;
        Ok(File::from(selected))
    }
    pub(super) fn pin(&self, dev: Dev) -> io::Result<Node> {
        Ok(Node {
            dir: self.at(dev)?,
            dev,
        })
    }
    fn bus(&self, name: &CStr) -> io::Result<Entity> {
        let f = File::from(openat2(
            &self.root,
            name,
            path_dir(),
            Mode::empty(),
            confined(),
        )?);
        obj(&f)
    }
}

pub(super) struct Node {
    dir: File,
    dev: Dev,
}
impl Node {
    pub(super) fn identity(&self, sys: &Sys) -> io::Result<Identity> {
        let object = obj(&self.dir)?;
        if obj(&sys.at(self.dev)?)? != object
            || Dev::parse(&attr(&self.dir, c"dev")?) != Some(self.dev)
        {
            return Err(invalid(
                "device index no longer denotes this held kernel object",
            ));
        }
        let spelling = self.dev.to_string();
        let target = readlinkat(&sys.index, spelling.as_str(), Vec::new())?;
        let base = target
            .as_bytes()
            .rsplit(|v| *v == b'/')
            .next()
            .unwrap_or_default();
        let name = label(base).map_err(|_| invalid("unusable kernel block name"))?;
        let (kind, seq) = match scalar(&self.dir, c"partition") {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Kind::Disk, diskseq(&self.dir)),
            Err(e) => return Err(e),
            Ok(n) => {
                let number = u32::try_from(n)
                    .ok()
                    .filter(|v| *v > 0)
                    .ok_or_else(|| invalid("invalid partition number"))?;
                let start = scalar(&self.dir, c"start")?;
                let whole = related(&self.dir, c"..")?;
                let parent = Dev::parse(&attr(&whole, c"dev")?)
                    .ok_or_else(|| invalid("invalid partition parent"))?;
                let parent_object = obj(&whole)?;
                if parent == self.dev || obj(&sys.at(parent)?)? != parent_object {
                    return Err(invalid("partition parent differs from the block index"));
                }
                (
                    Kind::Partition {
                        number,
                        start,
                        parent,
                        parent_object,
                    },
                    diskseq(&whole),
                )
            }
        };
        let sectors = scalar(&self.dir, c"size").map_err(Reason::io);
        if obj(&sys.at(self.dev)?)? != object {
            return Err(invalid("block instance changed during observation"));
        }
        Ok(Identity {
            dev: self.dev,
            object,
            name,
            kind,
            sectors,
            diskseq: seq,
        })
    }

    pub(super) fn counts(&self) -> Field<(Instant, Counts)> {
        let bytes = attr(&self.dir, c"stat").map_err(Reason::io)?;
        let time = Instant::now();
        Ok((time, Counts::parse(&bytes)?))
    }

    pub(super) fn model(&self, sys: &Sys, id: &Identity) -> Field<String> {
        if !matches!(id.kind, Kind::Disk) {
            return Err(Reason::NotReported);
        }
        let device = related(&self.dir, c"device").map_err(Reason::io)?;
        let subsystem = related(&device, c"subsystem")
            .and_then(|f| obj(&f))
            .map_err(Reason::io)?;
        // These two upstream show methods return cached model strings. Do not
        // try vendor-specific commands or trigger a device inquiry to fill a gap.
        if ![c"bus/scsi", c"class/nvme", c"class/nvme-subsystem"]
            .iter()
            .any(|p| sys.bus(p).is_ok_and(|cached| cached == subsystem))
        {
            return Err(Reason::Unsupported);
        }
        let raw = attr(&device, c"model").map_err(Reason::io)?;
        let text = label(raw.trim_ascii())?;
        if related(&self.dir, c"device")
            .and_then(|f| obj(&f))
            .map_err(Reason::io)?
            != obj(&device).map_err(Reason::io)?
        {
            return Err(Reason::Changed);
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_lookup_retries_only_again_and_only_once() {
        use rustix::io::Errno;

        for (responses, expected, unused) in [
            ([Ok(1), Ok(2)], Ok(1), Some(Ok(2))),
            ([Err(Errno::AGAIN), Ok(2)], Ok(2), None),
            (
                [Err(Errno::AGAIN), Err(Errno::AGAIN)],
                Err(Errno::AGAIN),
                None,
            ),
            ([Err(Errno::XDEV), Ok(2)], Err(Errno::XDEV), Some(Ok(2))),
        ] {
            let mut supplied = responses.into_iter();
            assert_eq!(retry_index_lookup(|| supplied.next().unwrap()), expected);
            assert_eq!(supplied.next(), unused);
        }
    }

    #[test]
    fn unchanged_index_remains_usable_but_replaced_root_or_index_never_qualifies() {
        use std::fs;
        let area = tempfile::tempdir().unwrap();
        let path = area.path().join("held");
        fs::create_dir_all(path.join("dev/block")).unwrap();
        let root = File::open(&path).unwrap();
        let sys = Sys {
            index: Sys::open_index(&root).unwrap(),
            root,
        };
        sys.require_source(&File::open(&path).unwrap()).unwrap();

        fs::rename(path.join("dev/block"), path.join("dev/old")).unwrap();
        fs::create_dir(path.join("dev/block")).unwrap();
        let err = sys.require_source(&File::open(&path).unwrap()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "original sysfs block index changed during observation"
        );

        fs::rename(&path, area.path().join("original")).unwrap();
        fs::create_dir_all(path.join("dev/block")).unwrap();
        let err = sys.require_source(&File::open(&path).unwrap()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "original sysfs mount changed during observation"
        );
    }

    #[test]
    fn control_bytes_spaces_and_terminal_escapes_never_pass_through_raw() {
        assert_eq!(
            label(b"disk\nA\x1b[3m %v").unwrap(),
            "disk%0AA%1B%5B3m%20%25v"
        );
        assert!(label(&[b'X'; 97]).is_err());
        assert!(label(b"").is_err());
    }
}
