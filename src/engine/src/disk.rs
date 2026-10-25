use crate::*;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{
    AtomicFile, EntryName, PrivateDirectory, RelativePath,
    linux::{MountPolicy, OpenAt2Root, SingleLinkRequirement},
};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

mod mountinfo;

pub(crate) fn fs_error(e: platform_io::fs_safety::Error) -> Error {
    match e {
        platform_io::fs_safety::Error::Io(e) if matches!(e.raw_os_error(), Some(20 | 40)) => {
            Error::new("UNSAFE_PATH", e.to_string())
        }
        platform_io::fs_safety::Error::Io(e) => e.into(),
        platform_io::fs_safety::Error::PublishedDurabilityUnknown(e) => {
            Error::new("DURABILITY_UNKNOWN", e.to_string())
        }
        platform_io::fs_safety::Error::DestinationExists(p) => {
            Error::new("DESTINATION_EXISTS", p.display().to_string())
        }
        other => Error::new("UNSAFE_PATH", other.to_string()),
    }
}
fn absolute_path(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => out.push("/"),
            Component::Normal(name) => out.push(name),
            Component::CurDir => {}
            // Resolve no parent traversals before a symlink-safe walk: accepting
            // them would change their meaning if an ancestor were a symlink.
            _ => {
                return Err(Error::new(
                    "UNSAFE_PATH",
                    "parent traversal is not allowed in managed I/O",
                ));
            }
        }
    }
    Ok(out)
}

