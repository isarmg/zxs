//! Ordinary input is an immutable inventory, admitted through one held root.
use super::{invalid, io};
use crate::Result;
use crate::persist::layout;
use name_policy::PortablePath;
use platform_io::fs_safety::{
    RelativePath,
    linux::{MountPolicy, OpenAt2Root},
};
use rustix::fs::{AtFlags, FileType, statat};
use std::{
    collections::BTreeMap,
    fs::{File, Metadata},
    os::unix::fs::MetadataExt,
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    mode: u32,
}
impl Stamp {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            mode: metadata.mode(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Directory,
    File,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub path: PortablePath,
    pub kind: Kind,
    stamp: Stamp,
}
impl Item {
    pub fn size(&self) -> u64 {
        self.stamp.size
    }
}
#[derive(Clone, Copy)]
pub struct Limits {
    pub entries: usize,
    pub bytes: u64,
    pub file_bytes: u64,
}
#[derive(PartialEq, Eq)]
struct Inventory {
    items: BTreeMap<String, Item>,
    root: Stamp,
    bytes: u64,
}

pub struct Source {
    root: OpenAt2Root,
    inventory: Inventory,
    limits: Limits,
}
impl Source {
    pub fn hold(file: File, path: &Path, limits: Limits) -> Result<Self> {
        let root = OpenAt2Root::from_directory(file, MountPolicy::SameMount)?;
        let named = OpenAt2Root::open(path, MountPolicy::SameMount)?;
        if named.identity() != root.identity() {
            return Err(invalid("source directory identity changed"));
        }
        let inventory = scan(&root, limits)?;
        Ok(Self {
            root,
            inventory,
            limits,
        })
    }
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.inventory.items.values()
    }
    pub fn update_digest(&self, digest: &mut blake3::Hasher) {
        update_stamp(digest, &self.inventory.root);
        for item in self.items() {
            digest.update(&(item.path.as_str().len() as u64).to_le_bytes());
            digest.update(item.path.as_str().as_bytes());
            digest.update(&[match item.kind {
                Kind::Directory => 0,
                Kind::File => 1,
            }]);
            update_stamp(digest, &item.stamp);
        }
    }
    pub fn count(&self) -> usize {
        self.inventory.items.len()
    }
    pub fn bytes(&self) -> u64 {
        self.inventory.bytes
    }
    pub fn open_file(&self, item: &Item) -> Result<File> {
        let file = self
            .root
            .open_file(&RelativePath::new(item.path.as_str())?, false)?;
        self.verify_file(item, &file)?;
        Ok(file)
    }
    pub fn verify_file(&self, item: &Item, file: &File) -> Result<()> {
        let metadata = file
            .metadata()
            .map_err(|error| io("inspect source file", item.path.as_str(), error))?;
        if Stamp::from_metadata(&metadata) != item.stamp {
            return Err(invalid(format!(
                "source changed during conversion: {:?}",
                item.path.as_str()
            )));
        }
        Ok(())
    }
    pub fn verify_named(&self, path: &Path) -> Result<()> {
        let named = OpenAt2Root::open(path, MountPolicy::SameMount)?;
        if named.identity() != self.root.identity() {
            return Err(invalid("source directory identity changed"));
        }
        Ok(())
    }
    pub fn verify_unchanged(&self) -> Result<()> {
        if scan(&self.root, self.limits)? != self.inventory {
            return Err(invalid(
                "source tree changed during conversion; incomplete output is preserved",
            ));
        }
        Ok(())
    }
}

