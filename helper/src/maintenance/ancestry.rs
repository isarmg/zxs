//! Mount evidence in the current maintenance thread's namespace.
//! Ancestry never searches other paths or recovers a hidden parent. The
//! separate discovery cut returns only bounded, admitted mountpoint names.
use crate::pinned::{DiscoveryAdmission, DiscoveryFdInfo};
#[cfg(any(test, feature = "embedded-zxs"))]
use crate::pinned::{Entity, Mount};
const PRODUCT_TOP: &str = "/zxs";
use linux_raw_sys::general::{
    BTRFS_SUPER_MAGIC, EXT4_SUPER_MAGIC, F2FS_SUPER_MAGIC, XFS_SUPER_MAGIC,
};
use rustix::fs::makedev;
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::File,
    io::{self, Read},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

const TABLE_BYTES: usize = 4 * 1024 * 1024;
const TABLE_ROWS: usize = 16_384;
const DISCOVERY_ROOTS: usize = 64;
fn unknown(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, why)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum MountPlace {
    NamespaceRoot,
    ProductRoot,
    Other,
}
#[derive(Clone, Copy, Eq, PartialEq)]
struct Row {
    parent: u64,
    device: u64,
    whole: bool,
    // Keep this filesystem class in the exact before/after mount-table cut.
    devtmpfs: bool,
    magic: Option<u32>,
    place: MountPlace,
}
fn data_magic(fs: &[u8]) -> Option<u32> {
    match fs {
        b"ext2" | b"ext3" | b"ext4" => Some(EXT4_SUPER_MAGIC),
        b"xfs" => Some(XFS_SUPER_MAGIC),
        b"f2fs" => Some(F2FS_SUPER_MAGIC),
        b"btrfs" => Some(BTRFS_SUPER_MAGIC),
        _ => None,
    }
}

