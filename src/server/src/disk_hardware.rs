//! Best-effort display facts, never disk identity or storage admission evidence.
use std::{
    fs::{self, File},
    io::{self, Read},
    os::{
        fd::BorrowedFd,
        unix::fs::{FileTypeExt, MetadataExt},
    },
    path::{Component, Path, PathBuf},
};

use error_definition::Error;
use rustix::fs::{AtFlags, StatxFlags};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HardwareMapping {
    Single,
    Multiple,
    Unknown,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct HardwareObservation {
    pub model: Option<String>,
    pub serial: Option<String>,
    /// Whole block-device/namespace capacity, not filesystem or partition capacity.
    /// This does not assert physical independence or total controller capacity.
    pub capacity_bytes: Option<u64>,
    /// Current mounted block-device path, not a persistent disk identity.
    pub device_path: Option<String>,
    pub mount_path: Option<String>,
    pub filesystem_uuid: Option<String>,
    pub mapping: HardwareMapping,
}

impl HardwareObservation {
    pub(crate) fn unknown() -> Self {
        Self {
            model: None,
            serial: None,
            capacity_bytes: None,
            device_path: None,
            mount_path: None,
            filesystem_uuid: None,
            mapping: HardwareMapping::Unknown,
        }
    }
}

#[derive(Debug, Error)]
enum ObservationError {
    #[error("{operation} {path}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid observation in {path}: {detail}")]
    Invalid { path: PathBuf, detail: &'static str },
    #[error("mount ID {0} is no longer present in mountinfo")]
    MountMissing(u64),
}

struct ObservationPaths<'a> {
    mountinfo: &'a Path,
    sysfs: &'a Path,
    filesystem_uuids: &'a Path,
}

#[derive(Clone, Copy)]
struct RootMount {
    id: u64,
    major: u32,
    minor: u32,
}

struct ObservationReport {
    value: HardwareObservation,
    errors: Vec<ObservationError>,
}

impl ObservationReport {
    fn record<T>(&mut self, result: Result<T, ObservationError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.errors.push(error);
                None
            }
        }
    }
}

/// `root_path` is diagnostic text only; all filesystem observations start from
/// the caller's verified, held directory descriptor, never its replaceable name.
pub(crate) fn observe(root: &Path, root_fd: BorrowedFd<'_>) -> HardwareObservation {
    let root_mount = match read_root_mount(root, root_fd) {
        Ok(value) => value,
        Err(error) => {
            tracing::debug!(root = %root.display(), error = ?error, "disk hardware observation unavailable");
            return HardwareObservation::unknown();
        }
    };
    let report = collect(
        root_mount,
        &ObservationPaths {
            mountinfo: Path::new("/proc/self/mountinfo"),
            sysfs: Path::new("/sys"),
            filesystem_uuids: Path::new("/dev/disk/by-uuid"),
        },
        block_device_number,
    );
    if !report.errors.is_empty() {
        tracing::debug!(root = %root.display(), errors = ?report.errors, "disk hardware observation incomplete");
    }
    report.value
}

fn read_root_mount(root: &Path, root_fd: BorrowedFd<'_>) -> Result<RootMount, ObservationError> {
    let stat = rustix::fs::statx(
        root_fd,
        "",
        AtFlags::EMPTY_PATH | AtFlags::NO_AUTOMOUNT,
        StatxFlags::MNT_ID,
    )
    .map_err(|source| io_error("statx disk root", root, source.into()))?;
    if stat.stx_mask & StatxFlags::MNT_ID.bits() == 0 {
        return Err(invalid(root, "kernel did not return a mount ID"));
    }
    Ok(RootMount {
        id: stat.stx_mnt_id,
        major: stat.stx_dev_major,
        minor: stat.stx_dev_minor,
    })
}

fn collect(
    root: RootMount,
    paths: &ObservationPaths<'_>,
    block_number: impl FnMut(&Path) -> Result<Option<(u32, u32)>, ObservationError>,
) -> ObservationReport {
    let mut report = ObservationReport {
        value: HardwareObservation::unknown(),
        errors: Vec::new(),
    };
    let mount = read_text(paths.mountinfo, 1024 * 1024)
        .and_then(|text| mount_path(&text, root, paths.mountinfo));
    let Some(mount) = report.record(mount) else {
        return report;
    };
    report.value.mount_path = Some(mount);
    report.value.filesystem_uuid = report
        .record(filesystem_uuid(paths.filesystem_uuids, root, block_number))
        .flatten();
    let link = paths
        .sysfs
        .join(format!("dev/block/{}:{}", root.major, root.minor));
    let Some(block) = report.record(
        fs::canonicalize(&link).map_err(|source| io_error("resolve block device", &link, source)),
    ) else {
        return report;
    };
    if !block.starts_with(paths.sysfs.join("devices")) {
        report
            .errors
            .push(invalid(&link, "block device is outside sysfs devices"));
        return report;
    }
    let Some(mounted) = report.record(read_device(&block)) else {
        return report;
    };
    if (mounted.major, mounted.minor) != (root.major, root.minor) {
        // Contradictory device metadata invalidates block-associated facts.
        report.value.filesystem_uuid = None;
        report
            .errors
            .push(invalid(&block, "block device number changed"));
        return report;
    }
    report.value.device_path = Some(format!("/dev/{}", mounted.name));
    collect_hardware(block, mounted.kind, paths.sysfs, &mut report);
    report
}

