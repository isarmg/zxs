//! Optional read-only SMART observations. Never a storage-admission input.
use super::{
    DeviceKind, HardwareMapping, HardwareObservation, ObservationError, RootMount,
    dependency_count, nvme_controller, read_attribute, read_device, read_root_mount,
};
use std::{
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
};
use std::{
    os::fd::{AsFd, AsRawFd},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use storage_contract::disk_details::{
    DiskHealth, DiskHealthAssessment, DiskHealthUnavailable as Reason,
};

static QUERY_ACTIVE: AtomicBool = AtomicBool::new(false);
const OUTPUT_LIMIT: usize = 64 * 1024;
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

struct QuerySlot;
impl Drop for QuerySlot {
    fn drop(&mut self) {
        QUERY_ACTIVE.store(false, Ordering::Release);
    }
}

pub(crate) fn observe(root: &Path, directory: &File, hardware: &HardwareObservation) -> DiskHealth {
    let reason = match hardware.mapping {
        HardwareMapping::Multiple => Some(Reason::MultipleDevices),
        HardwareMapping::Unknown => Some(Reason::MappingUnknown),
        HardwareMapping::Single => None,
    };
    if let Some(reason) = reason {
        return unavailable(reason);
    }
    if QUERY_ACTIVE
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return unavailable(Reason::Busy);
    }
    let slot = QuerySlot;
    let directory = match directory.try_clone() {
        Ok(directory) => directory,
        Err(error) => return unavailable(io_reason("clone registered directory", error)),
    };
    let root = root.to_owned();
    let mounted_path = hardware.device_path.clone();
    let (send, receive) = mpsc::sync_channel(1);
    // At most one dedicated worker, including kill/reap. A kernel-stuck ioctl
    // retains the slot rather than consuming arbitrarily many HTTP/blocking jobs.
    let worker = std::thread::Builder::new()
        .name("disk-smart".into())
        .spawn(move || {
            let _slot = slot;
            let result = query(&root, &directory, mounted_path.as_deref());
            if send.send(result).is_err() {
                tracing::debug!("SMART observation completed after caller deadline");
            }
        });
    if let Err(error) = worker {
        return unavailable(io_reason("start SMART worker", error));
    }
    match receive.recv_timeout(QUERY_TIMEOUT + Duration::from_secs(1)) {
        Ok(Ok(health)) => health,
        Ok(Err(reason)) => unavailable(reason),
        Err(mpsc::RecvTimeoutError::Timeout) => unavailable(Reason::TimedOut),
        Err(mpsc::RecvTimeoutError::Disconnected) => unavailable(Reason::QueryFailed),
    }
}

pub(crate) fn unavailable(reason: Reason) -> DiskHealth {
    DiskHealth::Unavailable { reason }
}

struct DeviceBinding {
    mount: RootMount,
    sysfs_path: PathBuf,
    sysfs: File,
    mounted_sysfs_path: PathBuf,
    mounted_sysfs: File,
    device: File,
    device_path: String,
    protocol: &'static str,
    number: (u32, u32),
}

fn query(root: &Path, directory: &File, mounted_path: Option<&str>) -> Result<DiskHealth, Reason> {
    let binding = bind(root, directory, mounted_path)?;
    let proc_fd = format!(
        "/proc/{}/fd/{}",
        std::process::id(),
        binding.device.as_raw_fd()
    );
    let mut command = Command::new("/usr/sbin/smartctl");
    command.env_clear().env("LC_ALL", "C").args([
        "--json",
        "--health",
        "--attributes",
        "--device",
        binding.protocol,
    ]);
    if binding.protocol == "sat" {
        // Fail closed if checking the power mode is unsupported; do not spin up.
        command.args(["--nocheck=standby,3,3"]);
    }
    command
        .arg(&proc_fd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    binding.revalidate(root, directory)?;
    let (status, bytes) = run(&mut command, QUERY_TIMEOUT)?;
    binding.revalidate(root, directory)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            tracing::debug!(?error, "SMART observation clock precedes Unix epoch");
            Reason::QueryFailed
        })?;
    let observed_at = u64::try_from(now.as_millis()).map_err(|_| Reason::QueryFailed)?;
    parse(&bytes, status, observed_at, binding.device_path)
}