pub(crate) const MAX_METADATA: usize = 64 * 1024 * 1024;
// A 1 TiB file with the smallest supported 64 KiB checksum chunks needs
// 512 MiB plus its fixed checksum header. Keep this separate from JSON metadata.
pub(crate) const MAX_CHECKSUM_MANIFEST: usize = 512 * 1024 * 1024 + CHECKSUM_HEADER_BYTES;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct DiskSeal {
    pub pool_id: String,
    pub disk_id: String,
    pub slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub binding_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub inode: u64,
    pub physical_device: String,
    pub filesystem_uuid: String,
    pub simulation: bool,
}
pub(crate) fn private_dir(path: &Path) -> Result<()> {
    let path = absolute_path(path)?;
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            PrivateDirectory::open_existing(&path).map_err(fs_error)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| Error::new("UNSAFE_PATH", "missing directory parent"))?;
            if !parent.exists() {
                private_dir(parent)?;
            }
            PrivateDirectory::create(&path).map_err(fs_error)?;
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
pub(crate) fn existing_private_dir(path: &Path) -> Result<PrivateDirectory> {
    PrivateDirectory::open_existing(absolute_path(path)?).map_err(fs_error)
}
pub(crate) fn sync_dir(path: &Path) -> Result<()> {
    OpenAt2Root::open(&absolute_path(path)?, MountPolicy::SameMount)
        .map_err(fs_error)?
        .directory()
        .sync_all()?;
    Ok(())
}
pub(crate) fn open_regular(path: &Path) -> Result<File> {
    open_optional_regular(path)?
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound).into())
}
/// Only an absent leaf below an opened, safe parent means no file. A missing
/// or linked parent and every other I/O failure must remain an error.
pub(crate) fn open_optional_regular(path: &Path) -> Result<Option<File>> {
    let path = absolute_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("UNSAFE_PATH", "missing file parent"))?;
    let root = OpenAt2Root::open(parent, MountPolicy::SameMount).map_err(fs_error)?;
    let name = RelativePath::new(
        path.file_name()
            .ok_or_else(|| Error::new("UNSAFE_PATH", "missing file name"))?,
    )
    .map_err(fs_error)?;
    match root.open_file(&name, false) {
        Ok(file) => Ok(Some(file)),
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(fs_error(error)),
    }
}
/// Open a journal relative to its verified parent, without following a leaf symlink.
pub(crate) fn open_append_regular(path: &Path) -> Result<File> {
    let path = absolute_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("UNSAFE_PATH", "missing file parent"))?;
    let root = OpenAt2Root::open(parent, MountPolicy::SameMount).map_err(fs_error)?;
    let name = EntryName::new(
        path.file_name()
            .ok_or_else(|| Error::new("UNSAFE_PATH", "missing file name"))?,
    )
    .map_err(fs_error)?;
    let file = File::from(
        rustix::fs::openat2(
            root.directory(),
            name.as_os_str(),
            rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::WRONLY
                | rustix::fs::OFlags::APPEND
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_raw_mode(0o600),
            rustix::fs::ResolveFlags::BENEATH
                | rustix::fs::ResolveFlags::NO_SYMLINKS
                | rustix::fs::ResolveFlags::NO_XDEV,
        )
        .map_err(|e| fs_error(platform_io::fs_safety::Error::Io(e.into())))?,
    );
    SingleLinkRequirement::check(&file).map_err(fs_error)?;
    Ok(file)
}
pub(crate) fn read_bytes(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    read_file_bytes(open_regular(path)?, maximum)
}
/// Apply the same metadata bound to an already validated, held regular file.
pub(crate) fn read_file_bytes(mut f: File, maximum: usize) -> Result<Vec<u8>> {
    let size = f.metadata()?.len();
    if size > maximum as u64 {
        return Err(Error::new("RESOURCE_LIMIT", "metadata input exceeds limit"));
    }
    let mut out = Vec::new();
    (&mut f).take(maximum as u64 + 1).read_to_end(&mut out)?;
    if out.len() > maximum {
        return Err(Error::new("RESOURCE_LIMIT", "metadata grew beyond limit"));
    }
    Ok(out)
}
pub(crate) fn read_json<T: data_serialization::de::DeserializeOwned>(path: &Path) -> Result<T> {
    decode_strict(&read_bytes(path, MAX_METADATA)?)
}
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    let path = absolute_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("UNSAFE_PATH", "missing parent"))?;
    private_dir(parent)?;
    let directory = PrivateDirectory::open_existing(parent).map_err(fs_error)?;
    let name = EntryName::new(
        path.file_name()
            .ok_or_else(|| Error::new("UNSAFE_PATH", "missing object name"))?,
    )
    .map_err(fs_error)?;
    if !replace && path.try_exists()? {
        if directory
            .read_bounded(&name, bytes.len())
            .map_err(fs_error)?
            == bytes
        {
            // A prior create may have installed these bytes before its parent
            // sync failed. Reusing an immutable object must finish that barrier.
            open_regular(&path)?.sync_all()?;
            directory.sync().map_err(fs_error)?;
            return Ok(());
        }
        return Err(Error::new(
            "OBJECT_COLLISION",
            "immutable object differs from existing bytes",
        ));
    }
    if replace {
        AtomicFile::replace(&directory, &name.as_relative(), bytes).map_err(fs_error)
    } else {
        AtomicFile::create(&directory, &name, bytes).map_err(fs_error)
    }
}
/// Every JSON object must fit the same bound used by its on-disk reader.
pub(crate) fn encode_metadata<T: Serialize + ?Sized>(value: &T, maximum: usize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > maximum {
        return Err(Error::new(
            "RESOURCE_LIMIT",
            format!("serialized metadata exceeds {maximum} byte reader limit"),
        ));
    }
    Ok(bytes)
}
pub(crate) fn write_json<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
    replace: bool,
) -> Result<()> {
    write_atomic(path, &encode_metadata(value, MAX_METADATA)?, replace)
}
pub(crate) fn full_copy(
    source: &Path,
    destination: &Path,
    chunk_bytes: u32,
    max_bytes: u64,
) -> Result<(u64, String, ChecksumManifest)> {
    let source = open_regular(source)?;
    full_copy_to_path(source, destination, chunk_bytes, max_bytes, None)
}