fn collect_hardware(
    block: PathBuf,
    kind: DeviceKind,
    sysfs: &Path,
    report: &mut ObservationReport,
) {
    let disk = match kind {
        DeviceKind::Disk => block,
        DeviceKind::Partition => {
            let parent = block
                .parent()
                .expect("invariant: sysfs device has a parent");
            let Some(device) = report.record(read_device(parent)) else {
                return;
            };
            if device.kind != DeviceKind::Disk {
                report
                    .errors
                    .push(invalid(parent, "partition parent is not a whole disk"));
                return;
            }
            parent.to_owned()
        }
    };
    let Some(dependencies) = report.record(dependency_count(&disk)) else {
        return;
    };
    if dependencies > 1 {
        report.value.mapping = HardwareMapping::Multiple;
        return;
    }
    // Do not flatten dm/md, loop, NVMe multipath or other virtual devices into
    // a guessed physical disk, even when only one dependency is currently visible.
    if dependencies != 0 || disk.starts_with(sysfs.join("devices/virtual")) {
        return;
    }
    report.value.mapping = HardwareMapping::Single;
    report.value.capacity_bytes = report.record(read_capacity(&disk));
    if let Some(Some(controller)) = report.record(nvme_controller(&disk, sysfs)) {
        report.value.model = report
            .record(read_attribute(&controller.join("model")))
            .flatten();
        report.value.serial = report
            .record(read_attribute(&controller.join("serial")))
            .flatten();
    }
}

fn mount_path(text: &str, root: RootMount, path: &Path) -> Result<String, ObservationError> {
    for line in text.lines() {
        let mut fields = line.split(' ');
        let id = fields
            .next()
            .ok_or_else(|| invalid(path, "missing mount ID"))?
            .parse::<u64>()
            .map_err(|_| invalid(path, "invalid mount ID"))?;
        if id != root.id {
            continue;
        }
        let _parent = fields
            .next()
            .ok_or_else(|| invalid(path, "missing parent mount ID"))?;
        let device = fields
            .next()
            .ok_or_else(|| invalid(path, "missing mount device number"))?;
        if device != format!("{}:{}", root.major, root.minor) {
            return Err(invalid(path, "mount device number changed"));
        }
        let _root = fields
            .next()
            .ok_or_else(|| invalid(path, "missing mount root"))?;
        let target = fields
            .next()
            .ok_or_else(|| invalid(path, "missing mount path"))?;
        let _options = fields
            .next()
            .ok_or_else(|| invalid(path, "missing mount options"))?;
        if !fields.any(|field| field == "-") {
            return Err(invalid(path, "missing mountinfo separator"));
        }
        if !matches!(
            (fields.next(), fields.next(), fields.next(), fields.next()),
            (Some(_), Some(_), Some(_), None)
        ) {
            return Err(invalid(path, "invalid mountinfo filesystem fields"));
        }
        return unescape_mount_path(target, path);
    }
    Err(ObservationError::MountMissing(root.id))
}

fn unescape_mount_path(value: &str, source: &Path) -> Result<String, ObservationError> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut rest = value.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        if byte != b'\\' {
            bytes.push(byte);
            rest = tail;
            continue;
        }
        let (escaped, tail) = rest
            .split_at_checked(4)
            .ok_or_else(|| invalid(source, "truncated mount path escape"))?;
        bytes.push(match escaped {
            b"\\040" => b' ',
            b"\\011" => b'\t',
            b"\\012" => b'\n',
            b"\\134" => b'\\',
            _ => return Err(invalid(source, "invalid mount path escape")),
        });
        rest = tail;
    }
    let path = String::from_utf8(bytes).map_err(|_| invalid(source, "mount path is not UTF-8"))?;
    if !Path::new(&path).is_absolute() {
        return Err(invalid(source, "mount path is not absolute"));
    }
    Ok(path)
}

#[derive(PartialEq, Eq)]
enum DeviceKind {
    Disk,
    Partition,
}

struct Device {
    major: u32,
    minor: u32,
    name: String,
    kind: DeviceKind,
}

