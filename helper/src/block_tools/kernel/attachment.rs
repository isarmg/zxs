//! Optional, bounded kernel attachment evidence. Walk the actual canonical
//! /sys/devices devpath, not guesses from names, "device" links or slaves.
use super::*;
use rustix::{fs::readlinkat_raw, io::Errno};
use std::{
    ffi::OsStr,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

const LINK_BYTES: usize = 4096;
const DEPTH: usize = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Subsystem {
    Block,
    Scsi,
    Nvme,
    NvmeGroup,
    Virtio,
    Mmc,
}
impl Subsystem {
    fn named(raw: &[u8]) -> Field<Self> {
        match raw {
            b"block" => Ok(Self::Block),
            b"scsi" => Ok(Self::Scsi),
            b"nvme" => Ok(Self::Nvme),
            b"nvme-subsystem" => Ok(Self::NvmeGroup),
            b"virtio" => Ok(Self::Virtio),
            b"mmc" => Ok(Self::Mmc),
            _ => Err(Reason::Unsupported),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Scsi => "scsi",
            Self::Nvme => "nvme",
            Self::NvmeGroup => "nvme-subsystem",
            Self::Virtio => "virtio",
            Self::Mmc => "mmc",
        }
    }
}
fn transport_type(raw: &[u8]) -> Field<&'static str> {
    match raw {
        b"pcie" => Ok("pcie"),
        b"tcp" => Ok("tcp"),
        b"rdma" => Ok("rdma"),
        b"fc" => Ok("fc"),
        b"loop" => Ok("loop"),
        _ if !raw.is_empty()
            && raw.len() <= 32
            && raw
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-') =>
        {
            Err(Reason::Unsupported)
        }
        _ => Err(Reason::Invalid),
    }
}
fn pinned_link(dir: &File, name: &OsStr) -> Field<File> {
    let f = File::from(
        openat2(
            dir,
            name,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            confined() | ResolveFlags::NO_SYMLINKS,
        )
        .map_err(|e| Reason::io(e.into()))?,
    );
    if !f.metadata().map_err(Reason::io)?.file_type().is_symlink() {
        return Err(Reason::Invalid);
    }
    Ok(f)
}
fn read_link(pin: &File, buf: &mut [u8; LINK_BYTES]) -> Field<usize> {
    let len = readlinkat_raw(pin, c"", &mut buf[..]).map_err(|e| Reason::io(e.into()))?;
    if len == buf.len() {
        return Err(Reason::Limit);
    }
    if len == 0 {
        return Err(Reason::Invalid);
    }
    Ok(len)
}
fn canonical_devpath(raw: &[u8]) -> Field<PathBuf> {
    if raw.len() >= LINK_BYTES {
        return Err(Reason::Limit);
    }
    let path = raw.strip_prefix(b"../../").ok_or(Reason::Unsupported)?;
    let below = path.strip_prefix(b"devices/").ok_or(Reason::Unsupported)?;
    let mut depth = 0;
    for part in below.split(|b| *b == b'/') {
        depth += 1;
        if depth > DEPTH {
            return Err(Reason::Limit);
        }
        if part.is_empty() || part.len() > 255 || matches!(part, b"." | b"..") || part.contains(&0)
        {
            return Err(Reason::Invalid);
        }
    }
    Ok(PathBuf::from(OsStr::from_bytes(path)))
}
fn equal(a: &File, b: &File) -> Field<bool> {
    Ok(obj(a).map_err(Reason::io)? == obj(b).map_err(Reason::io)?)
}
fn directory_fd(sys: &Sys, path: &Path, links: bool) -> rustix::io::Result<File> {
    let resolve = if links {
        confined()
    } else {
        confined() | ResolveFlags::NO_SYMLINKS
    };
    openat2(&sys.root, path, path_dir(), Mode::empty(), resolve).map(File::from)
}
fn directory(sys: &Sys, path: &Path, links: bool) -> Field<File> {
    directory_fd(sys, path, links).map_err(|e| Reason::io(e.into()))
}
fn changed_edge(r: Reason) -> Reason {
    match r {
        Reason::NotReported | Reason::Unsupported | Reason::Invalid => Reason::Changed,
        other => other,
    }
}
fn require_devpath(sys: &Sys, path: &Path, whole: &File) -> Field<()> {
    let named = directory_fd(sys, path, false).map_err(|e| match e {
        Errno::NOENT | Errno::NOTDIR | Errno::LOOP | Errno::XDEV | Errno::AGAIN => Reason::Changed,
        other => Reason::io(other.into()),
    })?;
    if !equal(whole, &named)? {
        return Err(Reason::Changed);
    }
    Ok(())
}

