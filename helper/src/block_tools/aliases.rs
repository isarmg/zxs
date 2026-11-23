//! Optional, explicitly labelled udev aliases are O_PATH-only. The separate
//! `NVMe` health selection can hold one checked direct namespace read-only.
use super::{
    BlockRoot, Dev, Entity, Field, File, HostMounts, Identity, Kind, Reason, io, unsupported,
};
use crate::pinned::Mount;
use rustix::fs::{Mode, OFlags, RawDir, ResolveFlags, openat2};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    mem::MaybeUninit,
    os::unix::{
        ffi::OsStrExt,
        fs::{FileTypeExt, MetadataExt},
    },
};

const LIMIT: usize = 512;
fn confined() -> ResolveFlags {
    ResolveFlags::BENEATH | ResolveFlags::NO_XDEV | ResolveFlags::NO_MAGICLINKS
}
fn path() -> OFlags {
    OFlags::PATH | OFlags::CLOEXEC
}
fn protected(file: &File) -> io::Result<()> {
    let m = file.metadata()?;
    if !m.is_dir() || m.uid() != 0 || m.mode() & 0o022 != 0 {
        return Err(unsupported("untrusted device-alias directory"));
    }
    Ok(())
}
fn legal(s: &[u8]) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.iter().all(|b| b.is_ascii_hexdigit() || *b == b'-')
        && s.iter().any(u8::is_ascii_hexdigit)
}
type AliasMap = BTreeMap<u64, Field<String>>;
pub(super) struct Index(Field<AliasMap>);
impl Index {
    pub(super) fn get(&self, d: Dev) -> Field<String> {
        self.0
            .as_ref()
            .map_err(|e| *e)?
            .get(&d.rdev())
            .cloned()
            .unwrap_or(Err(Reason::NotReported))
    }
}
pub(super) struct Aliases {
    pub(super) fs: Index,
    pub(super) partition: Index,
}