fn bind(
    root: &Path,
    directory: &File,
    mounted_path: Option<&str>,
) -> Result<DeviceBinding, Reason> {
    let mount = read_root_mount(root, directory.as_fd()).map_err(observation_reason)?;
    let link = PathBuf::from(format!("/sys/dev/block/{}:{}", mount.major, mount.minor));
    let block =
        fs::canonicalize(&link).map_err(|error| io_reason("resolve SMART device", error))?;
    if !block.starts_with("/sys/devices") || block.starts_with("/sys/devices/virtual") {
        return Err(Reason::UnsupportedDevice);
    }
    let mounted = read_device(&block).map_err(observation_reason)?;
    if (mounted.major, mounted.minor) != (mount.major, mount.minor)
        || mounted_path != Some(format!("/dev/{}", mounted.name).as_str())
    {
        return Err(Reason::DeviceChanged);
    }
    let mounted_sysfs =
        File::open(&block).map_err(|error| io_reason("hold mounted sysfs identity", error))?;
    let mounted_sysfs_path = block.clone();
    let disk = match mounted.kind {
        DeviceKind::Disk => block,
        DeviceKind::Partition => block
            .parent()
            .expect("invariant: sysfs block has parent")
            .to_owned(),
    };
    let device = read_device(&disk).map_err(observation_reason)?;
    if device.kind != DeviceKind::Disk {
        return Err(Reason::UnsupportedDevice);
    }
    match dependency_count(&disk).map_err(observation_reason)? {
        0 => {}
        1 => return Err(Reason::UnsupportedDevice),
        _ => return Err(Reason::MultipleDevices),
    }
    let protocol = if nvme_controller(&disk, Path::new("/sys"))
        .map_err(observation_reason)?
        .is_some()
    {
        "nvme"
    } else if read_attribute(&disk.join("device/vendor"))
        .map_err(observation_reason)?
        .as_deref()
        == Some("ATA")
    {
        "sat"
    } else {
        return Err(Reason::UnsupportedDevice);
    };
    let sysfs = File::open(&disk).map_err(|error| io_reason("hold SMART sysfs identity", error))?;
    let device_path = format!("/dev/{}", device.name);
    let fd = rustix::fs::open(
        &device_path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|error| io_reason("open SMART device read-only", error.into()))?;
    let device_file = File::from(fd);
    let metadata = device_file
        .metadata()
        .map_err(|error| io_reason("stat held SMART device", error))?;
    let number = (device.major, device.minor);
    if !metadata.file_type().is_block_device()
        || (
            rustix::fs::major(metadata.rdev()),
            rustix::fs::minor(metadata.rdev()),
        ) != number
    {
        return Err(Reason::DeviceChanged);
    }
    Ok(DeviceBinding {
        mount,
        sysfs_path: disk,
        sysfs,
        mounted_sysfs_path,
        mounted_sysfs,
        device: device_file,
        device_path,
        protocol,
        number,
    })
}

impl DeviceBinding {
    fn revalidate(&self, root: &Path, directory: &File) -> Result<(), Reason> {
        let mount = read_root_mount(root, directory.as_fd()).map_err(observation_reason)?;
        same_held_inode(&self.device, Path::new(&self.device_path))?;
        self.revalidate_mount(mount, Path::new("/sys"))
    }

    fn revalidate_mount(&self, mount: RootMount, sysfs: &Path) -> Result<(), Reason> {
        if mount != self.mount {
            return Err(Reason::DeviceChanged);
        }
        same_held_inode(&self.sysfs, &self.sysfs_path)?;
        same_held_inode(&self.mounted_sysfs, &self.mounted_sysfs_path)?;
        let link = sysfs.join(format!("dev/block/{}:{}", mount.major, mount.minor));
        let mounted = fs::canonicalize(link)
            .map_err(|error| io_reason("revalidate mounted device", error))?;
        if mounted != self.mounted_sysfs_path {
            return Err(Reason::DeviceChanged);
        }
        let mounted_device = read_device(&mounted).map_err(observation_reason)?;
        if (mounted_device.major, mounted_device.minor) != (mount.major, mount.minor) {
            return Err(Reason::DeviceChanged);
        }
        let device = read_device(&self.sysfs_path).map_err(observation_reason)?;
        if (device.major, device.minor) != self.number
            || dependency_count(&self.sysfs_path).map_err(observation_reason)? != 0
        {
            return Err(Reason::DeviceChanged);
        }
        Ok(())
    }
}

fn same_held_inode(held: &File, path: &Path) -> Result<(), Reason> {
    // Holding the inode detects removal/recreation, including dev_t reuse.
    let held = held
        .metadata()
        .map_err(|error| io_reason("stat held device identity", error))?;
    let current =
        fs::metadata(path).map_err(|error| io_reason("revalidate device identity", error))?;
    if (held.dev(), held.ino()) != (current.dev(), current.ino()) || held.nlink() == 0 {
        return Err(Reason::DeviceChanged);
    }
    Ok(())
}

