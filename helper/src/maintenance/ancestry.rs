//! Fresh-root ancestry in the current maintenance thread's mount namespace.
//! We never search other paths, open files on a volume, or recover a hidden parent.
use crate::pinned::{Entity, Mount, Pinned, invalid, local_data_filesystem};
use rustix::fs::{Mode, OFlags, fgetxattr, makedev, openat};
use rustix::io::Errno;
use std::{collections::BTreeMap, fs::File, io::{self, Read}, os::unix::fs::MetadataExt};

const TABLE_BYTES: usize = 4 * 1024 * 1024;
const TABLE_ROWS: usize = 16_384;
fn unknown(why: &'static str) -> io::Error { io::Error::new(io::ErrorKind::Unsupported, why) }

fn no_claim(dir: &File) -> io::Result<()> {
    let mut v = [0u8; 1];
    match fgetxattr(dir, c"user.zxs.volume", &mut v[..]) {
        Err(Errno::NODATA) => Ok(()),
        Ok(_) | Err(Errno::RANGE) => Err(invalid("selected storage root or its ancestor already has a volume attribute")),
        Err(e) => Err(e.into()),
    }
}

#[derive(Clone, Copy)]
struct Row { parent: u64, device: u64, whole: bool }

pub(super) struct Mounts(BTreeMap<u64, Row>);
impl Mounts {
    pub(super) fn current() -> io::Result<Self> {
        let f = super::thread_evidence::mountinfo()?;
        let mut bytes = Vec::new();
        f.take(TABLE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        Self::parse(&bytes)
    }

    // mountinfo(5), numeric mount ID, parent, major:minor, then the root
    // *inside* the filesystem. Field five's visible mountpoint is not this root.
    // No stored paths are decoded, logged or subsequently opened.
    fn parse(bytes: &[u8]) -> io::Result<Self> {
        let bad = || unknown("incomplete, oversized or ambiguous mountinfo");
        if bytes.is_empty() || bytes.len() > TABLE_BYTES || !bytes.ends_with(b"\n") { return Err(bad()); }
        let mut rows = BTreeMap::new();
        fn number(b: &[u8]) -> Option<u64> {
            if b.is_empty() || !b.iter().all(u8::is_ascii_digit) { return None; }
            std::str::from_utf8(b).ok()?.parse().ok()
        }
        for line in bytes[..bytes.len()-1].split(|&c| c == b'\n') {
            if rows.len() >= TABLE_ROWS { return Err(bad()); }
            let mut words = line.split(|&c| c == b' ');
            let id = words.next().and_then(number).filter(|v| *v != 0).ok_or_else(bad)?;
            let parent = words.next().and_then(number).filter(|v| *v != 0).ok_or_else(bad)?;
            let raw = words.next().ok_or_else(bad)?;
            let colon = raw.iter().position(|&c| c == b':').ok_or_else(bad)?;
            let device_number = |b: &[u8]| number(b).and_then(|n| u32::try_from(n).ok()).ok_or_else(bad);
            let device = makedev(device_number(&raw[..colon])?, device_number(&raw[colon+1..])?);
            let root = words.next().filter(|w| w.starts_with(b"/")).ok_or_else(bad)?;
            if !words.next().is_some_and(|w| w.starts_with(b"/")) || !words.next().is_some_and(|w| !w.is_empty())
                || !words.any(|w| w == b"-") || !(0..3).all(|_| words.next().is_some_and(|w| !w.is_empty()))
                || words.next().is_some() || rows.insert(id, Row { parent, device, whole: root == b"/" }).is_some() {
                return Err(bad());
            }
        }
        Ok(Self(rows))
    }

    fn whole(&self, id: u64, device: u64) -> io::Result<Row> {
        match self.0.get(&id) {
            Some(r) if r.whole && r.device == device => Ok(*r),
            _ => Err(unknown("cannot establish complete filesystem ancestry (subtree mount, subvolume or unknown mount)")),
        }
    }

    fn require_root_boundary(&self, mount: Mount, device: u64) -> io::Result<()> {
        let row = self.whole(mount.id, device)?;
        // A missing parent row is ambiguous: both a chroot at a *whole*
        // mounted filesystem and a normal host with hidden initramfs can have
        // it. Neither is positive evidence that the ascent reached its boundary.
        if mount.at_root == Some(true) && row.parent == mount.id { return Ok(()); }
        Err(unknown("kernel did not positively expose the self-parent mount root"))
    }

    /// Starting with a fresh description of the already checked leaf, walk ..
    /// only over the exact forward-resolved entities. Unreadable, hidden or
    /// reparented ancestry is not treated as an unclaimed original.
    pub(super) fn unclaimed_chain(&self, name: &Pinned, leaf: &File) -> io::Result<Vec<u64>> {
        self.chain(name, leaf, None).map(|(mounts, _)| mounts)
    }

    pub(super) fn pending_chain(&self, name: &Pinned, leaf: &File,
        image: &super::genesis::FreshGenesis<'_>, row: usize) -> io::Result<(Vec<u64>, bool)> {
        self.chain(name, leaf, Some((image, row)))
    }

    fn chain(&self, name: &Pinned, leaf: &File, original: Option<(&super::genesis::FreshGenesis<'_>, usize)>)
        -> io::Result<(Vec<u64>, bool)> {
        let mut mounts = Vec::with_capacity(name.visible_chain().len());
        let mut labelled = false;
        let mut parent_fd: Option<File> = None;
        let mut child: Option<(Mount, Row)> = None;
        let mut expected = name.visible_chain().iter().rev().peekable();
        while let Some(entity) = expected.next() {
            let dir = parent_fd.as_ref().unwrap_or(leaf);
            let m = dir.metadata()?;
            if !m.is_dir() || m.nlink() == 0 || Entity::of(&m)? != *entity {
                return Err(invalid("selected directory ancestry changed"));
            }
            let mount = Mount::on(dir)?.ok_or_else(|| unknown("ancestor has no mount identity"))?;
            let row = self.whole(mount.id, m.dev())?;
            if let Some((c, r)) = child {
                if c.id != mount.id && (c.at_root != Some(true) || r.parent != mount.id) {
                    return Err(unknown("cannot verify a mount transition or its covered parent"));
                }
            }
            // Unknown/union filesystems, denied xattrs and empty markers do not
            // count as unclaimed. No enumeration of ancestor contents is needed.
            local_data_filesystem(dir)?;
            if let Some((image, row)) = original.filter(|_| mounts.is_empty()) {
                // Only the selected leaf may already bear its own original label.
                labelled = super::identity::original_pending_label(dir, image, row)?;
            } else { no_claim(dir)?; }
            mounts.push(mount.id);
            if expected.peek().is_none() {
                self.require_root_boundary(mount, m.dev())?;
                break;
            }
            child = Some((mount, row));
            parent_fd = Some(File::from(openat(dir, c"..", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?));
        }
        Ok((mounts, labelled))
    }
}

#[cfg(test)]
#[path = "../../tests/maintenance/ancestry.rs"]
mod tests;