struct Parent {
    path: PathBuf,
    dir: File,
    link: File,
    target: File,
    kind: Subsystem,
}
impl Parent {
    fn select(sys: &Sys, path: &Path, dir: File) -> Field<Option<Self>> {
        let link = match pinned_link(&dir, OsStr::new("subsystem")) {
            Err(Reason::NotReported) => return Ok(None),
            other => other?,
        };
        let mut raw = [0; LINK_BYTES];
        let len = read_link(&link, &mut raw)?;
        // Use the kernel subsystem name, but only when the *resolved* target
        // is one of its matching known directories in the original sysfs.
        let name = raw[..len]
            .rsplit(|b| *b == b'/')
            .next()
            .ok_or(Reason::Invalid)?;
        let kind = Subsystem::named(name)?;
        let target = directory(sys, &path.join("subsystem"), true)?;
        let mut proven = false;
        for base in ["subsystem", "bus", "class"] {
            match directory(sys, &Path::new(base).join(kind.label()), false) {
                Ok(known) if equal(&known, &target)? => {
                    proven = true;
                    break;
                }
                Ok(_) | Err(Reason::NotReported) => (),
                Err(r) => return Err(r),
            }
        }
        if !proven {
            return Err(Reason::Invalid);
        }
        let selected = Self {
            path: path.to_owned(),
            dir,
            link,
            target,
            kind,
        };
        selected.require_named(sys)?;
        Ok(Some(selected))
    }
    fn require_named(&self, sys: &Sys) -> Field<()> {
        if !equal(&self.dir, &directory(sys, &self.path, false)?)?
            || !equal(
                &self.link,
                &pinned_link(&self.dir, OsStr::new("subsystem"))?,
            )?
            || !equal(
                &self.target,
                &directory(sys, &self.path.join("subsystem"), true)?,
            )?
        {
            return Err(Reason::Changed);
        }
        Ok(())
    }
    fn same(&self, next: &Self) -> Field<bool> {
        Ok(self.path == next.path
            && self.kind == next.kind
            && equal(&self.dir, &next.dir)?
            && equal(&self.link, &next.link)?
            && equal(&self.target, &next.target)?)
    }
    fn transport(&self) -> Field<&'static str> {
        // Linux' NVMe controller show method only reads ctrl->ops->name.
        // No ioctl, device node, hardware inquiry, status or speed probe.
        if self.kind != Subsystem::Nvme {
            return Err(Reason::Unsupported);
        }
        transport_type(&attr(&self.dir, c"transport").map_err(Reason::io)?)
    }
}
struct Cut {
    whole: File,
    index_link: File,
    path: PathBuf,
    parent: Parent,
    transport: Field<&'static str>,
}
impl Cut {
    fn read(sys: &Sys, dev: Dev, object: Entity, generation: u64) -> Field<Self> {
        let spelling = dev.to_string();
        let index_link = pinned_link(&sys.index, OsStr::new(&spelling))?;
        let mut raw = [0; LINK_BYTES];
        let len = read_link(&index_link, &mut raw)?;
        let path = canonical_devpath(&raw[..len])?;
        let whole = directory(sys, &path, false)?;
        let require_whole = || {
            if obj(&whole).map_err(Reason::io)? != object
                || !equal(
                    &whole,
                    &sys.at(dev).map_err(Reason::io).map_err(changed_edge)?,
                )?
                || Dev::parse(
                    &attr(&whole, c"dev")
                        .map_err(Reason::io)
                        .map_err(changed_edge)?,
                ) != Some(dev)
                || diskseq(&whole).map_err(changed_edge)? != generation
                || !equal(
                    &index_link,
                    &pinned_link(&sys.index, OsStr::new(&spelling)).map_err(changed_edge)?,
                )?
            {
                return Err(Reason::Changed);
            }
            // Kernfs builds index-link text from the current target ancestry.
            // An intact link and generation cannot prove the devpath used for
            // this cut still leads to the held whole disk after a reparent.
            require_devpath(sys, &path, &whole)
        };
        require_whole()?;
        for p in path.ancestors().skip(1).take(DEPTH) {
            if p == Path::new("devices") {
                break;
            }
            let Some(parent) = Parent::select(sys, p, directory(sys, p, false)?)? else {
                continue;
            };
            if parent.kind == Subsystem::Block {
                continue;
            }
            let transport = parent.transport();
            parent.require_named(sys).map_err(changed_edge)?;
            require_whole()?;
            return Ok(Self {
                whole,
                index_link,
                path,
                parent,
                transport,
            });
        }
        Err(Reason::NotReported)
    }
    fn finish(&self, later: &Self, source: AttachmentSource) -> Field<Attachment> {
        if self.path != later.path
            || !equal(&self.whole, &later.whole)?
            || !equal(&self.index_link, &later.index_link)?
            || !self.parent.same(&later.parent)?
        {
            return Err(Reason::Changed);
        }
        let nvme_transport = if self.transport == later.transport {
            later.transport
        } else {
            Err(Reason::Changed)
        };
        Ok(Attachment {
            subsystem: later.parent.kind.label(),
            source,
            nvme_transport,
        })
    }
}
impl Node {
    pub(in crate::block_tools) fn attachment(&self, sys: &Sys, id: &Identity) -> Field<Attachment> {
        if self.dev != id.dev || obj(&self.dir).map_err(Reason::io)? != id.object {
            return Err(Reason::Changed);
        }
        let generation = id.diskseq?;
        let (dev, object, source) = match id.kind {
            Kind::Disk => (self.dev, id.object, AttachmentSource::KernelAncestor),
            Kind::Partition {
                parent,
                parent_object,
                ..
            } => (parent, parent_object, AttachmentSource::InferredParent),
        };
        let first = Cut::read(sys, dev, object, generation)?;
        let second = Cut::read(sys, dev, object, generation).map_err(changed_edge)?;
        first.finish(&second, source)
    }
}