fn scan(root: &OpenAt2Root, limits: Limits) -> Result<Inventory> {
    let before = root
        .directory()
        .metadata()
        .map_err(|error| io("inspect source directory", ".", error))?;
    let mut inventory = Inventory {
        items: BTreeMap::new(),
        root: Stamp::from_metadata(&before),
        bytes: 0,
    };
    let mut pending = vec![String::new()];
    while let Some(parent) = pending.pop() {
        let directory = if parent.is_empty() {
            root.directory()
                .try_clone()
                .map_err(|error| io("retain source root", ".", error))?
        } else {
            root.open_directory(&RelativePath::new(&parent)?)?
        };
        let held = OpenAt2Root::from_directory(directory, MountPolicy::SameMount)?;
        // A selected ordinary parent does not authorize treating a nested ZXS
        // store as ordinary contents. Apply the same bounded classification to
        // every held directory, including each pre-publication inventory scan.
        reject_zxs_root(&held, limits.entries)?;
        for entry in held.entries()? {
            let entry = entry?;
            let name = entry
                .as_os_str()
                .to_str()
                .ok_or_else(|| invalid("ordinary filenames must be valid UTF-8"))?;
            let path = PortablePath::parse(&if parent.is_empty() {
                name.into()
            } else {
                format!("{parent}/{name}")
            })?;
            let metadata = statat(
                held.directory(),
                entry.as_os_str(),
                AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(|error| io("inspect source entry", path.as_str(), error.into()))?;
            let (kind, stamp) =
                match FileType::from_raw_mode(metadata.st_mode) {
                    FileType::Directory => {
                        let file = root.open_directory(&RelativePath::new(path.as_str())?)?;
                        let stamp = Stamp::from_metadata(&file.metadata().map_err(|error| {
                            io("inspect source directory", path.as_str(), error)
                        })?);
                        pending.push(path.as_str().to_owned());
                        (Kind::Directory, stamp)
                    }
                    FileType::RegularFile => {
                        let file = root.open_file(&RelativePath::new(path.as_str())?, false)?;
                        let stamp =
                            Stamp::from_metadata(&file.metadata().map_err(|error| {
                                io("inspect source file", path.as_str(), error)
                            })?);
                        if stamp.size > limits.file_bytes {
                            return Err(invalid("source file exceeds the file byte limit"));
                        }
                        inventory.bytes = inventory
                            .bytes
                            .checked_add(stamp.size)
                            .filter(|bytes| *bytes <= limits.bytes)
                            .ok_or_else(|| invalid("source exceeds total byte limit"))?;
                        (Kind::File, stamp)
                    }
                    _ => {
                        return Err(invalid(format!(
                            "symlinks and special files are not supported: {:?}",
                            path.as_str()
                        )));
                    }
                };
            if (stamp.device, stamp.inode) != (metadata.st_dev, metadata.st_ino) {
                return Err(invalid(format!(
                    "source entry changed while scanning: {:?}",
                    path.as_str()
                )));
            }
            if inventory.items.len() >= limits.entries {
                return Err(invalid("source exceeds entry limit"));
            }
            let key = path.key().to_owned();
            if inventory
                .items
                .insert(key, Item { path, kind, stamp })
                .is_some()
            {
                return Err(invalid(
                    "source contains names that collide under ZXS Unicode/case rules",
                ));
            }
        }
    }
    let after = root
        .directory()
        .metadata()
        .map_err(|error| io("recheck source root", ".", error))?;
    if Stamp::from_metadata(&after) != inventory.root {
        return Err(invalid("source root changed while scanning"));
    }
    Ok(inventory)
}

fn update_stamp(digest: &mut blake3::Hasher, stamp: &Stamp) {
    for value in [stamp.device, stamp.inode, stamp.size] {
        digest.update(&value.to_le_bytes());
    }
    for value in [
        stamp.modified.0,
        stamp.modified.1,
        stamp.changed.0,
        stamp.changed.1,
    ] {
        digest.update(&value.to_le_bytes());
    }
    digest.update(&stamp.mode.to_le_bytes());
}

fn reject_zxs_root(root: &OpenAt2Root, entry_limit: usize) -> Result<()> {
    use std::io::Read;
    // Reject an incomplete/corrupt ZXS directory as well as a valid seal. Never
    // downgrade missing identity evidence into permission for contents-only copy.
    match root.open_directory(&RelativePath::new("metadata")?) {
        Ok(directory) => {
            for reserved in ["pool-membership", "application-log", "objects"] {
                match statat(&directory, reserved, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(_) => {
                        return Err(invalid(
                            "source contains reserved ZXS metadata; use verified history-preserving recovery",
                        ));
                    }
                    Err(rustix::io::Errno::NOENT) => {}
                    Err(error) => {
                        return Err(io(
                            "inspect reserved source metadata",
                            reserved,
                            error.into(),
                        ));
                    }
                }
            }
        }
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(rustix::io::Errno::NOTDIR.raw_os_error()) => {}
        Err(error) => return Err(error.into()),
    }
    match root.open_directory(&RelativePath::new("histories")?) {
        Ok(directory) => {
            let histories = OpenAt2Root::from_directory(directory, MountPolicy::SameMount)?;
            for (index, entry) in histories.entries()?.enumerate() {
                if index >= entry_limit {
                    return Err(invalid(
                        "source exceeds entry limit during format classification",
                    ));
                }
                let entry = entry?;
                if entry
                    .as_os_str()
                    .as_encoded_bytes()
                    .starts_with(b"history_")
                {
                    return Err(invalid(
                        "source contains ZXS history storage; ordinary import cannot preserve its history semantics",
                    ));
                }
            }
        }
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(rustix::io::Errno::NOTDIR.raw_os_error()) => {}
        Err(error) => return Err(error.into()),
    }
    let file = match root.open_file(&layout::DISK_IDENTITY.as_relative(), false) {
        Ok(file) => file,
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(crate::disk::MAX_METADATA as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > crate::disk::MAX_METADATA {
        return Err(invalid("disk.json exceeds source classification limit"));
    }
    let value: json_codec::Value = crate::decode_strict(&bytes).map_err(|source| {
        crate::Error::from(source).context(crate::ErrorCode::FormatUnsupported, "ambiguous or corrupt disk.json is not an ordinary source; inspect it before converting")
    })?;
    if value.get("pool_id").is_some()
        || value.get("disk_id").is_some()
        || value.get("slot_id").is_some()
        || value.get("format").is_some()
    {
        return Err(invalid(
            "ZXS or unknown storage cannot use contents-only import; verified history, versions and branches must be retained",
        ));
    }
    Ok(())
}
