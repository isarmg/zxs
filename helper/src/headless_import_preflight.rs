//! Metadata-only census of an externally selected source directory.
use super::{FreshSelection, parse_paths, refused};
use crate::{
    maintenance::diagnostic_worker,
    pinned::{Entity, Mount, Pinned, require_no_automount},
};
use rustix::{
    fs::{Dir, Mode, OFlags, ResolveFlags, openat2},
    process::{getgid, getuid},
};
use std::{
    ffi::{CStr, CString, OsString},
    fs::File,
    io::{self, Write},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    process::ExitCode,
};

const SCOPED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_XDEV);

#[derive(Debug)]
pub(crate) struct Preflight {
    source: PathBuf,
    roles: Vec<PathBuf>,
}

impl Preflight {
    pub(crate) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        const USAGE: &str = "expected preflight-import SOURCE SYSTEM RECYCLE DATA [DATA ...]";
        let source = PathBuf::from(args.next().ok_or(USAGE)?);
        crate::path_rules::Kind::StorageRoot.check_path(source.as_os_str().as_bytes())?;
        let roles = parse_paths(args)?;
        Ok(Self { source, roles })
    }

    pub(crate) fn run(self) -> io::Result<ExitCode> {
        rustix::thread::set_no_new_privs(true)?;
        diagnostic_worker(getuid().as_raw(), getgid().as_raw())?;
        let selected = FreshSelection::select(&self.roles)?;
        selected.inspect_import_source(&self.source, |observed| {
            let mut out = io::stdout().lock();
            writeln!(
                out,
                "source metadata only: {} files, {} directories ({} empty), {} logical bytes",
                observed.files, observed.dirs, observed.empty_dirs, observed.bytes
            )?;
            writeln!(
                out,
                "hard-linked file entries: {}; counts are by name",
                observed.hard_links
            )?;
            writeln!(
                out,
                "Metadata estimate only; no file contents read or import authorized."
            )?;
            if observed.files == 0 && observed.dirs == 0 {
                writeln!(out, "The selected source is empty.")?;
                return Ok(ExitCode::from(2));
            }
            Ok(ExitCode::SUCCESS)
        })
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct Counts {
    files: u64,
    dirs: u64,
    empty_dirs: u64,
    hard_links: u64,
    bytes: u128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Stamp {
    entity: Entity,
    kind: u32,
    mode: u32,
    uid: u32,
    gid: u32,
    links: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl Stamp {
    fn read(fd: &File, mount: Mount, root_device: u64) -> io::Result<Self> {
        require_no_automount(fd)?;
        let m = fd.metadata()?;
        if Mount::on(fd)?.is_none_or(|current| current.id != mount.id) || m.dev() != root_device {
            return Err(blocked("the source crosses a mount boundary"));
        }
        Ok(Self {
            entity: Entity::of(&m)?,
            kind: m.mode() & 0o170_000,
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
            links: m.nlink(),
            len: m.len(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
        })
    }
}

fn blocked(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn changed() -> io::Error {
    blocked("the original source changed during the metadata walk")
}

fn overlapping(source: &Pinned, original: &Pinned) -> bool {
    source.path().starts_with(original.path())
        || original.path().starts_with(source.path())
        || source.entity_is_visible_ancestor_of(original)
        || original.entity_is_visible_ancestor_of(source)
}

pub(super) fn scan(
    selected: &FreshSelection,
    source: &Path,
    require_roles: impl Fn() -> io::Result<()>,
) -> io::Result<Counts> {
    let pin = Pinned::local_storage(source, &selected.scope)?;
    // Refuse protected roles, aliases, ancestors and descendants. The source
    // is external; this path can never stand in for a missing DATA root.
    if overlapping(&pin, &selected.state.original)
        || selected
            .roots
            .iter()
            .any(|root| overlapping(&pin, &root.original))
    {
        return Err(blocked(
            "the source overlaps the selected state or storage roles",
        ));
    }
    for protected in ["/zxs/state", "/zxs/bin"] {
        let dir = Pinned::local_storage(Path::new(protected), &selected.scope)?;
        if overlapping(&pin, &dir) {
            return Err(blocked("the source overlaps protected deployment paths"));
        }
    }
    let before = pin.require_same_named_mount()?;
    selected.scope.full_local_mount(before.id)?;
    let directory = pin.storage_directory()?;
    let root = Stamp::read(&directory, before, pin.entity().dev)?;
    if root.kind != 0o040_000 || root.entity != pin.entity() {
        return Err(refused());
    }
    let check = || {
        require_roles()?;
        if pin.require_same_named_mount()? != before
            || Stamp::read(&directory, before, root.entity.dev)? != root
        {
            return Err(changed());
        }
        Ok(())
    };
    check()?;
    let counts = census(directory.try_clone()?, root, before)?;
    check()?;
    Ok(counts)
}

fn dir_reader(dir: &File) -> io::Result<File> {
    Ok(File::from(openat2(
        dir,
        c".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOATIME | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        SCOPED,
    )?))
}

fn entry(dir: &File, name: &CStr) -> io::Result<File> {
    match openat2(
        dir,
        name,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        SCOPED,
    ) {
        Ok(fd) => Ok(File::from(fd)),
        Err(rustix::io::Errno::XDEV) => Err(blocked("the source crosses a mount boundary")),
        Err(error) => Err(error.into()),
    }
}

struct Frame {
    // Retain the actual held directory and its exact stream; no reopening
    // a different pathname or retaining names from across a directory.
    original: File,
    stream: Dir,
    stamp: Stamp,
    width: usize,
    parent_name: Option<CString>,
    saw_member: bool,
}

impl Frame {
    fn open(
        original: File,
        stamp: Stamp,
        mount: Mount,
        width: usize,
        name: Option<CString>,
    ) -> io::Result<Self> {
        let reader = dir_reader(&original)?;
        if Stamp::read(&reader, mount, stamp.entity.dev)? != stamp {
            return Err(changed());
        }
        let stream = Dir::new(reader)?;
        Ok(Self {
            original,
            stream,
            stamp,
            width,
            parent_name: name,
            saw_member: false,
        })
    }
}

fn add_count(value: &mut u64) -> io::Result<()> {
    *value = value
        .checked_add(1)
        .ok_or_else(|| blocked("source count exceeds its numeric range"))?;
    Ok(())
}

fn census(dir: File, root: Stamp, mount: Mount) -> io::Result<Counts> {
    let mut frames = vec![Frame::open(dir, root, mount, 0, None)?];
    let mut counts = Counts::default();
    // Iterative depth-first walk; open descriptors and memory follow path
    // depth, never the total number of entries or siblings.
    while let Some(frame) = frames.last_mut() {
        let Some(next) = frame.stream.read() else {
            if Stamp::read(&frame.original, mount, root.entity.dev)? != frame.stamp {
                return Err(changed());
            }
            let Some(done) = frames.pop() else {
                return Err(changed());
            };
            if let Some(name) = done.parent_name {
                let parent = frames.last().ok_or_else(changed)?;
                let final_name = entry(&parent.original, &name)?;
                if Stamp::read(&final_name, mount, root.entity.dev)? != done.stamp {
                    return Err(changed());
                }
                if !done.saw_member {
                    add_count(&mut counts.empty_dirs)?;
                }
            }
            continue;
        };
        let row = next?;
        let name = row.file_name();
        let raw = name.to_bytes();
        if matches!(raw, b"." | b"..") {
            continue;
        }
        frame.saw_member = true;
        let width = frame.width + usize::from(frame.width != 0) + raw.len();
        if width > crate::path_rules::MAX_PATH_BYTES {
            return Err(blocked("a relative source path is too long"));
        }
        let pinned = entry(&frame.original, name)?;
        let one = Stamp::read(&pinned, mount, root.entity.dev)?;
        if one.entity.ino != row.ino() {
            return Err(changed());
        }
        match one.kind {
            0o040_000 => {
                add_count(&mut counts.dirs)?;
                let child = Frame::open(pinned, one, mount, width, Some(name.to_owned()))?;
                frames.push(child);
            }
            0o100_000 => {
                let named = entry(&frame.original, name)?;
                if Stamp::read(&named, mount, root.entity.dev)? != one
                    || Stamp::read(&pinned, mount, root.entity.dev)? != one
                {
                    return Err(changed());
                }
                add_count(&mut counts.files)?;
                if one.links > 1 {
                    add_count(&mut counts.hard_links)?;
                }
                counts.bytes = counts
                    .bytes
                    .checked_add(u128::from(one.len))
                    .ok_or_else(|| blocked("source bytes exceed their numeric range"))?;
            }
            _ => {
                return Err(blocked(
                    "the source contains a symbolic link or special object",
                ));
            }
        }
    }
    Ok(counts)
}

#[cfg(test)]
#[path = "../tests/maintenance/import_preflight_args.rs"]
mod tests;