/// Retain the actual direct `NVMe` controller ancestor, not just its label.
/// Multipath/subsystem routing, partitions, fabrics and inferred owners refuse.
pub(in crate::block_tools) struct NvmeOwner {
    cut: Cut,
    dev: Dev,
    object: Entity,
    generation: u64,
}
impl NvmeOwner {
    pub(in crate::block_tools) fn select(sys: &Sys, node: &Node, id: &Identity) -> Field<Self> {
        if id.kind != Kind::Disk {
            return Err(Reason::Unsupported);
        }
        if node.dev != id.dev || obj(&node.dir).map_err(Reason::io)? != id.object {
            return Err(Reason::Changed);
        }
        let generation = id.diskseq?;
        let cut = Cut::read(sys, id.dev, id.object, generation)?;
        if cut.parent.kind != Subsystem::Nvme || cut.transport != Ok("pcie") {
            return Err(Reason::Unsupported);
        }
        let owner = Self {
            cut,
            dev: id.dev,
            object: id.object,
            generation,
        };
        owner.require(sys)?;
        Ok(owner)
    }

    pub(in crate::block_tools) fn require(&self, sys: &Sys) -> Field<()> {
        let later = Cut::read(sys, self.dev, self.object, self.generation).map_err(changed_edge)?;
        let verified = self.cut.finish(&later, AttachmentSource::KernelAncestor)?;
        if verified.nvme_transport != Ok("pcie") {
            return Err(Reason::Changed);
        }
        let state = attr(&later.parent.dir, c"state").map_err(Reason::io)?;
        if state != b"live" {
            return Err(Reason::NotReported);
        }
        later.parent.require_named(sys).map_err(changed_edge)?;
        sys.require_named().map_err(Reason::io)
    }