pub(crate) fn verified_full_copy(
    source: File,
    destination: &Path,
    version: &FileVersion,
) -> Result<()> {
    full_copy_to_path(
        source,
        destination,
        version.checksum_chunk_bytes,
        version.logical_size,
        Some(version),
    )
    .map(|_| ())
}

fn full_copy_to_path(
    source: File,
    destination: &Path,
    chunk_bytes: u32,
    max_bytes: u64,
    expected: Option<&FileVersion>,
) -> Result<(u64, String, ChecksumManifest)> {
    let destination = absolute_path(destination)?;
    let parent = destination
        .parent()
        .ok_or_else(|| Error::new("UNSAFE_PATH", "missing destination parent"))?;
    if !parent.exists() {
        private_dir(parent)?;
    }
    let directory = OpenAt2Root::open(parent, MountPolicy::SameMount).map_err(fs_error)?;
    let destination_name = EntryName::new(
        destination
            .file_name()
            .ok_or_else(|| Error::new("UNSAFE_PATH", "missing destination name"))?,
    )
    .map_err(fs_error)?;
    full_copy_checked(
        source,
        &directory,
        &destination_name,
        chunk_bytes,
        max_bytes,
        expected,
    )
    .map(|(size, hash, checksums, _file)| (size, hash, checksums))
}

pub(crate) fn full_copy_held(
    source: File,
    directory: &OpenAt2Root,
    destination_name: &EntryName,
    chunk_bytes: u32,
    max_bytes: u64,
) -> Result<(u64, String, ChecksumManifest, File)> {
    full_copy_checked(
        source,
        directory,
        destination_name,
        chunk_bytes,
        max_bytes,
        None,
    )
}