pub(crate) struct Mounts(BTreeMap<u64, Row>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoveryMount {
    pub(crate) path: PathBuf,
    pub(crate) id: u64,
    pub(crate) device: u64,
    pub(crate) magic: u32,
}
// mountinfo escapes only whitespace and backslash. Parse byte-for-byte, not
// as shell, URL or Path normalization. Explicit evidence may be non-UTF-8;
// only paths shown as automatic SYSTEM candidates must also be UTF-8.
fn discovery_path(mut raw: &[u8]) -> Option<PathBuf> {
    let mut name = Vec::new();
    while let Some((&byte, tail)) = raw.split_first() {
        if name.len() >= crate::path_rules::MAX_PATH_BYTES {
            return None;
        }
        match byte {
            b'\\' => {
                let code = tail.get(..3)?;
                name.push(match code {
                    b"040" => b' ',
                    b"011" => b'\t',
                    b"012" => b'\n',
                    b"134" => b'\\',
                    _ => return None,
                });
                raw = &tail[3..];
            }
            b' ' | b'\t' | b'\n' | 0 => return None,
            v => {
                name.push(v);
                raw = tail;
            }
        }
    }
    if name.as_slice() != b"/" {
        crate::pinned::Kind::StorageRoot.check_path(&name).ok()?;
    }
    Some(PathBuf::from(OsStr::from_bytes(&name)))
}

struct DiscoveryTable {
    mounts: Mounts,
    names: BTreeMap<PathBuf, Option<(u64, u64)>>,
    #[cfg(test)]
    roots: Vec<DiscoveryMount>,
}
impl DiscoveryTable {
    fn named(&self, path: &Path, parent: Option<u64>) -> io::Result<Option<u64>> {
        let no = || unknown("no unambiguous exact local mount at this discovery crossing");
        let Some(row) = self.names.get(path) else {
            return Ok(None);
        };
        let (id, up) = (*row).ok_or_else(no)?;
        if !self.mounts.discovery_ancestry(id) || parent.is_some_and(|p| p != up) {
            return Err(no());
        }
        Ok(Some(id))
    }
    #[cfg(any(test, feature = "embedded-zxs"))]
    fn product_crossing(&self, parent: u64) -> io::Result<Option<u64>> {
        let next = self.named(Path::new(PRODUCT_TOP), Some(parent))?;
        // Rule out a subtree before crossing, not after querying it. The
        // named test also rules out remote, pseudo, autofs and stacked rows.
        if let Some(id) = next {
            let (above, _) = self.mounts.full_mount(id)?;
            if above != parent || id == parent {
                return Err(unknown("not a complete separate /zxs mount"));
            }
        }
        Ok(next)
    }
}
/// One process-local discovery cut; its table cannot be populated by CLI,
/// a file argument, Helper config or an external mount/identity cache.
pub(crate) struct DiscoveryView {
    table: DiscoveryTable,
    fds: DiscoveryFdInfo,
}
impl DiscoveryView {
    #[cfg(any(test, feature = "embedded-zxs"))]
    pub(crate) fn product_crossing(&self, parent: u64) -> io::Result<Option<u64>> {
        self.table.product_crossing(parent)
    }
    #[cfg(any(test, feature = "embedded-zxs"))]
    pub(crate) fn require_product_mount(
        &self,
        child: Mount,
        parent: u64,
        entity: Entity,
        magic: u32,
    ) -> io::Result<()> {
        self.table
            .mounts
            .require_product_mount(child, parent, entity, magic)
    }
    pub(crate) fn full_local_mount(&self, id: u64) -> io::Result<(u64, u64)> {
        if !self.table.mounts.discovery_ancestry(id) {
            return Err(unknown("no eligible local mount ancestry"));
        }
        self.table.mounts.full_mount(id)
    }
    pub(crate) fn require_unchanged(&self) -> io::Result<()> {
        let current = Mounts::current_discovery_table(false)?;
        if self.table.names != current.names || self.table.mounts.0 != current.mounts.0 {
            return Err(unknown("local mount evidence changed"));
        }
        Ok(())
    }
}
impl DiscoveryAdmission for DiscoveryView {
    fn root_mount(&self) -> io::Result<u64> {
        self.table
            .named(Path::new("/"), None)?
            .ok_or_else(|| unknown("cannot prove a local visible root for discovery"))
    }
    fn crossing(&self, parent: u64, prefix: &Path) -> io::Result<Option<u64>> {
        self.table.named(prefix, Some(parent))
    }
    fn admit(&self, fd: &File) -> io::Result<u64> {
        let id = self.fds.mount(fd)?;
        if !self.table.mounts.discovery_ancestry(id) {
            return Err(unknown(
                "a live discovery path is not on an admitted local mount",
            ));
        }
        Ok(id)
    }
}

impl Mounts {
    #[cfg(any(test, feature = "embedded-zxs"))]
    pub(crate) fn current() -> io::Result<Self> {
        let f = super::thread_evidence::mountinfo()?;
        let mut bytes = Vec::new();
        f.take(TABLE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        Self::parse(&bytes)
    }

    pub(crate) fn discovery_view(automatic: bool) -> io::Result<DiscoveryView> {
        Ok(DiscoveryView {
            table: Self::current_discovery_table(automatic)?,
            fds: DiscoveryFdInfo::open()?,
        })
    }
    fn current_discovery_table(automatic: bool) -> io::Result<DiscoveryTable> {
        let f = super::thread_evidence::passive_mountinfo()?;
        let mut bytes = Vec::new();
        f.take(TABLE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        Self::discovery_table(&bytes, automatic)
    }
    #[cfg(test)]
    fn discover(bytes: &[u8]) -> io::Result<Vec<DiscoveryMount>> {
        Ok(Self::discovery_table(bytes, true)?.roots)
    }
    fn discovery_table(bytes: &[u8], automatic: bool) -> io::Result<DiscoveryTable> {
        let mut roots = Vec::new();
        let mut names = BTreeMap::new();
        let table = Self::parse_each(bytes, |id, row, point| {
            let Some(path) = discovery_path(point) else {
                return Ok(());
            };
            names
                .entry(path.clone())
                .and_modify(|entry| *entry = None)
                .or_insert(Some((id, row.parent)));
            // No overlay, network, device-free pseudo filesystems, autofs or
            // filesystem subtree binds in automatic discovery. Explicit
            // selection is the only fallback, not a recursive path scan.
            if let Some(magic) = row.magic.filter(|_| {
                automatic
                    && row.whole
                    && row.device != 0
                    && row.place != MountPlace::NamespaceRoot
                    && path.to_str().is_some()
            }) {
                if roots.len() == DISCOVERY_ROOTS {
                    return Err(unknown(
                        "too many eligible mounted roots; select one --system directory",
                    ));
                }
                roots.push(DiscoveryMount {
                    path,
                    id,
                    device: row.device,
                    magic,
                });
            }
            Ok(())
        })?;
        // A reachable local mount can itself be nested under NFS, autofs or
        // another unsupported parent. Skip these before any pathname walk.
        roots.retain(|r| table.discovery_ancestry(r.id));
        roots.sort_unstable_by(|a, b| (&a.path, a.id).cmp(&(&b.path, b.id)));
        Ok(DiscoveryTable {
            mounts: table,
            names,
            #[cfg(test)]
            roots,
        })
    }
    fn discovery_ancestry(&self, mut id: u64) -> bool {
        for _ in 0..=256 {
            let Some(row) = self
                .0
                .get(&id)
                .filter(|r| r.magic.is_some() && r.device != 0)
            else {
                return false;
            };
            if row.place == MountPlace::NamespaceRoot {
                return true;
            }
            if row.parent == id {
                return false;
            }
            id = row.parent;
        }
        false
    }

    // mountinfo(5), numeric mount ID, parent, major:minor, then the root
    // *inside* the filesystem. Field five's visible mountpoint is not this root.
    // No stored paths are decoded, logged or subsequently opened.
    #[cfg(any(test, feature = "embedded-zxs"))]
    fn parse(bytes: &[u8]) -> io::Result<Self> {
        Self::parse_each(bytes, |_, _, _| Ok(()))
    }
    fn parse_each(
        bytes: &[u8],
        mut visit: impl FnMut(u64, Row, &[u8]) -> io::Result<()>,
    ) -> io::Result<Self> {
        fn number(b: &[u8]) -> Option<u64> {
            if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
                return None;
            }
            std::str::from_utf8(b).ok()?.parse().ok()
        }
        let bad = || unknown("incomplete, oversized or ambiguous mountinfo");
        if bytes.is_empty() || bytes.len() > TABLE_BYTES || !bytes.ends_with(b"\n") {
            return Err(bad());
        }
        let mut rows = BTreeMap::new();
        for line in bytes[..bytes.len() - 1].split(|&c| c == b'\n') {
            if rows.len() >= TABLE_ROWS {
                return Err(bad());
            }
            let mut words = line.split(|&c| c == b' ');
            let id = words
                .next()
                .and_then(number)
                .filter(|v| *v != 0)
                .ok_or_else(bad)?;
            let parent = words
                .next()
                .and_then(number)
                .filter(|v| *v != 0)
                .ok_or_else(bad)?;
            let raw = words.next().ok_or_else(bad)?;
            let colon = raw.iter().position(|&c| c == b':').ok_or_else(bad)?;
            let device_number = |b: &[u8]| {
                number(b)
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(bad)
            };
            let device = makedev(
                device_number(&raw[..colon])?,
                device_number(&raw[colon + 1..])?,
            );
            let root = words
                .next()
                .filter(|w| w.starts_with(b"/"))
                .ok_or_else(bad)?;
            let point = words
                .next()
                .filter(|w| w.starts_with(b"/"))
                .ok_or_else(bad)?;
            if words.next().is_none_or(<[u8]>::is_empty) || !words.any(|w| w == b"-") {
                return Err(bad());
            }
            let fs_type = words.next().filter(|v| !v.is_empty()).ok_or_else(bad)?;
            let row = Row {
                parent,
                device,
                whole: root == b"/",
                devtmpfs: fs_type == b"devtmpfs",
                magic: data_magic(fs_type),
                place: match point {
                    b"/" => MountPlace::NamespaceRoot,
                    path if path == PRODUCT_TOP.as_bytes() => MountPlace::ProductRoot,
                    _ => MountPlace::Other,
                },
            };
            if !(0..2).all(|_| words.next().is_some_and(|w| !w.is_empty()))
                || words.next().is_some()
                || rows.insert(id, row).is_some()
            {
                return Err(bad());
            }
            visit(id, row, point)?;
        }
        Ok(Self(rows))
    }

    /// Generic live kernel mount evidence: parent mount ID and this complete
    /// mount's superblock device (field 3, not the name or an inode's `st_dev`).
    /// Neither field is a persistent medium ID or physical-drive certificate.
    pub(crate) fn full_mount(&self, id: u64) -> io::Result<(u64, u64)> {
        self.0
            .get(&id)
            .filter(|r| r.whole && r.device != 0)
            .map(|r| (r.parent, r.device))
            .ok_or_else(|| unknown("cannot establish a complete selected mount"))
    }

    /// The only protected site mount boundary is /zxs itself, if it is an
    /// already-mounted complete supported local root. A subtree/subvolume
    /// mount is not a complete root. Btrfs inode stat uses `anon_dev` rather
    /// than the mountinfo superblock device, so check the root inode there.
    /// This does not prove the underlying physical drive or exclude a bind
    /// of a whole independently selected filesystem root.
    #[cfg(any(test, feature = "embedded-zxs"))]
    pub(crate) fn require_product_mount(
        &self,
        child: Mount,
        parent: u64,
        entity: Entity,
        magic: u32,
    ) -> io::Result<()> {
        if child.id != parent
            && child.at_root == Some(true)
            && self.0.get(&child.id).is_some_and(|row| {
                row.place == MountPlace::ProductRoot
                    && row.whole
                    && row.parent == parent
                    && row.magic == Some(magic)
                    && row.device != 0
                    && (if magic == BTRFS_SUPER_MAGIC {
                        entity.ino == 256
                    } else {
                        entity.dev == row.device
                    })
            })
        {
            Ok(())
        } else {
            Err(unknown(
                "the original /zxs mount transition could not be attributed to a qualified complete mounted root",
            ))
        }
    }
}

#[cfg(test)]
impl Mounts {
    fn whole(&self, id: u64, device: u64) -> io::Result<Row> {
        match self.0.get(&id) {
            Some(r) if r.whole && r.device == device => Ok(*r),
            _ => Err(unknown(
                "cannot establish complete filesystem ancestry (subtree mount, subvolume or unknown mount)",
            )),
        }
    }

    fn require_root_boundary(&self, mount: Mount, device: u64) -> io::Result<()> {
        let row = self.whole(mount.id, device)?;
        // A missing parent row is ambiguous: both a chroot at a *whole*
        // mounted filesystem and a normal host with hidden initramfs can have
        // it. Neither is positive evidence that the ascent reached its boundary.
        if mount.at_root == Some(true) && row.parent == mount.id {
            return Ok(());
        }
        Err(unknown(
            "kernel did not positively expose the self-parent mount root",
        ))
    }
}

#[cfg(test)]
#[path = "../../tests/maintenance/ancestry.rs"]
mod tests;