    pub(in crate::block_tools) fn generation(&self) -> u64 {
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    struct Fixture {
        temp: tempfile::TempDir,
        sys: Sys,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            fs::create_dir_all(temp.path().join("dev/block")).unwrap();
            let sys = Sys {
                root: File::open(temp.path()).unwrap(),
                index: File::open(temp.path().join("dev/block")).unwrap(),
            };
            Self { temp, sys }
        }
        fn at(&self, p: &str) -> PathBuf {
            self.temp.path().join(p)
        }
        fn dir(&self, p: &str) {
            fs::create_dir_all(self.at(p)).unwrap();
        }
        fn name(&self, body: &str, path: &str) {
            symlink(body, self.at(path)).unwrap();
        }
        fn attr(&self, p: &str, b: &str) {
            fs::write(self.at(p), b).unwrap();
        }
        fn block(&self, path: &str, id: Dev) {
            self.dir(path);
            self.attr(&format!("{path}/dev"), &format!("{id}\n"));
            self.attr(&format!("{path}/size"), "2048\n");
            self.name(&format!("../../{path}"), &format!("dev/block/{id}"));
        }
        fn nvme(&self) {
            self.dir("class/nvme");
            self.block("devices/pci-case/controller/namespace", Dev(259, 0));
            self.attr("devices/pci-case/controller/namespace/diskseq", "17\n");
            self.name(
                "../../../class/nvme",
                "devices/pci-case/controller/subsystem",
            );
            self.attr("devices/pci-case/controller/transport", "pcie\n");
        }
        fn sample(&self, dev: Dev) -> Field<Attachment> {
            let node = self.sys.pin(dev).unwrap();
            let id = node.identity(&self.sys).unwrap();
            node.attachment(&self.sys, &id)
        }
    }
    #[test]
    fn canonical_devpaths_and_transport_labels_are_bounded_and_never_guess() {
        assert_eq!(
            canonical_devpath(b"../../devices/virtual/block/loop0").unwrap(),
            Path::new("devices/virtual/block/loop0")
        );
        for bad in [
            b"../../devices/x/../y".as_slice(),
            b"../../devices/x//y",
            b"../../devices/",
            b"../../devices/./x",
            b"../../devices/a\0b",
        ] {
            assert_eq!(canonical_devpath(bad), Err(Reason::Invalid));
        }
        for bad in [
            b"../../../devices/x".as_slice(),
            b"../../class/x",
            b"/devices/x",
        ] {
            assert_eq!(canonical_devpath(bad), Err(Reason::Unsupported));
        }
        let too_deep = format!("../../devices/{}", ["p"; DEPTH + 1].join("/"));
        assert_eq!(canonical_devpath(too_deep.as_bytes()), Err(Reason::Limit));
        assert_eq!(canonical_devpath(&[b'x'; LINK_BYTES]), Err(Reason::Limit));
        assert_eq!(transport_type(b"rdma"), Ok("rdma"));
        assert_eq!(transport_type(b"future"), Err(Reason::Unsupported));
        assert_eq!(transport_type(b"tcp\t"), Err(Reason::Invalid));
    }
    #[test]
    fn actual_ancestor_relation_and_nvme_transport_are_separate_from_partition_inference() {
        let f = Fixture::new();
        f.nvme();
        let part = "devices/pci-case/controller/namespace/partition7";
        f.block(part, Dev(259, 7));
        f.attr(&format!("{part}/partition"), "7\n");
        f.attr(&format!("{part}/start"), "80\n");
        let whole = Attachment {
            subsystem: "nvme",
            source: AttachmentSource::KernelAncestor,
            nvme_transport: Ok("pcie"),
        };
        assert_eq!(f.sample(Dev(259, 0)), Ok(whole));
        assert_eq!(
            f.sample(Dev(259, 7)),
            Ok(Attachment {
                source: AttachmentSource::InferredParent,
                ..whole
            })
        );
        let node = f.sys.pin(Dev(259, 0)).unwrap();
        let id = node.identity(&f.sys).unwrap();
        let a = Cut::read(&f.sys, id.dev, id.object, 17).unwrap();
        f.attr("devices/pci-case/controller/transport", "tcp\n");
        let b = Cut::read(&f.sys, id.dev, id.object, 17).unwrap();
        assert_eq!(
            a.finish(&b, AttachmentSource::KernelAncestor),
            Ok(Attachment {
                nvme_transport: Err(Reason::Changed),
                ..whole
            })
        );
        fs::remove_file(f.at("devices/pci-case/controller/transport")).unwrap();
        assert_eq!(
            f.sample(Dev(259, 0)),
            Ok(Attachment {
                nvme_transport: Err(Reason::NotReported),
                ..whole
            })
        );
    }
    #[test]
    fn scsi_and_multipath_do_not_invent_a_transport_or_use_a_device_name() {
        let f = Fixture::new();
        f.dir("bus/scsi");
        f.dir("class/nvme-subsystem");
        f.block("devices/outer/scsi-node/block/nvme-like", Dev(8, 0));
        f.attr("devices/outer/scsi-node/block/nvme-like/diskseq", "41\n");
        f.name("../../../bus/scsi", "devices/outer/scsi-node/subsystem");
        // Even if a neighboring attribute happened to exist, this isn't NVMe.
        f.attr("devices/outer/scsi-node/transport", "pcie\n");
        f.block(
            "devices/virtual/nvme-subsystem/controller-group/n0",
            Dev(259, 1),
        );
        f.attr(
            "devices/virtual/nvme-subsystem/controller-group/n0/diskseq",
            "42\n",
        );
        f.name(
            "../../../../class/nvme-subsystem",
            "devices/virtual/nvme-subsystem/controller-group/subsystem",
        );
        for (dev, class) in [(Dev(8, 0), "scsi"), (Dev(259, 1), "nvme-subsystem")] {
            assert_eq!(
                f.sample(dev),
                Ok(Attachment {
                    subsystem: class,
                    source: AttachmentSource::KernelAncestor,
                    nvme_transport: Err(Reason::Unsupported)
                })
            );
        }
        f.block("devices/virtual/block/sda", Dev(7, 0));
        f.attr("devices/virtual/block/sda/diskseq", "43\n");
        assert_eq!(f.sample(Dev(7, 0)), Err(Reason::NotReported));
    }
    #[test]
    fn matching_text_is_not_identity_and_stale_disk_generation_is_rejected() {
        let f = Fixture::new();
        f.nvme();
        let dev = Dev(259, 0);
        let held = f.sys.pin(dev).unwrap();
        let id = held.identity(&f.sys).unwrap();
        let a = Cut::read(&f.sys, dev, id.object, 17).unwrap();
        let link = "devices/pci-case/controller/subsystem";
        fs::rename(f.at(link), f.at("devices/pci-case/controller/earlier")).unwrap();
        f.name("../../../class/nvme", link);
        let b = Cut::read(&f.sys, dev, id.object, 17).unwrap();
        assert_eq!(
            a.finish(&b, AttachmentSource::KernelAncestor),
            Err(Reason::Changed)
        );
        f.attr("devices/pci-case/controller/namespace/diskseq", "18\n");
        assert_eq!(held.attachment(&f.sys, &id), Err(Reason::Changed));

        fs::remove_file(f.at(link)).unwrap();
        f.dir("other/nvme");
        f.name("../../../other/nvme", link);
        assert_eq!(f.sample(dev), Err(Reason::Invalid));
        fs::remove_file(f.at(link)).unwrap();
        f.dir("bus/unknown");
        f.name("../../../bus/unknown", link);
        assert_eq!(f.sample(dev), Err(Reason::Unsupported));
    }
    #[test]
    fn recorded_devpath_must_still_name_the_held_whole_after_reparent() {
        let f = Fixture::new();
        f.nvme();
        let dev = Dev(259, 0);
        let initial = f.sys.pin(dev).unwrap().identity(&f.sys).unwrap();
        let cut = Cut::read(&f.sys, dev, initial.object, 17).unwrap();
        require_devpath(&f.sys, &cut.path, &cut.whole).unwrap();
        let original = f.temp.path().join(&cut.path);
        f.dir("devices/new-parent");
        let moved = f.at("devices/new-parent/namespace");
        fs::rename(&original, &moved).unwrap();
        assert_eq!(obj(&File::open(&moved).unwrap()).unwrap(), initial.object);
        assert_eq!(diskseq(&cut.whole), Ok(17));
        cut.parent.require_named(&f.sys).unwrap();
        assert_eq!(
            require_devpath(&f.sys, &cut.path, &cut.whole),
            Err(Reason::Changed)
        );
        fs::create_dir(&original).unwrap();
        assert_eq!(
            require_devpath(&f.sys, &cut.path, &cut.whole),
            Err(Reason::Changed)
        );
        fs::remove_dir(&original).unwrap();
        symlink("../../new-parent/namespace", &original).unwrap();
        assert_eq!(
            obj(&File::open(&original).unwrap()).unwrap(),
            initial.object
        );
        assert_eq!(
            require_devpath(&f.sys, &cut.path, &cut.whole),
            Err(Reason::Changed)
        );
    }
}