fn full_copy_checked(
    source: File,
    directory: &OpenAt2Root,
    destination_name: &EntryName,
    chunk_bytes: u32,
    max_bytes: u64,
    expected: Option<&FileVersion>,
) -> Result<(u64, String, ChecksumManifest, File)> {
    let temporary = format!(".zxs-full-{}", new_id("req"));
    let mut target = File::from(
        rustix::fs::openat(
            directory.directory(),
            temporary.as_str(),
            rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::EXCL
                | rustix::fs::OFlags::WRONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_raw_mode(0o600),
        )
        .map_err(std::io::Error::from)?,
    );
    let result = (|| {
        let mut full = blake3::Hasher::new();
        let mut chunks = Vec::new();
        let mut size = 0u64;
        let mut buf = vec![0u8; chunk_bytes as usize];
        loop {
            let mut used = 0;
            while used < buf.len() {
                let n = std::os::unix::fs::FileExt::read_at(
                    &source,
                    &mut buf[used..],
                    size + used as u64,
                )?;
                if n == 0 {
                    break;
                }
                used += n;
            }
            if used == 0 {
                break;
            }
            size = size
                .checked_add(used as u64)
                .ok_or_else(|| Error::new("FILE_TOO_LARGE", "file length overflow"))?;
            if size > max_bytes {
                return Err(Error::new(
                    "FILE_TOO_LARGE",
                    "file exceeds configured maximum",
                ));
            }
            target.write_all(&buf[..used])?;
            full.update(&buf[..used]);
            chunks.push(*blake3::hash(&buf[..used]).as_bytes());
        }
        let hash = full.finalize().to_hex().to_string();
        let checksums = ChecksumManifest {
            chunk_bytes,
            logical_size: size,
            chunks,
        };
        // Validate the bytes actually copied, not an earlier read of the source.
        // Never expose the final pathname until all published commitments match.
        if let Some(expected) = expected
            && (size != expected.logical_size
                || hash != expected.content_hash
                || blake3::hash(&checksums.encode()?).to_hex().as_str()
                    != expected.checksum_manifest_hash)
        {
            return Err(Error::new(
                "SOURCE_CHANGED",
                "exported content differs from the published manifest",
            ));
        }
        target.set_permissions(fs::Permissions::from_mode(0o400))?;
        // Persist the complete bytes and final mode together before publishing
        // the name. The rename still needs its own parent-directory barrier.
        target.sync_all()?;
        rustix::fs::renameat_with(
            directory.directory(),
            temporary.as_str(),
            directory.directory(),
            destination_name.as_os_str(),
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
        directory.directory().sync_all()?;
        Ok((size, hash, checksums))
    })();
    if result.is_err() {
        let _ = rustix::fs::unlinkat(
            directory.directory(),
            temporary.as_str(),
            rustix::fs::AtFlags::empty(),
        );
    }
    result.map(|(size, hash, checksums)| (size, hash, checksums, target))
}
pub(crate) fn verify_payload(directory: &Path, version: &FileVersion) -> Result<()> {
    verify_payload_held(directory, version).map(|_| ())
}

pub(crate) fn verify_payload_held(directory: &Path, version: &FileVersion) -> Result<File> {
    let checks = read_bytes(&directory.join("checksums.bin"), MAX_CHECKSUM_MANIFEST)?;
    if blake3::hash(&checks).to_hex().as_str() != version.checksum_manifest_hash {
        return Err(Error::new(
            "CHECKSUM_MISMATCH",
            "checksum manifest digest differs",
        ));
    }
    let manifest = ChecksumManifest::decode(&checks)?;
    if manifest.logical_size != version.logical_size
        || manifest.chunk_bytes != version.checksum_chunk_bytes
    {
        return Err(Error::new(
            "CHECKSUM_MISMATCH",
            "checksum header differs from version",
        ));
    }
    let mut file = open_regular(&crate::container_storage::content_path(directory, version)?)?;
    if file.metadata()?.len() != version.logical_size {
        return Err(Error::new("CHECKSUM_MISMATCH", "payload length differs"));
    }
    let mut full = blake3::Hasher::new();
    let mut buf = vec![0u8; manifest.chunk_bytes as usize];
    let mut remaining = manifest.logical_size;
    for expected in manifest.chunks {
        let n = remaining.min(buf.len() as u64) as usize;
        file.read_exact(&mut buf[..n])?;
        if blake3::hash(&buf[..n]).as_bytes() != &expected {
            return Err(Error::new("CHECKSUM_MISMATCH", "payload chunk differs"));
        }
        full.update(&buf[..n]);
        remaining -= n as u64;
    }
    let mut extra = [0u8; 1];
    if file.read(&mut extra)? != 0 || file.metadata()?.len() != version.logical_size {
        return Err(Error::new(
            "CHECKSUM_MISMATCH",
            "payload length changed during verification",
        ));
    }
    if full.finalize().to_hex().as_str() != version.content_hash {
        return Err(Error::new(
            "CHECKSUM_MISMATCH",
            "complete payload hash differs",
        ));
    }
    Ok(file)
}
pub(crate) fn version_dir(disk: &DiskConfig, history: &str, version: &str) -> PathBuf {
    disk.root
        .join("histories")
        .join(history)
        .join("versions")
        .join(version)
}

pub(crate) fn identity(config: &Config, disk: &DiskConfig) -> Result<DiskSeal> {
    let m = fs::symlink_metadata(&disk.root)?;
    if !m.is_dir() || m.file_type().is_symlink() {
        return Err(Error::new(
            "DISK_IDENTITY_MISMATCH",
            "disk root is not a physical directory",
        ));
    }
    let (physical_device, filesystem_uuid) = if config.mode == Mode::Development {
        (
            format!("development:{}", disk.slot_id),
            format!("development:{}", disk.disk_id),
        )
    } else {
        production_device(&disk.root)?
    };
    Ok(DiskSeal {
        pool_id: config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
        device: m.dev(),
        inode: m.ino(),
        physical_device,
        filesystem_uuid,
        simulation: config.mode == Mode::Development,
    })
}
fn production_device(root: &Path) -> Result<(String, String)> {
    let canonical = root.canonicalize()?;
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")?;
    let selected = mountinfo::resolve(&mountinfo, &canonical)?;
    if selected.kind != "btrfs" {
        return Err(Error::new(
            "FILESYSTEM_REQUIRED",
            "production requires an independent Btrfs filesystem for every disk",
        ));
    }
    let device = selected.source.canonicalize()?;
    let name = device
        .file_name()
        .ok_or_else(|| Error::new("DISK_IDENTITY_MISMATCH", "missing block identity"))?;
    let class = Path::new("/sys/class/block").join(name);
    let hardware = class.canonicalize()?;
    let mut physical = if class.join("partition").exists() {
        hardware
            .parent()
            .ok_or_else(|| Error::new("DISK_IDENTITY_MISMATCH", "partition parent missing"))?
            .to_path_buf()
    } else {
        hardware.clone()
    };
    // Device-mapper, loop, md and other virtual block devices cannot establish
    // independent physical media from this kernel view. Refuse them instead of
    // counting two mappings of one drive as independent replicas.
    if physical.starts_with("/sys/devices/virtual")
        || fs::read_dir(class.join("slaves"))?.next().is_some()
    {
        return Err(Error::new(
            "UNSUPPORTED_DEVICE_TOPOLOGY",
            "production requires directly identifiable independent drives; virtual or aggregated block devices are not supported",
        ));
    }
    // Multiple namespaces of one NVMe controller still share a physical drive.
    if let Some(name) = physical.file_name().and_then(|n| n.to_str())
        && name.starts_with("nvme")
        && name.contains('n')
    {
        let suffix = &name[4..];
        if suffix
            .split_once('n')
            .is_some_and(|(controller, namespace)| {
                !controller.is_empty()
                    && !namespace.is_empty()
                    && controller.bytes().all(|b| b.is_ascii_digit())
                    && namespace.bytes().all(|b| b.is_ascii_digit())
            })
        {
            physical = physical
                .parent()
                .ok_or_else(|| {
                    Error::new(
                        "DISK_IDENTITY_MISMATCH",
                        "NVMe controller identity is missing",
                    )
                })?
                .to_path_buf();
        }
    }
    let mut fsid = None;
    for entry in fs::read_dir("/sys/fs/btrfs")? {
        let entry = entry?;
        let devices = entry.path().join("devices");
        if !devices.is_dir() {
            continue;
        }
        let entries = fs::read_dir(&devices)?.collect::<std::io::Result<Vec<_>>>()?;
        let belongs = entries
            .iter()
            .any(|e| e.path().canonicalize().ok().as_ref() == Some(&hardware));
        if belongs {
            if entries.len() != 1 {
                return Err(Error::new(
                    "MULTI_DEVICE_FILESYSTEM",
                    "Btrfs must contain exactly one physical device",
                ));
            }
            fsid = Some(entry.file_name().to_string_lossy().into_owned());
            break;
        }
    }
    Ok((
        physical.to_string_lossy().into_owned(),
        fsid.ok_or_else(|| {
            Error::new(
                "DISK_IDENTITY_MISMATCH",
                "cannot verify Btrfs device count and UUID",
            )
        })?,
    ))
}
/// An unmounted root is unavailable only when absent or a safe, empty directory.
/// A missing seal in a nonempty root is corruption, not permission to initialize.
pub(crate) fn disk_unavailable(disk: &DiskConfig) -> Result<bool> {
    match fs::symlink_metadata(&disk.root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error.into()),
        Ok(_) => (),
    }
    existing_private_dir(&disk.root)?;
    match fs::read_dir(&disk.root)?.next() {
        None => Ok(true),
        Some(Ok(_)) => Ok(false),
        Some(Err(error)) => Err(error.into()),
    }
}