fn read_one(dev: &File, set: &str) -> Field<AliasMap> {
    let root = openat2(
        dev,
        c"disk",
        path() | OFlags::DIRECTORY,
        Mode::empty(),
        confined() | ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .map_err(|e| Reason::io_kind(io::Error::from(e).kind()))?;
    protected(&root).map_err(|error| Reason::io_kind(error.kind()))?;
    let dir = openat2(
        &root,
        set,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        confined() | ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .map_err(|e| Reason::io_kind(io::Error::from(e).kind()))?;
    protected(&dir).map_err(|error| Reason::io_kind(error.kind()))?;
    let original_dir = stamp(&dir).map_err(|error| Reason::io_kind(error.kind()))?;
    let original_disk = stamp(&root).map_err(|error| Reason::io_kind(error.kind()))?;
    let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
    let mut entries = RawDir::new(&dir, &mut buffer);
    let (mut count, mut out) = (0, BTreeMap::new());
    while let Some(next) = entries.next() {
        let item = next.map_err(|e| Reason::io_kind(io::Error::from(e).kind()))?;
        let name = item.file_name().to_bytes();
        if matches!(name, b"." | b"..") {
            continue;
        }
        count += 1;
        if count > LIMIT {
            return Err(Reason::Limit);
        }
        if !legal(name) {
            continue;
        } // unusual identifier: no guess, no output from that name
        record_alias(dev, &dir, set, name, &mut out)?;
    }
    let again = openat2(
        dev,
        format!("disk/{set}").as_str(),
        path() | OFlags::DIRECTORY,
        Mode::empty(),
        confined() | ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .map_err(|e| Reason::io_kind(io::Error::from(e).kind()))?;
    if stamp(&again).map_err(|error| Reason::io_kind(error.kind()))? != original_dir
        || stamp(&dir).map_err(|error| Reason::io_kind(error.kind()))? != original_dir
        || stamp(&root).map_err(|error| Reason::io_kind(error.kind()))? != original_disk
    {
        return Err(Reason::Changed);
    }
    Ok(out)
}

fn stamp(file: &File) -> io::Result<(u64, u64, i64, i64, i64, i64)> {
    let m = file.metadata()?;
    Ok((
        m.dev(),
        m.ino(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}

fn record_alias(dev: &File, dir: &File, set: &str, name: &[u8], out: &mut AliasMap) -> Field<()> {
    let alias = OsStr::from_bytes(name);
    let before = openat2(
        dir,
        alias,
        path() | OFlags::NOFOLLOW,
        Mode::empty(),
        confined(),
    )
    .map(File::from)
    .map_err(|e| Reason::io_kind(io::Error::from(e).kind()))?;
    let m = before
        .metadata()
        .map_err(|error| Reason::io_kind(error.kind()))?;
    if !m.file_type().is_symlink() || m.uid() != 0 {
        return Err(Reason::Invalid);
    }
    let name = std::str::from_utf8(name).map_err(|_| Reason::Invalid)?;
    let link = format!("disk/{set}/{name}");
    let resolve = || {
        openat2(dev, link.as_str(), path(), Mode::empty(), confined())
            .map(File::from)
            .map_err(|e| Reason::io_kind(io::Error::from(e).kind()))
    };
    let node = resolve()?;
    let n = node
        .metadata()
        .map_err(|error| Reason::io_kind(error.kind()))?;
    if !n.file_type().is_block_device() {
        return Err(Reason::Invalid);
    }
    let again = openat2(
        dir,
        alias,
        path() | OFlags::NOFOLLOW,
        Mode::empty(),
        confined(),
    )
    .map(File::from)
    .map_err(|e| Reason::io_kind(io::Error::from(e).kind()))?;
    let still = resolve()?
        .metadata()
        .map_err(|error| Reason::io_kind(error.kind()))?;
    if stamp(&before).map_err(|error| Reason::io_kind(error.kind()))?
        != stamp(&again).map_err(|error| Reason::io_kind(error.kind()))?
        || !still.file_type().is_block_device()
        || (n.dev(), n.ino(), n.rdev()) != (still.dev(), still.ino(), still.rdev())
    {
        return Err(Reason::Changed);
    }
    out.entry(n.rdev())
        .and_modify(|value| *value = Err(Reason::Invalid))
        .or_insert_with(|| Ok(name.to_owned()));
    Ok(())
}

pub(super) struct DevRoot {
    fd: File,
    entity: Entity,
    mount: Mount,
}
impl DevRoot {
    pub(super) fn pin() -> Field<Self> {
        let fd = BlockRoot::DeviceAliases
            .pin()
            .map_err(|error| Reason::io_kind(error.kind()))?;
        protected(&fd).map_err(|error| Reason::io_kind(error.kind()))?;
        let entity = Entity::of(
            &fd.metadata()
                .map_err(|error| Reason::io_kind(error.kind()))?,
        )
        .map_err(|error| Reason::io_kind(error.kind()))?;
        let mount = Mount::on(&fd)
            .map_err(|error| Reason::io_kind(error.kind()))?
            .ok_or(Reason::Unsupported)?;
        if mount.at_root != Some(true) {
            return Err(Reason::Unsupported);
        }
        Ok(Self { fd, entity, mount })
    }
    fn still_named(&self) -> Field<()> {
        let now = Self::pin()?;
        if (now.entity, now.mount) != (self.entity, self.mount) {
            return Err(Reason::Changed);
        }
        Ok(())
    }

    fn require_devtmpfs(&self) -> Field<()> {
        if !HostMounts::current()
            .map_err(|error| Reason::io_kind(error.kind()))?
            .is_full_devtmpfs(self.mount.id, self.entity.dev)
        {
            return Err(Reason::Unsupported);
        }
        self.still_named()
    }
}

pub(super) fn nvme_namespace_name(name: &str) -> bool {
    let canonical_number = |value: &str| {
        value.len() <= 10 && value.parse::<u32>().is_ok_and(|n| n.to_string() == value)
    };
    name.strip_prefix("nvme")
        .and_then(|rest| rest.split_once('n'))
        .is_some_and(|(controller, namespace)| {
            canonical_number(controller) && canonical_number(namespace) && namespace != "0"
        })
}

/// Only the direct namespace spelling, under the original root-owned
/// devtmpfs. Never use a udev alias, controller pathname or caller string.
pub(super) struct NvmeNamespace<'a> {
    root: DevRoot,
    name: &'a str,
    dev: Dev,
    pin: File,
    original: File,
}
impl<'a> NvmeNamespace<'a> {
    fn open_slot(root: &DevRoot, name: &str, flags: OFlags) -> Field<File> {
        openat2(
            &root.fd,
            name,
            flags | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            confined() | ResolveFlags::NO_SYMLINKS,
        )
        .map(File::from)
        .map_err(|e| Reason::io_kind(io::Error::from(e).kind()))
    }

    fn stamp(root: &DevRoot, fd: &File, dev: Dev) -> Field<Entity> {
        let m = fd
            .metadata()
            .map_err(|error| Reason::io_kind(error.kind()))?;
        if !m.file_type().is_block_device()
            || m.uid() != 0
            || m.rdev() != dev.rdev()
            || Mount::on(fd)
                .map_err(|error| Reason::io_kind(error.kind()))?
                .is_none_or(|m| m.id != root.mount.id || m.at_root != Some(false))
        {
            return Err(Reason::Changed);
        }
        Entity::of(&m).map_err(|error| Reason::io_kind(error.kind()))
    }

    pub(super) fn open(identity: &'a Identity) -> Field<Self> {
        if identity.kind != Kind::Disk || !nvme_namespace_name(&identity.name) {
            return Err(Reason::Unsupported);
        }
        let name = &identity.name;
        let dev = identity.dev;
        let root = DevRoot::pin()?;
        root.require_devtmpfs()?;
        let pin = Self::open_slot(&root, name, OFlags::PATH)?;
        Self::stamp(&root, &pin, dev)?;
        let original = Self::open_slot(&root, name, OFlags::RDONLY | OFlags::NONBLOCK)?;
        let node = Self {
            root,
            name,
            dev,
            pin,
            original,
        };
        node.require()?;
        Ok(node)
    }

    pub(super) fn require(&self) -> Field<()> {
        self.root.require_devtmpfs()?;
        let now = Self::open_slot(&self.root, self.name, OFlags::PATH)?;
        let initial = Self::stamp(&self.root, &self.pin, self.dev)?;
        if Self::stamp(&self.root, &self.original, self.dev)? != initial
            || Self::stamp(&self.root, &now, self.dev)? != initial
        {
            return Err(Reason::Changed);
        }
        Ok(())
    }

    pub(super) fn original(&self) -> &File {
        &self.original
    }
}

pub(super) fn read(root: Field<DevRoot>, mounts: Field<&HostMounts>) -> Aliases {
    let res = (|| {
        let dev = root?;
        if !mounts?.is_full_devtmpfs(dev.mount.id, dev.entity.dev) {
            return Err(Reason::Unsupported);
        }
        let fs = Index(read_one(&dev.fd, "by-uuid"));
        let partition = Index(read_one(&dev.fd, "by-partuuid"));
        dev.still_named()?;
        Ok(Aliases { fs, partition })
    })();
    res.unwrap_or_else(|e| Aliases {
        fs: Index(Err(e)),
        partition: Index(Err(e)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vendor_text_control_characters_and_paths_are_not_uuid_aliases() {
        for s in [
            &b"02b605ba-13b0-417e-9016-81b518f6e0c1"[..],
            b"ABCD-1234",
            b"8a3101a1-02",
        ] {
            assert!(legal(s));
        }
        for s in [&b"../x"[..], b"", b"-", b"x\x1b[0m", b"ab/ab", b"xyz"] {
            assert!(!legal(s));
        }
    }
}