fn read_device(block: &Path) -> Result<Device, ObservationError> {
    let path = block.join("uevent");
    let text = read_text(&path, 16 * 1024)?;
    let property = |key| {
        text.lines()
            .find_map(|line| line.strip_prefix(key))
            .ok_or_else(|| invalid(&path, "missing block-device property"))
    };
    let number = |key| {
        property(key)?
            .parse::<u32>()
            .map_err(|_| invalid(&path, "invalid block-device number"))
    };
    let name = property("DEVNAME=")?;
    if name.is_empty()
        || !Path::new(name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(invalid(&path, "invalid device name"));
    }
    let kind = match property("DEVTYPE=")? {
        "disk" => DeviceKind::Disk,
        "partition" => DeviceKind::Partition,
        _ => return Err(invalid(&path, "unsupported block-device type")),
    };
    Ok(Device {
        major: number("MAJOR=")?,
        minor: number("MINOR=")?,
        name: name.to_owned(),
        kind,
    })
}

fn nvme_controller(disk: &Path, sysfs: &Path) -> Result<Option<PathBuf>, ObservationError> {
    let link = disk.join("device");
    let controller = fs::canonicalize(&link)
        .map_err(|source| io_error("resolve block-device controller", &link, source))?;
    let link = controller.join("subsystem");
    let subsystem = fs::canonicalize(&link)
        .map_err(|source| io_error("resolve controller subsystem", &link, source))?;
    // Identify the subsystem through sysfs, never by parsing an nvmeXnY name.
    Ok((subsystem == sysfs.join("class/nvme")).then_some(controller))
}

fn read_attribute(path: &Path) -> Result<Option<String>, ObservationError> {
    let text = read_text(path, 4096)?;
    let value = text.trim();
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

fn block_device_number(path: &Path) -> Result<Option<(u32, u32)>, ObservationError> {
    // stat the symlink target only; never open or probe the device itself.
    let metadata =
        fs::metadata(path).map_err(|source| io_error("stat filesystem UUID link", path, source))?;
    Ok(metadata.file_type().is_block_device().then(|| {
        (
            rustix::fs::major(metadata.rdev()),
            rustix::fs::minor(metadata.rdev()),
        )
    }))
}

fn filesystem_uuid(
    directory: &Path,
    root: RootMount,
    mut block_number: impl FnMut(&Path) -> Result<Option<(u32, u32)>, ObservationError>,
) -> Result<Option<String>, ObservationError> {
    let entries = fs::read_dir(directory)
        .map_err(|source| io_error("read filesystem UUID links", directory, source))?;
    let mut found = None;
    for (count, entry) in entries.enumerate() {
        if count == 4096 {
            return Err(invalid(
                directory,
                "filesystem UUID link count exceeds observation limit",
            ));
        }
        let entry =
            entry.map_err(|source| io_error("read filesystem UUID entry", directory, source))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|source| io_error("read filesystem UUID link type", &path, source))?;
        if !kind.is_symlink() || block_number(&path)? != Some((root.major, root.minor)) {
            continue;
        }
        if found.is_some() {
            return Err(invalid(
                directory,
                "multiple filesystem UUID links match the mounted device",
            ));
        }
        found = Some(
            entry
                .file_name()
                .into_string()
                .map_err(|_| invalid(&path, "filesystem UUID is not UTF-8"))?,
        );
    }
    Ok(found)
}

fn dependency_count(disk: &Path) -> Result<usize, ObservationError> {
    let path = disk.join("slaves");
    let entries =
        fs::read_dir(&path).map_err(|source| io_error("read block dependencies", &path, source))?;
    let mut count = 0;
    // Only zero, one, or multiple is relevant; no recursive topology traversal.
    for entry in entries.take(2) {
        entry.map_err(|source| io_error("read block dependency", &path, source))?;
        count += 1;
    }
    Ok(count)
}

fn read_capacity(disk: &Path) -> Result<u64, ObservationError> {
    let path = disk.join("size");
    let sectors = read_text(&path, 64)?
        .trim()
        .parse::<u64>()
        .map_err(|_| invalid(&path, "invalid block-device sector count"))?;
    sectors
        .checked_mul(512)
        .ok_or_else(|| invalid(&path, "block-device capacity overflows bytes"))
}

fn read_text(path: &Path, limit: u64) -> Result<String, ObservationError> {
    let file =
        File::open(path).map_err(|source| io_error("open observation file", path, source))?;
    let mut text = String::new();
    file.take(limit + 1)
        .read_to_string(&mut text)
        .map_err(|source| io_error("read observation file", path, source))?;
    if text.len() as u64 > limit {
        return Err(invalid(path, "observation exceeds read limit"));
    }
    Ok(text)
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> ObservationError {
    ObservationError::Io {
        operation,
        path: path.to_owned(),
        source,
    }
}

fn invalid(path: &Path, detail: &'static str) -> ObservationError {
    ObservationError::Invalid {
        path: path.to_owned(),
        detail,
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/disk_hardware.rs"]
mod tests;
