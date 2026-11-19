//! Resolve inputs using held directories, without adopting a replacement at the same spelling.
pub(crate) mod storage_status;
pub(crate) mod portable_pending;
pub(crate) use crate::path_rules::Kind;

use rustix::fs::{AtFlags, Mode, OFlags, PROC_SUPER_MAGIC, StatxAttributes, StatxFlags, fstatfs, open, openat, readlinkat, statx};
use std::ffi::OsStr;
use std::fs::{File, Metadata};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(crate) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) struct Entity { pub dev: u64, pub ino: u64 }
impl Entity {
    pub(crate) fn of(m: &Metadata) -> io::Result<Self> {
        if m.ino() == 0 { return Err(invalid("filesystem did not supply an inode identity")); }
        Ok(Self { dev: m.dev(), ino: m.ino() })
    }
}

/// Kernel observation for this held mount now (not a persistent filesystem UUID).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) struct Mount { pub(crate) id: u64, pub(crate) at_root: Option<bool> }
impl Mount {
    pub(crate) fn on(fd: &File) -> io::Result<Option<Self>> {
        let s = statx(fd, c"", AtFlags::EMPTY_PATH | AtFlags::NO_AUTOMOUNT | AtFlags::STATX_DONT_SYNC,
                      StatxFlags::MNT_ID)?;
        if s.stx_mask & StatxFlags::MNT_ID.bits() == 0 || s.stx_mnt_id == 0 { return Ok(None); }
        Ok(Some(Self { id: s.stx_mnt_id, at_root: s.stx_attributes_mask.contains(StatxAttributes::MOUNT_ROOT)
            .then(|| s.stx_attributes.contains(StatxAttributes::MOUNT_ROOT)) }))
    }
}

pub(crate) struct Pinned {
    path: PathBuf,
    kind: Kind,
    fd: File,
    chain: Vec<Entity>,
}