pub(crate) fn validate_seal(config: &Config, disk: &DiskConfig) -> Result<DiskSeal> {
    let stored: DiskSeal = read_json(&disk.root.join("disk.json"))?;
    let live = identity(config, disk)?;
    if stored.pool_id != live.pool_id
        || stored.disk_id != live.disk_id
        || stored.slot_id != live.slot_id
        || stored.binding_generation != live.binding_generation
        || stored.inode != live.inode
        || stored.physical_device != live.physical_device
        || stored.filesystem_uuid != live.filesystem_uuid
        || stored.simulation != live.simulation
    {
        return Err(Error::new(
            "DISK_IDENTITY_MISMATCH",
            format!("disk {} identity changed", disk.slot_id),
        ));
    }
    Ok(live)
}
pub(crate) fn available(root: &Path) -> Result<u64> {
    let s = rustix::fs::statvfs(root).map_err(std::io::Error::from)?;
    Ok(s.f_bavail.saturating_mul(s.f_frsize))
}

pub fn offline_check(root: &Path) -> Result<CheckReport> {
    let _seal: DiskSeal = read_json(&root.join("disk.json"))?;
    let mut report = CheckReport {
        checked_versions: 0,
        healthy_replicas: 0,
        tombstoned_versions: 0,
        issues: Vec::new(),
    };
    let histories = root.join("histories");
    if !histories.exists() {
        return Ok(report);
    }
    for h in fs::read_dir(histories)? {
        let h = h?;
        if !h.file_type()?.is_dir() {
            continue;
        }
        let versions = h.path().join("versions");
        if !versions.exists() {
            continue;
        }
        for v in fs::read_dir(versions)? {
            let v = v?;
            if !v.file_type()?.is_dir() {
                continue;
            }
            report.checked_versions += 1;
            if v.path().join("purged.json").exists() {
                let result = read_json::<FileVersion>(&v.path().join("version.json"))
                    .and_then(|version| crate::journal::verify_tombstone(&v.path(), &version));
                match result {
                    Ok(()) => report.tombstoned_versions += 1,
                    Err(e) => report
                        .issues
                        .push(format!("{}: {e}", v.file_name().to_string_lossy())),
                }
                continue;
            }
            let check = (|| {
                let version: FileVersion = read_json(&v.path().join("version.json"))?;
                validate_id(&version.version_id, "ver")?;
                name_policy::validate_name(&version.creation_name)?;
                crate::journal::verify_local_publication(&v.path(), &version)?;
                verify_payload(&v.path(), &version)
            })();
            match check {
                Ok(()) => report.healthy_replicas += 1,
                Err(e) => report
                    .issues
                    .push(format!("{}: {}", v.file_name().to_string_lossy(), e)),
            }
        }
    }
    Ok(report)
}
pub fn offline_export(root: &Path, version_id: &str, destination: &Path) -> Result<()> {
    validate_id(version_id, "ver")?;
    let _seal: DiskSeal = read_json(&root.join("disk.json"))?;
    for h in fs::read_dir(root.join("histories"))? {
        let h = h?;
        if !h.file_type()?.is_dir() {
            continue;
        }
        let directory = h.path().join("versions").join(version_id);
        if !directory.exists() {
            continue;
        }
        let version: FileVersion = read_json(&directory.join("version.json"))?;
        name_policy::validate_name(&version.creation_name)?;
        crate::journal::verify_local_publication(&directory, &version)?;
        let source = verify_payload_held(&directory, &version)?;
        if destination.exists() {
            return Err(Error::new(
                "DESTINATION_EXISTS",
                "export does not overwrite existing files",
            ));
        }
        verified_full_copy(source, destination, &version)?;
        return Ok(());
    }
    Err(Error::new(
        "VERSION_NOT_FOUND",
        "version is not present on this disk",
    ))
}

#[cfg(test)]
#[path = "../../../test/engine/unit/disk/export_tests.rs"]
mod export_tests;