fn run(command: &mut Command, timeout: Duration) -> Result<(i32, Vec<u8>), Reason> {
    let mut child = command.spawn().map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => Reason::ToolMissing,
        _ => io_reason("execute smartctl", error),
    })?;
    let result = collect_output(&mut child, timeout);
    if result.is_err() {
        // Cleanup may wait on uninterruptible kernel I/O, but only this dedicated
        // worker and its single global slot remain occupied; HTTP has a deadline.
        if let Err(error) = child.kill() {
            tracing::debug!(?error, "stop SMART child");
        }
        if let Err(error) = child.wait() {
            tracing::debug!(?error, "reap SMART child");
        }
    }
    result
}

fn collect_output(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<(i32, Vec<u8>), Reason> {
    let mut stdout = child
        .stdout
        .take()
        .expect("invariant: SMART stdout is piped");
    rustix::fs::fcntl_setfl(&stdout, rustix::fs::OFlags::NONBLOCK)
        .map_err(|error| io_reason("set SMART pipe nonblocking", error.into()))?;
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut status = None;
    loop {
        if Instant::now() >= deadline {
            return Err(Reason::TimedOut);
        }
        let mut chunk = [0; 4096];
        match stdout.read(&mut chunk) {
            Ok(0) if status.is_some() => {
                return Ok((status.expect("invariant: status checked"), bytes));
            }
            Ok(count) => {
                if bytes.len() + count > OUTPUT_LIMIT {
                    return Err(Reason::InvalidOutput);
                }
                bytes.extend_from_slice(&chunk[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(io_reason("read SMART output", error)),
        }
        if status.is_none() {
            status = child
                .try_wait()
                .map_err(|error| io_reason("wait SMART child", error))?
                .map(|status| status.code().ok_or(Reason::QueryFailed))
                .transpose()?;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[derive(data_serialization::Deserialize)]
struct SmartOutput {
    smartctl: SmartExit,
    smart_status: Option<SmartStatus>,
    temperature: Option<Temperature>,
    power_on_time: Option<PowerOnTime>,
    power_mode: Option<PowerMode>,
}
#[derive(data_serialization::Deserialize)]
struct SmartExit {
    exit_status: i32,
}
#[derive(data_serialization::Deserialize)]
struct SmartStatus {
    passed: bool,
}
#[derive(data_serialization::Deserialize)]
struct Temperature {
    current: i32,
}
#[derive(data_serialization::Deserialize)]
struct PowerOnTime {
    hours: u64,
}
#[derive(data_serialization::Deserialize)]
struct PowerMode {
    name: String,
}

fn parse(
    bytes: &[u8],
    status: i32,
    observed_at_unix_ms: u64,
    device_path: String,
) -> Result<DiskHealth, Reason> {
    let value: SmartOutput = json_codec::from_slice(bytes).map_err(|error| {
        tracing::debug!(?error, "invalid SMART JSON");
        Reason::InvalidOutput
    })?;
    if value.smartctl.exit_status != status {
        return Err(Reason::InvalidOutput);
    }
    if status == 3 {
        return Err(match value.power_mode.as_ref().map(|v| v.name.as_str()) {
            Some("SLEEP" | "STANDBY" | "STANDBY_Y") => Reason::LowPower,
            _ => {
                tracing::debug!(status, "SMART query failed without a low-power report");
                Reason::QueryFailed
            }
        });
    }
    if !(0..=255).contains(&status) || status & 7 != 0 {
        tracing::debug!(status, "SMART query failed before a complete device report");
        return Err(Reason::QueryFailed);
    }
    let passed = value.smart_status.ok_or(Reason::IncompleteOutput)?.passed;
    if passed && status & 8 != 0 {
        return Err(Reason::InvalidOutput);
    }
    Ok(DiskHealth::Smart {
        observed_at_unix_ms,
        device_path,
        assessment: if !passed || status & 16 != 0 {
            DiskHealthAssessment::Failed
        } else if status & (32 | 64 | 128) != 0 {
            DiskHealthAssessment::Warning
        } else {
            DiskHealthAssessment::Passed
        },
        temperature_celsius: value.temperature.map(|v| v.current),
        power_on_hours: value.power_on_time.map(|v| v.hours),
    })
}

fn observation_reason(error: ObservationError) -> Reason {
    tracing::debug!(?error, "SMART device mapping unavailable");
    match error {
        ObservationError::Io { source, .. } if source.kind() == io::ErrorKind::PermissionDenied => {
            Reason::PermissionDenied
        }
        ObservationError::MountMissing(_) => Reason::DeviceChanged,
        ObservationError::Io { .. } | ObservationError::Invalid { .. } => Reason::MappingUnknown,
    }
}
fn io_reason(operation: &'static str, error: io::Error) -> Reason {
    tracing::debug!(operation, ?error, "SMART observation unavailable");
    if error.kind() == io::ErrorKind::PermissionDenied {
        Reason::PermissionDenied
    } else {
        Reason::QueryFailed
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/disk_health.rs"]
mod tests;