fn dir_flags() -> OFlags {
    OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

impl Pinned {
    pub(crate) fn open(path: &Path, kind: Kind) -> io::Result<Self> {
        let raw = path.as_os_str().as_bytes();
        kind.check_path(raw).map_err(invalid)?;
        let mut names = raw[1..].split(|b| *b == b'/').peekable();
        let mut fd = File::from(open("/", dir_flags(), Mode::empty())?);
        let mut chain = vec![Entity::of(&fd.metadata()?)?];
        if raw == b"/" {
            return Ok(Self { path: path.to_owned(), kind, fd, chain });
        }
        while let Some(name) = names.next() {
            let last_file = names.peek().is_none() && matches!(kind, Kind::Regular);
            let flags = if last_file {
                OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC
            } else { dir_flags() };
            let child = File::from(openat(&fd, OsStr::from_bytes(name), flags, Mode::empty())?);
            let meta = child.metadata()?;
            if (last_file && !meta.is_file()) || (!last_file && !meta.is_dir()) {
                return Err(invalid("path must have the requested type; symbolic links and special files are refused"));
            }
            let entity = Entity::of(&meta)?;
            if chain.contains(&entity) { return Err(invalid("aliased ancestor in selected path")); }
            chain.push(entity);
            fd = child;
        }
        Ok(Self { path: path.to_owned(), kind, fd, chain })
    }

    pub(crate) fn entity(&self) -> Entity { *self.chain.last().expect("root and leaf are present") }
    pub(crate) fn visible_chain(&self) -> &[Entity] { &self.chain }
    pub(crate) fn entity_is_visible_ancestor_of(&self, other: &Self) -> bool {
        other.chain[..other.chain.len() - 1].contains(&self.entity())
    }
    pub(crate) fn metadata(&self) -> io::Result<Metadata> { self.fd.metadata() }

    pub(crate) fn path(&self) -> &Path { &self.path }

    /// Return an independent, real open description for this selected storage
    /// directory. Never lock a duplicate of somebody else's open description.
    pub(crate) fn storage_directory(&self) -> io::Result<File> {
        if !matches!(self.kind, Kind::StorageRoot) { return Err(invalid("expected a selected storage root")); }
        let now = Self::open(&self.path, self.kind)?;
        let original_mount = Mount::on(&self.fd)?.ok_or_else(|| invalid("original mount is unavailable"))?;
        if self.chain != now.chain || Mount::on(&now.fd)?.is_none_or(|m| m.id != original_mount.id) {
            return Err(invalid("original storage path, visible ancestry or mount changed"));
        }
        // The fresh name is evidence only. Open "." relative to the FIRST held
        // directory, so revalidation never changes the object receiving the open.
        let f = File::from(openat(&self.fd, c".", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW
            | OFlags::NOATIME | OFlags::CLOEXEC, Mode::empty())?);
        if self.entity() != Entity::of(&f.metadata()?)?
            || Mount::on(&f)?.is_none_or(|m| m.id != original_mount.id) {
            return Err(invalid("opened description does not designate the selected storage directory"));
        }
        Ok(f)
    }

    pub(crate) fn require_same_path(&self) -> io::Result<()> {
        let now = Self::open(&self.path, self.kind)?;
        if now.chain != self.chain { return Err(invalid("selected path or parent changed during observation")); }
        Ok(())
    }
}

/// Keeps the original selected comparison operand and its admitted filesystem.
pub(crate) struct LocalDataFile { pin: Pinned, magic: u32 }
impl LocalDataFile {
    pub(crate) fn select(path: &Path) -> io::Result<Self> {
        let pin = Pinned::open(path, Kind::Regular)?;
        let magic = local_data_filesystem(&pin.fd)?;
        Ok(Self { pin, magic })
    }

    pub(crate) fn metadata(&self) -> io::Result<Metadata> { self.pin.metadata() }
    pub(crate) fn require_same_path(&self) -> io::Result<()> { self.pin.require_same_path() }

    // Selection keeps the admitted regular-file handle alive. Reopening the operand's
    // pathname instead could read a different object or open a device or FIFO.
    pub(crate) fn reader(&self) -> io::Result<File> {
        reopen_admitted_reader(&self.pin.fd, self.pin.entity(), ReaderBoundary::DataFilesystem(self.magic))
    }
}

/// Upgrade an original O_PATH private regular inode only; never use its mutable pathname.
/// The caller must independently recheck the original name, held birth, protocol and lock.
pub(crate) fn private_inode_reader(pin: &File, uid: u32, expected_mount: u64) -> io::Result<File> {
    let m = pin.metadata()?;
    if !rustix::fs::fcntl_getfl(pin)?.contains(OFlags::PATH) || !m.is_file()
        || m.uid() != uid || m.nlink() != 1 || m.mode() & 0o7777 != 0o600
        || Mount::on(pin)?.is_none_or(|v| v.id != expected_mount) {
        return Err(invalid("expected a singly linked private file pinned on its original mount"));
    }
    let file = reopen_admitted_reader(pin, Entity::of(&m)?, ReaderBoundary::DataFilesystem(local_data_filesystem(pin)?))?;
    if Mount::on(&file)?.is_none_or(|v| v.id != expected_mount) {
        return Err(invalid("opened file no longer has the selected mount"));
    }
    Ok(file)
}

// Unknown comparison inputs still have their prior filesystem allowlist. The other
// caller only reads fixed private control names below a separately selected, held
// storage directory. A current mount ID binds its descriptors, not its provenance.
#[derive(Copy, Clone)]
enum ReaderBoundary { DataFilesystem(u32), SelectedControlMount(u64) }
impl ReaderBoundary {
    fn matches(self, fd: &File) -> io::Result<bool> {
        match self {
            Self::DataFilesystem(magic) => Ok(filesystem(fd)? == Some(magic)),
            Self::SelectedControlMount(id) => Ok(Mount::on(fd)?.is_some_and(|m| m.id == id)),
        }
    }
}

fn reopen_admitted_reader(pin: &File, entity: Entity, boundary: ReaderBoundary) -> io::Result<File> {
    let proc = open("/proc", dir_flags(), Mode::empty())?;
    if fstatfs(&proc)?.f_type != PROC_SUPER_MAGIC {
        return Err(invalid("a genuine procfs for this process is required"));
    }
    let pid = std::process::id().to_string();
    if readlinkat(&proc, "self", Vec::new())?.as_bytes() != pid.as_bytes() {
        return Err(invalid("procfs refers to another process namespace"));
    }
    let proc_pid = openat(&proc, pid.as_str(), dir_flags(), Mode::empty())?;
    let fds = File::from(openat(&proc_pid, "fd", dir_flags(), Mode::empty())?);
    if fstatfs(&fds)?.f_type != PROC_SUPER_MAGIC { return Err(invalid("untrusted descriptor directory")); }
    // Even a procfs directory must belong to our descriptor table. Follow the kernel
    // link for the directory's own held fd and compare identities before using it.
    let myself = fds.as_raw_fd().to_string();
    let again = File::from(openat(&fds, myself.as_str(),
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())?);
    if Entity::of(&again.metadata()?)? != Entity::of(&fds.metadata()?)? {
        return Err(invalid("procfs descriptor directory is not this process's"));
    }
    let number = pin.as_raw_fd().to_string();
    let proc_link = File::from(openat(&fds, number.as_str(),
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?);
    if !proc_link.metadata()?.file_type().is_symlink() || fstatfs(&proc_link)?.f_type != PROC_SUPER_MAGIC {
        return Err(invalid("descriptor is not a procfs kernel link"));
    }
    let check = File::from(openat(&fds, number.as_str(), OFlags::PATH | OFlags::CLOEXEC, Mode::empty())?);
    let original = check.metadata()?;
    if !original.is_file()
        || Entity::of(&original)? != entity
        || !boundary.matches(&check)?
    {
        return Err(invalid("descriptor no longer selects the original regular file"));
    }
    let result = File::from(openat(&fds, number.as_str(),
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::NOATIME | OFlags::CLOEXEC, Mode::empty())?);
    let observed = result.metadata()?;
    if !observed.is_file() || Entity::of(&observed)? != entity {
        return Err(invalid("read descriptor did not resolve to the original regular file"));
    }
    if !boundary.matches(&result)? {
        return Err(invalid("read descriptor no longer belongs to the selected data filesystem"));
    }
    Ok(result)
}

pub(crate) fn local_data_filesystem(fd: &File) -> io::Result<u32> {
    require_local_data_fs(filesystem(fd)?)
}

// S_IFREG on its own does not make a kernel/remote control endpoint a data file.
// These are Linux UAPI values, not distro/mount-point names. Unknown means no reader.
fn require_local_data_fs(magic: Option<u32>) -> io::Result<u32> {
    match magic {
        Some(kind) if helper_linux_evidence::supported_local_magic(kind) => Ok(kind),
        _ => Err(io::Error::new(io::ErrorKind::Unsupported, "requires a supported local data filesystem (ext2/3/4, XFS, Btrfs or F2FS)")),
    }
}

fn filesystem(file: &File) -> io::Result<Option<u32>> {
    Ok(u32::try_from(fstatfs(file)?.f_type).ok())
}

#[cfg(test)]
mod tests {
    use super::require_local_data_fs;
    use linux_raw_sys::general as fs;
    #[test]
    fn new_directory_at_the_same_spelling_cannot_replace_a_held_observation() {
        let area = tempfile::tempdir().unwrap();
        let dir = area.path().join("original");
        std::fs::create_dir(&dir).unwrap();
        let original = super::Pinned::open(&dir, super::Kind::Directory).unwrap();
        std::fs::rename(&dir, area.path().join("retained")).unwrap();
        std::fs::create_dir(&dir).unwrap();
        assert!(original.require_same_path().is_err());
        assert_ne!(original.entity(), super::Pinned::open(&dir, super::Kind::Directory).unwrap().entity());
    }
    #[test]
    fn replacing_only_a_parent_is_rejected_even_when_the_original_leaf_is_moved_back() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("original-parent");
        let saved_parent = temp.path().join("saved-parent");
        let leaf = parent.join("unchanged-leaf");
        std::fs::create_dir(&parent).unwrap();
        std::fs::create_dir(&leaf).unwrap();
        let selected = super::Pinned::open(&leaf, super::Kind::Directory).unwrap();
        let original_parent = super::Pinned::open(&parent, super::Kind::Directory).unwrap();
        std::fs::rename(&parent, &saved_parent).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::rename(saved_parent.join("unchanged-leaf"), &leaf).unwrap();

        let current = super::Pinned::open(&leaf, super::Kind::Directory).unwrap();
        assert_eq!(selected.entity(), current.entity()); // comparing just the leaf would pass
        assert_ne!(original_parent.entity(), super::Pinned::open(&parent, super::Kind::Directory).unwrap().entity());
        assert!(selected.require_same_path().is_err());
        assert!(current.require_same_path().is_ok());
    }
    #[test]
    fn filesystem_allowlist_returns_the_admitted_type_or_rejects_it() {
        for v in [fs::EXT2_SUPER_MAGIC, fs::EXT3_SUPER_MAGIC, fs::EXT4_SUPER_MAGIC,
                  fs::XFS_SUPER_MAGIC, fs::BTRFS_SUPER_MAGIC, fs::F2FS_SUPER_MAGIC] {
            assert_eq!(require_local_data_fs(Some(v)).unwrap(), v);
        }
        for rejected in [None, Some(0), Some(u32::MAX), Some(fs::PROC_SUPER_MAGIC), Some(fs::SYSFS_MAGIC),
                         Some(fs::DEBUGFS_MAGIC), Some(fs::TRACEFS_MAGIC), Some(fs::TMPFS_MAGIC),
                         Some(fs::NFS_SUPER_MAGIC), Some(fs::CIFS_SUPER_MAGIC), Some(fs::FUSE_SUPER_MAGIC),
                         Some(fs::OVERLAYFS_SUPER_MAGIC)] {
            assert!(require_local_data_fs(rejected).is_err());
        }
    }
}
