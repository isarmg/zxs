//! Metadata-only census and optional naming preview of an external source.
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
    ffi::{CStr, CString, OsStr, OsString},
    fs::File,
    io::{self, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
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
    show_names: bool,
    check_target: bool,
}

impl Preflight {
    pub(crate) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        const USAGE: &str = "expected preflight-import [--show-names | --check-target] SOURCE SYSTEM RECYCLE DATA [DATA ...]";
        let first = args.next().ok_or(USAGE)?;
        let check_target = first == "--check-target";
        let show_names = check_target || first == "--show-names";
        let source = PathBuf::from(if show_names {
            args.next().ok_or(USAGE)?
        } else {
            first
        });
        crate::path_rules::Kind::StorageRoot.check_path(source.as_os_str().as_bytes())?;
        let roles = parse_paths(args)?;
        Ok(Self {
            source,
            roles,
            show_names,
            check_target,
        })
    }

    pub(crate) fn run(self) -> io::Result<ExitCode> {
        rustix::thread::set_no_new_privs(true)?;
        diagnostic_worker(getuid().as_raw(), getgid().as_raw())?;
        let selected = FreshSelection::select(&self.roles)?;
        let mut preview = self.show_names.then(Vec::new);
        selected.inspect_import_source(&self.source, preview.as_mut(), self.check_target, |observed, rows| {
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
                "source naming: {} nested files need an explicit mapping; {} top-level names cannot be used as-is",
                observed.nested_files, observed.invalid_top_level_names
            )?;
            if self.check_target {
                writeln!(
                    out,
                    "target naming: {} top-level files match existing exact container names",
                    observed.target_collisions
                )?;
                writeln!(out, "Metadata only; Workspace names are not checked.")?;
            } else {
                writeln!(
                    out,
                    "Metadata only; destination collisions and Workspace names are not checked."
                )?;
            }
            if observed.files == 0 && observed.dirs == 0 {
                writeln!(out, "The selected source is empty.")?;
                return Ok(ExitCode::from(2));
            }
            if let Some(rows) = rows {
                writeln!(out, "name preview: source path, status, proposed display name")?;
                out.write_all(rows)?;
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
    nested_files: u64,
    invalid_top_level_names: u64,
    target_collisions: u64,
    bytes: u128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Stamp {
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

fn same_or_nested((left, a): (&Path, Entity), (right, b): (&Path, Entity)) -> bool {
    a == b || left.starts_with(right) || right.starts_with(left)
}

fn overlapping(source: &Pinned, original: &Pinned) -> bool {
    // Visible-ancestor tests omit the leaf; a differently named whole-mount
    // alias of the very same role or deployment directory is still protected.
    same_or_nested(
        (source.path(), source.entity()),
        (original.path(), original.entity()),
    ) || source.entity_is_visible_ancestor_of(original)
        || original.entity_is_visible_ancestor_of(source)
}

pub(super) fn pin_source(selected: &FreshSelection, source: &Path) -> io::Result<Pinned> {
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
    Ok(pin)
}

pub(super) fn scan(
    selected: &FreshSelection,
    source: &Path,
    preview: Option<&mut Vec<u8>>,
    target_names: Option<&[String]>,
    require_roles: impl Fn() -> io::Result<()>,
) -> io::Result<Counts> {
    SourceTree::open(selected, source)?.observe(preview, target_names, None, require_roles)
}

/// Keep the same root and mounted directory across every census and fingerprint.
pub(super) struct SourceTree {
    pin: Pinned,
    mount: Mount,
    directory: File,
    root: Stamp,
}

#[derive(Eq, PartialEq)]
pub(super) struct Inventory {
    counts: Counts,
    entries: Vec<(OsString, Stamp)>, // includes directory names and witnesses
}

impl Inventory {
    pub(super) fn files(&self) -> impl Iterator<Item = (&OsStr, &Stamp)> {
        self.entries.iter().filter_map(|(path, stamp)| {
            (stamp.kind == 0o100_000).then_some((path.as_os_str(), stamp))
        })
    }

    pub(super) fn report(&self, out: &mut impl Write) -> io::Result<()> {
        writeln!(
            out,
            "source census: {} files, {} logical bytes, {} directories ({} empty); directories stay on the source; {} hard-linked entries would be imported by name",
            self.counts.files,
            self.counts.bytes,
            self.counts.dirs,
            self.counts.empty_dirs,
            self.counts.hard_links
        )
    }
}

impl SourceTree {
    pub(super) fn open(selected: &FreshSelection, source: &Path) -> io::Result<Self> {
        let pin = pin_source(selected, source)?;
        let mount = pin.require_same_named_mount()?;
        selected.scope.full_local_mount(mount.id)?;
        let directory = pin.storage_directory()?;
        let root = Stamp::read(&directory, mount, pin.entity().dev)?;
        if root.kind != 0o040_000 || root.entity != pin.entity() {
            return Err(refused());
        }
        Ok(Self {
            pin,
            mount,
            directory,
            root,
        })
    }

    fn verify(&self) -> io::Result<()> {
        if self.pin.require_same_named_mount()? != self.mount
            || Stamp::read(&self.directory, self.mount, self.root.entity.dev)? != self.root
        {
            return Err(changed());
        }
        Ok(())
    }

    /// The actual FD selected for hashing, not just later totals or a display path.
    pub(super) fn require_file(
        &self,
        root: &Pinned,
        mount: Mount,
        held: &File,
        stamp: &Stamp,
    ) -> io::Result<()> {
        self.verify()?;
        if self.pin.entity() != root.entity()
            || self.mount != mount
            || Stamp::read(held, self.mount, self.root.entity.dev)? != *stamp
        {
            return Err(changed());
        }
        Ok(())
    }

    fn observe(
        &self,
        preview: Option<&mut Vec<u8>>,
        target_names: Option<&[String]>,
        entries: Option<&mut Vec<(OsString, Stamp)>>,
        require_roles: impl Fn() -> io::Result<()>,
    ) -> io::Result<Counts> {
        require_roles()?;
        self.verify()?;
        let counts = census(
            self.directory.try_clone()?,
            self.root,
            self.mount,
            preview,
            target_names,
            entries,
        )?;
        self.verify()?;
        require_roles()?;
        Ok(counts)
    }

    pub(super) fn inventory(
        &self,
        require_roles: impl Fn() -> io::Result<()>,
    ) -> io::Result<Inventory> {
        let mut entries = Vec::new();
        let counts = self.observe(None, None, Some(&mut entries), require_roles)?;
        entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(changed());
        }
        Ok(Inventory { counts, entries })
    }
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

// Match the server's display component for same-name candidates only.
// Workspace path portability and collisions belong to the conversion plan.
pub(super) fn same_name_candidate(raw: &[u8]) -> bool {
    const MAX: usize = 255;
    if raw.is_empty() || raw.len() > MAX {
        return false;
    }
    let Ok(name) = std::str::from_utf8(raw) else {
        return false;
    };
    !matches!(name, "." | "..")
        && !name.ends_with(['.', ' '])
        && name.encode_utf16().count() <= MAX
        && !name.chars().any(|c| {
            c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
}

fn census(
    dir: File,
    root: Stamp,
    mount: Mount,
    mut preview: Option<&mut Vec<u8>>,
    target_names: Option<&[String]>,
    mut entries: Option<&mut Vec<(OsString, Stamp)>>,
) -> io::Result<Counts> {
    let mut frames = vec![Frame::open(dir, root, mount, 0, None)?];
    let mut counts = Counts::default();
    // Iterative depth-first walk; open descriptors and memory follow path
    // depth; only explicit preview or full-tree selection buffers row data.
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
                if let Some(rows) = entries.as_deref_mut() {
                    record_entry(rows, &frames, &name, done.width, done.stamp)?;
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
                let nested = frame.parent_name.is_some();
                let candidate = !nested && same_name_candidate(raw);
                if nested {
                    add_count(&mut counts.nested_files)?;
                } else if !candidate {
                    add_count(&mut counts.invalid_top_level_names)?;
                }
                let status = name_status(raw, candidate, target_names);
                if matches!(status, PreviewName::Occupied) {
                    add_count(&mut counts.target_collisions)?;
                }
                if let Some(rows) = preview.as_deref_mut() {
                    write_preview_row(
                        rows,
                        frames.iter().filter_map(|part| part.parent_name.as_deref()),
                        name,
                        width,
                        status,
                    )?;
                }
                if let Some(rows) = entries.as_deref_mut() {
                    record_entry(rows, &frames, name, width, one)?;
                }
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

fn record_entry(
    rows: &mut Vec<(OsString, Stamp)>,
    parents: &[Frame],
    name: &CStr,
    width: usize,
    stamp: Stamp,
) -> io::Result<()> {
    let allocation = |_| io::Error::from(io::ErrorKind::OutOfMemory);
    let mut raw = Vec::new();
    raw.try_reserve_exact(width).map_err(allocation)?;
    for component in parents.iter().filter_map(|f| f.parent_name.as_deref()) {
        raw.extend_from_slice(component.to_bytes());
        raw.push(b'/');
    }
    raw.extend_from_slice(name.to_bytes());
    rows.try_reserve(1).map_err(allocation)?;
    rows.push((OsString::from_vec(raw), stamp));
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreviewName {
    MappingRequired,
    SameName,
    Occupied,
    NotFound,
}

fn name_status(raw: &[u8], candidate: bool, target: Option<&[String]>) -> PreviewName {
    if !candidate {
        return PreviewName::MappingRequired;
    }
    let Some(target) = target else {
        return PreviewName::SameName;
    };
    if target
        .binary_search_by(|name| name.as_bytes().cmp(raw))
        .is_ok()
    {
        PreviewName::Occupied
    } else {
        PreviewName::NotFound
    }
}

fn write_preview_row<'a>(
    out: &mut Vec<u8>,
    parents: impl Iterator<Item = &'a CStr>,
    name: &CStr,
    path_width: usize,
    status: PreviewName,
) -> io::Result<()> {
    // Each byte expands to at most four ASCII bytes. The scanner bounds the
    // input path; reserve before writing and fail without a partial report.
    out.try_reserve(4 * (path_width + name.to_bytes().len()) + 32)
        .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
    for parent in parents {
        append_escaped(out, parent.to_bytes());
        out.push(b'/');
    }
    append_escaped(out, name.to_bytes());
    let field: &[u8] = match status {
        PreviewName::MappingRequired => b"\tmapping-required\t",
        PreviewName::SameName => b"\tsame-name\t",
        PreviewName::Occupied => b"\ttarget-name-exists\t",
        PreviewName::NotFound => b"\tno-exact-target-name\t",
    };
    out.extend_from_slice(field);
    if status != PreviewName::MappingRequired {
        append_escaped(out, name.to_bytes());
    }
    out.push(b'\n');
    Ok(())
}

fn append_escaped(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend(bytes.iter().copied().flat_map(std::ascii::escape_default));
}

#[cfg(test)]
#[path = "../tests/maintenance/import_preflight_args.rs"]
mod tests;
